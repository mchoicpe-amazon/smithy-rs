/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0.
 */

//! IMDS Token Middleware
//! Requests to IMDS are two part:
//! 1. A PUT request to the token API is made
//! 2. A GET request is made to the requested API. The Token is added as a header.
//!
//! This module implements a middleware that will:
//! - Load a token via the token API
//! - Cache the token according to the TTL
//! - Retry token loading when it fails
//! - Attach the token to the request in the `x-aws-ec2-metadata-token` header

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use aws_http::user_agent::UserAgentStage;
use aws_types::os_shim_internal::TimeSource;
use http::{HeaderValue, Uri};
use smithy_client::erase::DynConnector;
use smithy_http::body::SdkBody;
use smithy_http::endpoint::Endpoint;
use smithy_http::middleware::AsyncMapRequest;
use smithy_http::operation;
use smithy_http::operation::Operation;
use smithy_http::operation::{Metadata, Request};
use smithy_http::response::ParseStrictResponse;
use smithy_http_tower::map_request::MapRequestLayer;

use crate::cache::ExpiringCache;
use crate::imds::client::{ImdsError, ImdsErrorPolicy, TokenError};
use smithy_client::retry;
use std::fmt::{Debug, Formatter};

/// Token Refresh Buffer
///
/// Tokens are cached to remove the need to reload the token between subsequent requests. To ensure
/// that a request never fails with a 401 (expired token), a buffer window exists during which the token
/// may not be expired, but will still be refreshed.
const TOKEN_REFRESH_BUFFER: Duration = Duration::from_secs(120);

const X_AWS_EC2_METADATA_TOKEN_TTL_SECONDS: &str = "x-aws-ec2-metadata-token-ttl-seconds";
const X_AWS_EC2_METADATA_TOKEN: &str = "x-aws-ec2-metadata-token";

/// IMDS Token
#[derive(Clone)]
struct Token {
    value: HeaderValue,
    expiry: SystemTime,
}

/// Token Middleware
///
/// Token middleware will load/cache a token when required and handle caching/expiry.
///
/// It will attach the token to the incoming request on the `x-aws-ec2-metadata-token` header.
#[derive(Clone)]
pub(super) struct TokenMiddleware {
    client: Arc<smithy_client::Client<DynConnector, MapRequestLayer<UserAgentStage>>>,
    token_parser: GetTokenResponseHandler,
    token: ExpiringCache<Token, ImdsError>,
    time_source: TimeSource,
    endpoint: Endpoint,
    token_ttl: Duration,
}

impl Debug for TokenMiddleware {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "ImdsTokenMiddleware")
    }
}

impl TokenMiddleware {
    pub(super) fn new(
        connector: DynConnector,
        time_source: TimeSource,
        endpoint: Endpoint,
        token_ttl: Duration,
        retry_config: retry::Config,
    ) -> Self {
        let inner_client = smithy_client::Client::new(connector).with_retry_config(retry_config);
        let client = Arc::new(inner_client);
        Self {
            client,
            token_parser: GetTokenResponseHandler {
                time: time_source.clone(),
            },
            token: ExpiringCache::new(TOKEN_REFRESH_BUFFER),
            time_source,
            endpoint,
            token_ttl,
        }
    }
    async fn add_token(&self, request: Request) -> Result<Request, ImdsError> {
        let preloaded_token = self
            .token
            .yield_or_clear_if_expired(self.time_source.now())
            .await;
        let token = match preloaded_token {
            Some(token) => Ok(token),
            None => {
                self.token
                    .get_or_load(|| async move { self.get_token().await })
                    .await
            }
        }?;
        request.augment(|mut request, _| {
            request
                .headers_mut()
                .insert(X_AWS_EC2_METADATA_TOKEN, token.value);
            Ok(request)
        })
    }

    async fn get_token(&self) -> Result<(Token, SystemTime), ImdsError> {
        let mut uri = Uri::from_static("/latest/api/token");
        self.endpoint.set_endpoint(&mut uri, None);
        let request = http::Request::builder()
            .header(
                X_AWS_EC2_METADATA_TOKEN_TTL_SECONDS,
                self.token_ttl.as_secs(),
            )
            .uri(uri)
            .method("PUT")
            .body(SdkBody::empty())
            .expect("valid HTTP request");
        let mut request = operation::Request::new(request);
        request.properties_mut().insert(super::USER_AGENT);

        let operation = Operation::new(request, self.token_parser.clone())
            .with_retry_policy(ImdsErrorPolicy)
            .with_metadata(Metadata::new("get-token", "imds"));
        let response = self
            .client
            .call(operation)
            .await
            .map_err(ImdsError::FailedToLoadToken)?;
        let expiry = response.expiry;
        Ok((response, expiry))
    }
}

impl AsyncMapRequest for TokenMiddleware {
    type Error = ImdsError;
    type Future = Pin<Box<dyn Future<Output = Result<Request, Self::Error>> + Send + 'static>>;

    fn apply(&self, request: Request) -> Self::Future {
        let this = self.clone();
        Box::pin(async move { this.add_token(request).await })
    }
}

#[derive(Clone)]
struct GetTokenResponseHandler {
    time: TimeSource,
}

impl ParseStrictResponse for GetTokenResponseHandler {
    type Output = Result<Token, TokenError>;

    fn parse(&self, response: &http::Response<bytes::Bytes>) -> Self::Output {
        match response.status().as_u16() {
            400 => return Err(TokenError::InvalidParameters),
            403 => return Err(TokenError::Forbidden),
            _ => {}
        }
        let value = HeaderValue::from_maybe_shared(response.body().clone())
            .map_err(|_| TokenError::InvalidToken)?;
        let ttl: u64 = response
            .headers()
            .get(X_AWS_EC2_METADATA_TOKEN_TTL_SECONDS)
            .ok_or(TokenError::NoTtl)?
            .to_str()
            .map_err(|_| TokenError::InvalidTtl)?
            .parse()
            .map_err(|_parse_error| TokenError::InvalidTtl)?;
        Ok(Token {
            value,
            expiry: self.time.now() + Duration::from_secs(ttl),
        })
    }
}