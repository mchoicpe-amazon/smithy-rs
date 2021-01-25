/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0.
 */

package software.amazon.smithy.rustsdk

import software.amazon.smithy.model.shapes.OperationShape
import software.amazon.smithy.rust.codegen.rustlang.CargoDependency
import software.amazon.smithy.rust.codegen.rustlang.Writable
import software.amazon.smithy.rust.codegen.rustlang.rust
import software.amazon.smithy.rust.codegen.rustlang.writable
import software.amazon.smithy.rust.codegen.smithy.RuntimeConfig
import software.amazon.smithy.rust.codegen.smithy.RuntimeType
import software.amazon.smithy.rust.codegen.smithy.generators.OperationCustomization
import software.amazon.smithy.rust.codegen.smithy.generators.OperationSection
import software.amazon.smithy.rust.codegen.smithy.generators.config.ConfigCustomization
import software.amazon.smithy.rust.codegen.smithy.generators.config.ServiceConfig

class RegionConfig(runtimeConfig: RuntimeConfig) : ConfigCustomization() {
    private val region = Region(runtimeConfig)
    override fun section(section: ServiceConfig) = writable {
        when (section) {
            is ServiceConfig.ConfigStruct -> rust("pub region: Option<#T::Region>,", region)
            is ServiceConfig.ConfigImpl -> emptySection
            is ServiceConfig.BuilderStruct ->
                rust("region: Option<#T::Region>,", region)
            ServiceConfig.BuilderImpl ->
                rust(
                    """
            pub fn region(mut self, region: impl #T::ProvideRegion) -> Self {
                self.region = region.region();
                self
            }
            """,
                    region
                )
            ServiceConfig.BuilderPreamble -> rust(
                """
                use #1T::ProvideRegion;
                let region = self.region.or_else(||#1T::default_provider().region());
            """,
                region
            )
            ServiceConfig.BuilderBuild -> rust(
                """region: region.clone(),""",
            )
        }
    }
}

class RegionConfigPlugin(private val operationShape: OperationShape) : OperationCustomization() {
    override fun section(section: OperationSection): Writable {
        return when (section) {
            OperationSection.ImplBlock -> emptySection
            OperationSection.Plugin -> writable {
                rust(
                    """
                if let Some(region) = &_config.region {
                    request.config_mut().insert(region.clone());

                }
                """
                )
            }
        }
    }
}

fun Region(runtimeConfig: RuntimeConfig) =
    RuntimeType("region", CargoDependency.OperationWip(runtimeConfig), "operationwip")
