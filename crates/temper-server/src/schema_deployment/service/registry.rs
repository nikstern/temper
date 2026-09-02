use super::*;

#[derive(Debug, Clone, Copy)]
enum PersistedBundleContract {
    V1,
    V2,
}

fn persisted_bundle_contract(version: &str) -> Result<PersistedBundleContract, ServiceError> {
    match version {
        temper_spec::bundle::SCOPED_SPEC_BUNDLE_CONTRACT_V1 => Ok(PersistedBundleContract::V1),
        temper_spec::bundle::SCOPED_SPEC_BUNDLE_CONTRACT_V2 => Ok(PersistedBundleContract::V2),
        _ => Err(ServiceError::new(
            "invalid_bundle",
            format!("unsupported canonicalization version '{version}'"),
            false,
        )),
    }
}

impl GovernedSchemaDeploymentService<'_> {
    pub(super) fn stage_registry_bundle(
        &self,
        record: &SchemaDeploymentRecord,
    ) -> Result<(), ServiceError> {
        let csdl = temper_spec::parse_csdl(&record.bundle.canonical_csdl)
            .map_err(|error| ServiceError::new("invalid_bundle", error.to_string(), false))?;
        let contract = persisted_bundle_contract(&record.bundle.canonicalization_version)?;
        let source_keys_are_qualified = matches!(contract, PersistedBundleContract::V2);
        let owned_sources = record
            .bundle
            .canonical_ioa
            .iter()
            .map(|(qualified, source)| {
                (
                    if source_keys_are_qualified {
                        qualified.clone()
                    } else {
                        qualified
                            .rsplit('.')
                            .next()
                            .unwrap_or(qualified)
                            .to_string()
                    },
                    source.clone(),
                )
            })
            .collect::<Vec<_>>();
        let borrowed_sources = owned_sources
            .iter()
            .map(|(entity, source)| (entity.as_str(), source.as_str()))
            .collect::<Vec<_>>();
        let modules = record
            .bundle
            .wasm_module_digests
            .iter()
            .map(|(module_name, artifact_digest)| {
                let data_binding = record
                    .bundle
                    .wasm_module_data_bindings
                    .get(module_name)
                    .map(|stored| {
                        let manifest: temper_wasm_sdk::data::ModuleSdkManifest =
                            serde_json::from_str(&stored.canonical_manifest_json).map_err(
                                |error| {
                                    ServiceError::new(
                                        "invalid_bundle",
                                        format!(
                                            "scoped module '{module_name}' data binding is invalid: {error}"
                                        ),
                                        false,
                                    )
                                },
                            )?;
                        let actual = manifest
                            .binding_digest()
                            .map(|digest| format!("sha256:{digest}"))
                            .map_err(|error| {
                                ServiceError::new("invalid_bundle", error, false)
                            })?;
                        if actual != stored.binding_digest {
                            return Err(ServiceError::new(
                                "invalid_bundle",
                                format!(
                                    "scoped module '{module_name}' data binding digest mismatch"
                                ),
                                false,
                            ));
                        }
                        Ok(manifest)
                    })
                    .transpose()?;
                Ok((
                    module_name.clone(),
                    crate::registry::ScopedModuleDescriptor {
                        artifact_digest: artifact_digest.clone(),
                        data_binding,
                    },
                ))
            })
            .collect::<Result<std::collections::BTreeMap<_, _>, ServiceError>>()?;
        if record
            .bundle
            .wasm_module_data_bindings
            .keys()
            .any(|module_name| !record.bundle.wasm_module_digests.contains_key(module_name))
        {
            return Err(ServiceError::new(
                "invalid_bundle",
                "scoped module data binding has no artifact descriptor",
                false,
            ));
        }
        let mut registry = self.state.registry.write().map_err(|_| {
            ServiceError::new(
                "backend_unavailable",
                "spec registry lock is unavailable",
                true,
            )
        })?;
        let result = match contract {
            PersistedBundleContract::V2 => registry.stage_scoped_bundle_v2_with_modules(
                TenantId::new(&record.bundle.tenant),
                record.bundle.scope.clone(),
                record.bundle.digest.clone(),
                csdl,
                record.bundle.canonical_csdl.clone(),
                &borrowed_sources,
                modules,
            ),
            PersistedBundleContract::V1 => registry.stage_scoped_bundle_persisted_v1_with_modules(
                TenantId::new(&record.bundle.tenant),
                record.bundle.scope.clone(),
                record.bundle.digest.clone(),
                csdl,
                record.bundle.canonical_csdl.clone(),
                &borrowed_sources,
                modules,
            ),
        };
        result.map_err(|error| ServiceError::new("invalid_bundle", error.to_string(), false))?;
        registry
            .stage_scoped_cedar_policies(
                TenantId::new(&record.bundle.tenant),
                record.bundle.scope.clone(),
                record.bundle.digest.clone(),
                record.bundle.cedar_policies.clone(),
            )
            .map_err(|error| ServiceError::new("invalid_bundle", error.to_string(), false))
    }

    pub(crate) async fn recover_registry_pointer(
        &self,
        tenant: &str,
        scope: &SchemaScope,
    ) -> Result<(), ServiceError> {
        let Some(pointer) = self
            .store()?
            .active_schema_pointer(tenant, scope)
            .await
            .map_err(ServiceError::from_store)?
        else {
            return Ok(());
        };
        let tenant_id = TenantId::new(tenant);
        let registry_digest = self
            .state
            .registry
            .read()
            .map_err(|_| {
                ServiceError::new(
                    "backend_unavailable",
                    "spec registry lock is unavailable",
                    true,
                )
            })?
            .active_scope_digest(&tenant_id, scope)
            .map(str::to_string);
        if registry_digest.as_deref() == Some(pointer.bundle_digest.as_str()) {
            return Ok(());
        }
        let record = self
            .store()?
            .get_schema_deployment(tenant, scope, &pointer.bundle_digest)
            .await
            .map_err(ServiceError::from_store)?
            .ok_or_else(|| {
                ServiceError::new(
                    "backend_unavailable",
                    "active pointer lost its immutable bundle",
                    true,
                )
            })?;
        self.stage_registry_bundle(&record)?;
        self.state
            .registry
            .write()
            .map_err(|_| {
                ServiceError::new(
                    "backend_unavailable",
                    "spec registry lock is unavailable",
                    true,
                )
            })?
            .activate_scoped_bundle(
                &tenant_id,
                scope,
                &pointer.bundle_digest,
                registry_digest.as_deref(),
            )
            .map_err(|error| ServiceError::new("backend_unavailable", error.to_string(), true))
    }

    /// Hydrate one immutable bundle without changing the active scope pointer.
    pub(crate) async fn recover_registry_bundle(
        &self,
        tenant: &str,
        scope: &SchemaScope,
        bundle_digest: &str,
    ) -> Result<(), ServiceError> {
        let tenant_id = TenantId::new(tenant);
        if self
            .state
            .registry
            .read()
            .map_err(|_| {
                ServiceError::new(
                    "backend_unavailable",
                    "spec registry lock is unavailable",
                    true,
                )
            })?
            .get_scoped_config_at_digest(&tenant_id, scope, bundle_digest)
            .is_some()
        {
            return Ok(());
        }
        let record = self
            .store()?
            .get_schema_deployment(tenant, scope, bundle_digest)
            .await
            .map_err(ServiceError::from_store)?
            .ok_or_else(|| {
                ServiceError::new(
                    "backend_unavailable",
                    "pinned work lost its immutable bundle",
                    true,
                )
            })?;
        self.stage_registry_bundle(&record)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn persisted_staging_rejects_unknown_canonicalization_version() {
        let error = persisted_bundle_contract("scoped-spec-bundle/v999")
            .expect_err("unknown persisted contracts must not be reinterpreted as v1");
        assert_eq!(error.code(), "invalid_bundle");
        assert!(
            error
                .message()
                .contains("unsupported canonicalization version")
        );
    }
}
