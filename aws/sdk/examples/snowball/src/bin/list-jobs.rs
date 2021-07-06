/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0.
 */

use aws_sdk_snowball::{Config, Region};

#[tokio::main]
async fn main() -> Result<(), aws_sdk_snowball::Error> {
    let region = Region::new("us-east-1");
    let conf = Config::builder().region(region).build();
    let client = aws_sdk_snowball::Client::from_conf(conf);
    let jobs = client.list_jobs().send().await?;
    for job in jobs.job_list_entries.unwrap() {
        println!("JobId: {:?}", job.job_id);
    }

    Ok(())
}