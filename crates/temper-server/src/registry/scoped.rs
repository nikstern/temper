use super::*;

impl SpecRegistry {
    /// Stage one immutable scoped registry bundle without changing any reader.
    pub fn stage_scoped_bundle(
        &mut self,
        tenant: TenantId,
        scope: SchemaScope,
        digest: String,
        csdl: CsdlDocument,
        csdl_xml: String,
        ioa_sources: &[(&str, &str)],
    ) -> Result<(), RegistryError> {
        self.stage_scoped_bundle_with_modules(
            tenant,
            scope,
            digest,
            csdl,
            csdl_xml,
            ioa_sources,
            BTreeMap::new(),
        )
    }

    /// Stage a strictly linked v2 scoped bundle without modules.
    pub fn stage_scoped_bundle_v2(
        &mut self,
        tenant: TenantId,
        scope: SchemaScope,
        digest: String,
        csdl: CsdlDocument,
        csdl_xml: String,
        ioa_sources: &[(&str, &str)],
    ) -> Result<(), RegistryError> {
        self.stage_scoped_bundle_v2_with_modules(
            tenant,
            scope,
            digest,
            csdl,
            csdl_xml,
            ioa_sources,
            BTreeMap::new(),
        )
    }

    /// Stage one immutable scoped registry bundle and its exact module closure.
    #[expect(
        clippy::too_many_arguments,
        reason = "the immutable staging boundary keeps every authority input explicit"
    )]
    pub fn stage_scoped_bundle_with_modules(
        &mut self,
        tenant: TenantId,
        scope: SchemaScope,
        digest: String,
        csdl: CsdlDocument,
        csdl_xml: String,
        ioa_sources: &[(&str, &str)],
        modules: BTreeMap<String, ScopedModuleDescriptor>,
    ) -> Result<(), RegistryError> {
        self.stage_scoped_bundle_with_contract(
            tenant,
            scope,
            digest,
            csdl,
            csdl_xml,
            ioa_sources,
            modules,
            RegistryCanonicalization::Legacy,
        )
    }

    /// Stage one immutable strictly linked v2 bundle and module closure.
    #[expect(
        clippy::too_many_arguments,
        reason = "the immutable staging boundary keeps every authority input explicit"
    )]
    pub fn stage_scoped_bundle_v2_with_modules(
        &mut self,
        tenant: TenantId,
        scope: SchemaScope,
        digest: String,
        csdl: CsdlDocument,
        csdl_xml: String,
        ioa_sources: &[(&str, &str)],
        modules: BTreeMap<String, ScopedModuleDescriptor>,
    ) -> Result<(), RegistryError> {
        self.stage_scoped_bundle_with_contract(
            tenant,
            scope,
            digest,
            csdl,
            csdl_xml,
            ioa_sources,
            modules,
            RegistryCanonicalization::StrictV2,
        )
    }

    /// Stage a persisted frozen-v1 bundle while retaining its exact metadata bytes.
    #[expect(
        clippy::too_many_arguments,
        reason = "the immutable staging boundary keeps every authority input explicit"
    )]
    pub fn stage_scoped_bundle_persisted_v1_with_modules(
        &mut self,
        tenant: TenantId,
        scope: SchemaScope,
        digest: String,
        csdl: CsdlDocument,
        csdl_xml: String,
        ioa_sources: &[(&str, &str)],
        modules: BTreeMap<String, ScopedModuleDescriptor>,
    ) -> Result<(), RegistryError> {
        self.stage_scoped_bundle_with_contract(
            tenant,
            scope,
            digest,
            csdl,
            csdl_xml,
            ioa_sources,
            modules,
            RegistryCanonicalization::PersistedV1,
        )
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "the immutable staging boundary keeps every authority input explicit"
    )]
    fn stage_scoped_bundle_with_contract(
        &mut self,
        tenant: TenantId,
        scope: SchemaScope,
        digest: String,
        csdl: CsdlDocument,
        csdl_xml: String,
        ioa_sources: &[(&str, &str)],
        modules: BTreeMap<String, ScopedModuleDescriptor>,
        canonicalization: RegistryCanonicalization,
    ) -> Result<(), RegistryError> {
        let key = (tenant.clone(), scope.clone(), digest.clone());
        if let Some(existing) = self.scoped_bundles.get(&key) {
            let existing_modules = self.scoped_modules_for_key(&tenant, &scope, &digest);
            let identical = existing.csdl_xml.as_str() == csdl_xml
                && existing.entities.len() == ioa_sources.len()
                && ioa_sources.iter().all(|(entity, source)| {
                    existing
                        .entities
                        .get(*entity)
                        .is_some_and(|spec| spec.ioa_source == *source)
                })
                && existing_modules == modules;
            if identical {
                return Ok(());
            }
            return Err(RegistryError::ScopedBundleConflict {
                tenant: tenant.to_string(),
                scope: scope.id,
                digest,
            });
        }
        let mut isolated = SpecRegistry::new();
        match canonicalization {
            RegistryCanonicalization::StrictV2 => isolated
                .try_register_tenant_v2_with_reactions_and_constraints(
                    tenant.clone(),
                    csdl,
                    csdl_xml,
                    ioa_sources,
                    TenantRegistrationOptions::default(),
                )?,
            RegistryCanonicalization::PersistedV1 => isolated.try_register_tenant_persisted_v1(
                tenant.clone(),
                csdl,
                csdl_xml,
                ioa_sources,
            )?,
            RegistryCanonicalization::Legacy => {
                isolated.try_register_tenant(tenant.clone(), csdl, csdl_xml, ioa_sources)?;
            }
        }
        let config = isolated
            .tenants
            .remove(&tenant)
            .expect("successful isolated registration must create tenant config");
        self.scoped_bundles.insert(key, config);
        for (module_name, descriptor) in modules {
            self.scoped_modules.insert(
                (tenant.clone(), scope.clone(), digest.clone(), module_name),
                descriptor,
            );
        }
        Ok(())
    }

    fn scoped_modules_for_key(
        &self,
        tenant: &TenantId,
        scope: &SchemaScope,
        digest: &str,
    ) -> BTreeMap<String, ScopedModuleDescriptor> {
        self.scoped_modules
            .iter()
            .filter(|((known_tenant, known_scope, known_digest, _), _)| {
                known_tenant == tenant && known_scope == scope && known_digest == digest
            })
            .map(|((_, _, _, name), descriptor)| (name.clone(), descriptor.clone()))
            .collect()
    }

    /// Atomically select one already-staged immutable bundle for a scope.
    pub fn activate_scoped_bundle(
        &mut self,
        tenant: &TenantId,
        scope: &SchemaScope,
        digest: &str,
        expected_predecessor: Option<&str>,
    ) -> Result<(), RegistryError> {
        let scope_key = (tenant.clone(), scope.clone());
        if self.active_scopes.get(&scope_key).map(String::as_str) != expected_predecessor {
            return Err(RegistryError::ScopedPredecessorMismatch {
                tenant: tenant.to_string(),
                scope: scope.id.clone(),
            });
        }
        if !self
            .scoped_bundles
            .contains_key(&(tenant.clone(), scope.clone(), digest.to_string()))
        {
            return Err(RegistryError::ScopedBundleMissing {
                tenant: tenant.to_string(),
                scope: scope.id.clone(),
                digest: digest.to_string(),
            });
        }
        self.active_scopes.insert(scope_key, digest.to_string());
        Ok(())
    }

    /// Remove the active pointer only when it still names the expected digest.
    pub fn retire_scoped_bundle(
        &mut self,
        tenant: &TenantId,
        scope: &SchemaScope,
        expected_digest: &str,
    ) -> Result<(), RegistryError> {
        let key = (tenant.clone(), scope.clone());
        if self.active_scopes.get(&key).map(String::as_str) != Some(expected_digest) {
            return Err(RegistryError::ScopedPredecessorMismatch {
                tenant: tenant.to_string(),
                scope: scope.id.clone(),
            });
        }
        self.active_scopes.remove(&key);
        Ok(())
    }

    /// Explicitly allow or deny tenant-global compatibility for an unactivated scope.
    pub fn set_scope_global_compatibility(
        &mut self,
        tenant: TenantId,
        scope: SchemaScope,
        compatible: bool,
    ) {
        let key = (tenant, scope);
        if compatible {
            self.global_compatible_scopes.insert(key);
        } else {
            self.global_compatible_scopes.remove(&key);
        }
    }

    /// Whether scope creation explicitly selected tenant-global compatibility.
    pub fn scope_allows_global_compatibility(
        &self,
        tenant: &TenantId,
        scope: &SchemaScope,
    ) -> bool {
        self.global_compatible_scopes
            .contains(&(tenant.clone(), scope.clone()))
    }

    /// Current immutable digest for an exact scope, without global fallback.
    pub fn active_scope_digest(&self, tenant: &TenantId, scope: &SchemaScope) -> Option<&str> {
        self.active_scopes
            .get(&(tenant.clone(), scope.clone()))
            .map(String::as_str)
    }

    /// Exact active scoped config; malformed/missing scopes never fall back.
    pub fn get_scoped_config(
        &self,
        tenant: &TenantId,
        scope: &SchemaScope,
    ) -> Option<&TenantConfig> {
        let digest = self.active_scope_digest(tenant, scope)?;
        self.scoped_bundles
            .get(&(tenant.clone(), scope.clone(), digest.to_string()))
    }

    /// Exact immutable scoped config by digest, including retired predecessors.
    pub fn get_scoped_config_at_digest(
        &self,
        tenant: &TenantId,
        scope: &SchemaScope,
        digest: &str,
    ) -> Option<&TenantConfig> {
        self.scoped_bundles
            .get(&(tenant.clone(), scope.clone(), digest.to_string()))
    }

    /// Resolve one scoped entity set without consulting tenant-global metadata.
    pub fn resolve_scoped_entity_type(
        &self,
        tenant: &TenantId,
        scope: &SchemaScope,
        entity_set: &str,
    ) -> Option<String> {
        self.get_scoped_config(tenant, scope)
            .and_then(|config| config.entity_set_map.get(entity_set).cloned())
    }

    /// Snapshot one exact scoped transition table without global fallback.
    pub fn get_scoped_table(
        &self,
        tenant: &TenantId,
        scope: &SchemaScope,
        entity_type: &str,
    ) -> Option<Arc<TransitionTable>> {
        self.get_scoped_config(tenant, scope)
            .and_then(|config| entity_spec_for_type(config, entity_type))
            .map(EntitySpec::table)
    }

    /// Snapshot one exact immutable table without consulting the active pointer.
    pub fn get_scoped_table_at_digest(
        &self,
        tenant: &TenantId,
        scope: &SchemaScope,
        digest: &str,
        entity_type: &str,
    ) -> Option<Arc<TransitionTable>> {
        self.get_scoped_config_at_digest(tenant, scope, digest)
            .and_then(|config| entity_spec_for_type(config, entity_type))
            .map(EntitySpec::table)
    }

    /// Exact immutable scoped entity spec, including its integration metadata.
    pub fn get_scoped_spec_at_digest(
        &self,
        tenant: &TenantId,
        scope: &SchemaScope,
        digest: &str,
        entity_type: &str,
    ) -> Option<&EntitySpec> {
        self.get_scoped_config_at_digest(tenant, scope, digest)
            .and_then(|config| entity_spec_for_type(config, entity_type))
    }

    /// Exact immutable scoped module descriptor without tenant-global fallback.
    pub fn get_scoped_module_at_digest(
        &self,
        tenant: &TenantId,
        scope: &SchemaScope,
        digest: &str,
        module_name: &str,
    ) -> Option<&ScopedModuleDescriptor> {
        self.scoped_modules.get(&(
            tenant.clone(),
            scope.clone(),
            digest.to_string(),
            module_name.to_string(),
        ))
    }

    /// Snapshot reaction rules from one exact immutable scoped bundle.
    pub fn scoped_reaction_candidates_at_digest(
        &self,
        tenant: &TenantId,
        scope: &SchemaScope,
        digest: &str,
        entity_type: &str,
        action: &str,
    ) -> Vec<ReactionRule> {
        self.get_scoped_config_at_digest(tenant, scope, digest)
            .map(|config| {
                let mut rules = config.reactions.clone();
                for (source_entity_type, spec) in &config.entities {
                    for source_action in &spec.automaton.actions {
                        rules.extend(source_action.triggers.iter().filter_map(|trigger| {
                            synthesize_action_trigger_reaction(
                                source_entity_type,
                                &source_action.name,
                                trigger,
                            )
                        }));
                    }
                }
                rules
                    .into_iter()
                    .filter(|rule| {
                        rule.when.entity_type == entity_type
                            && rule
                                .when
                                .action
                                .as_deref()
                                .is_none_or(|name| name == action)
                    })
                    .collect()
            })
            .unwrap_or_default()
    }
}
