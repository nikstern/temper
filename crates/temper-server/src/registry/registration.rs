use super::*;
#[path = "registration/creation.rs"]
mod creation;
use creation::build_creation_manifests;

impl SpecRegistry {
    /// Register a tenant with its CSDL document and IOA specs.
    ///
    /// `ioa_sources` maps entity type name to IOA TOML source string.
    /// Each source is parsed into an [`Automaton`] and compiled into a
    /// [`TransitionTable`].
    ///
    /// If the tenant already exists, existing entity tables are hot-swapped
    /// via their [`SwapController`] so that live actors see the new table on
    /// their next action dispatch — no restart required. New entities are
    /// added; entities not in the new spec set are removed.
    pub fn register_tenant(
        &mut self,
        tenant: impl Into<TenantId>,
        csdl: CsdlDocument,
        csdl_xml: String,
        ioa_sources: &[(&str, &str)],
    ) {
        self.try_register_tenant_with_reactions_and_constraints(
            tenant,
            csdl,
            csdl_xml,
            ioa_sources,
            Vec::new(),
            None,
            false,
        )
        .unwrap_or_else(|e| panic!("{e}"));
    }

    /// Fallible variant of [`register_tenant`](Self::register_tenant).
    pub fn try_register_tenant(
        &mut self,
        tenant: impl Into<TenantId>,
        csdl: CsdlDocument,
        csdl_xml: String,
        ioa_sources: &[(&str, &str)],
    ) -> Result<(), RegistryError> {
        self.try_register_tenant_with_reactions_and_constraints(
            tenant,
            csdl,
            csdl_xml,
            ioa_sources,
            Vec::new(),
            None,
            false,
        )
    }

    /// Register a tenant with CSDL, IOA specs, reaction rules, and optional
    /// cross-entity invariant definitions.
    pub fn register_tenant_with_reactions_and_constraints(
        &mut self,
        tenant: impl Into<TenantId>,
        csdl: CsdlDocument,
        csdl_xml: String,
        ioa_sources: &[(&str, &str)],
        reactions: Vec<ReactionRule>,
        cross_invariants_source: Option<String>,
    ) {
        self.try_register_tenant_with_reactions_and_constraints(
            tenant,
            csdl,
            csdl_xml,
            ioa_sources,
            reactions,
            cross_invariants_source,
            false,
        )
        .unwrap_or_else(|e| panic!("{e}"));
    }

    /// Fallible variant of [`register_tenant_with_reactions_and_constraints`](Self::register_tenant_with_reactions_and_constraints).
    ///
    /// When `merge` is `true`, the new specs are **merged** into the existing
    /// tenant config rather than replacing it.  Existing entity types, CSDL
    /// schemas, and entity-set-map entries that are not part of the new
    /// submission are preserved.  This is the correct mode for
    /// `load-inline` (agent `submit_specs`), where the agent only submits
    /// its own entities and should not wipe platform types.
    #[allow(clippy::too_many_arguments)]
    #[instrument(skip_all, fields(otel.name = "registry.try_register_tenant_with_reactions_and_constraints"))]
    pub fn try_register_tenant_with_reactions_and_constraints(
        &mut self,
        tenant: impl Into<TenantId>,
        csdl: CsdlDocument,
        csdl_xml: String,
        ioa_sources: &[(&str, &str)],
        reactions: Vec<ReactionRule>,
        cross_invariants_source: Option<String>,
        merge: bool,
    ) -> Result<(), RegistryError> {
        self.try_register_tenant_with_contract(
            tenant,
            csdl,
            csdl_xml,
            ioa_sources,
            TenantRegistrationOptions {
                reactions,
                cross_invariants_source,
                merge,
            },
            RegistryCanonicalization::Legacy,
        )
    }

    /// Register a newly compiled v2 model with strict IOA/CSDL linking.
    pub fn try_register_tenant_v2_with_reactions_and_constraints(
        &mut self,
        tenant: impl Into<TenantId>,
        csdl: CsdlDocument,
        csdl_xml: String,
        ioa_sources: &[(&str, &str)],
        options: TenantRegistrationOptions,
    ) -> Result<(), RegistryError> {
        self.try_register_tenant_with_contract(
            tenant,
            csdl,
            csdl_xml,
            ioa_sources,
            options,
            RegistryCanonicalization::StrictV2,
        )
    }

    pub(super) fn try_register_tenant_persisted_v1(
        &mut self,
        tenant: impl Into<TenantId>,
        csdl: CsdlDocument,
        csdl_xml: String,
        ioa_sources: &[(&str, &str)],
    ) -> Result<(), RegistryError> {
        self.try_register_tenant_with_contract(
            tenant,
            csdl,
            csdl_xml,
            ioa_sources,
            TenantRegistrationOptions::default(),
            RegistryCanonicalization::PersistedV1,
        )
    }

    fn try_register_tenant_with_contract(
        &mut self,
        tenant: impl Into<TenantId>,
        csdl: CsdlDocument,
        csdl_xml: String,
        ioa_sources: &[(&str, &str)],
        options: TenantRegistrationOptions,
        canonicalization: RegistryCanonicalization,
    ) -> Result<(), RegistryError> {
        let TenantRegistrationOptions {
            reactions,
            cross_invariants_source,
            merge,
        } = options;
        let tenant = tenant.into();
        let tenant_name = tenant.to_string();
        let submitted_csdl_xml = csdl_xml;
        let structural_csdl = if merge {
            self.tenants
                .get(&tenant)
                .map(|existing| merge_csdl(existing.canonical_model.structural_csdl(), &csdl))
                .unwrap_or(csdl)
        } else {
            csdl
        };
        let mut complete_sources = if merge {
            self.tenants
                .get(&tenant)
                .map(|existing| {
                    existing
                        .entities
                        .iter()
                        .map(|(entity_type, spec)| (entity_type.clone(), spec.ioa_source.clone()))
                        .collect::<BTreeMap<_, _>>()
                })
                .unwrap_or_default()
        } else {
            BTreeMap::new()
        };
        for (entity_type, source) in ioa_sources {
            complete_sources.insert((*entity_type).to_string(), (*source).to_string());
        }
        let canonical_model = if canonicalization == RegistryCanonicalization::StrictV2 {
            link_registry_model(&tenant_name, &structural_csdl, &complete_sources)?
        } else if canonicalization == RegistryCanonicalization::PersistedV1 {
            link_legacy_registry_model(
                &tenant_name,
                &structural_csdl,
                submitted_csdl_xml.clone(),
                &complete_sources,
            )?
        } else {
            link_legacy_registry_model(
                &tenant_name,
                &structural_csdl,
                temper_spec::csdl::emit_csdl_xml(&structural_csdl),
                &complete_sources,
            )?
        };
        let csdl = canonical_model.emitted_csdl().clone();
        let csdl_xml = canonical_model.emitted_csdl_xml().to_owned();
        let creation_manifests = build_creation_manifests(
            &tenant_name,
            &canonical_model,
            &structural_csdl,
            complete_sources.keys().cloned(),
        )?;
        let cross_invariants = cross_invariants_source
            .as_ref()
            .filter(|s| !s.trim().is_empty())
            .map(|s| {
                parse_cross_invariants(s).map_err(|e| RegistryError::CrossInvariantParse {
                    tenant: tenant_name.clone(),
                    source: e.to_string(),
                })
            })
            .transpose()?;
        let relation_graph = build_relation_graph(&csdl, cross_invariants.as_ref());

        // Build entity set map from CSDL
        let mut entity_set_map = BTreeMap::new();
        for schema in &csdl.schemas {
            for container in &schema.entity_containers {
                for entity_set in &container.entity_sets {
                    let type_name = if canonicalization != RegistryCanonicalization::StrictV2 {
                        entity_set
                            .entity_type
                            .rsplit('.')
                            .next()
                            .unwrap_or(&entity_set.entity_type)
                            .to_string()
                    } else if complete_sources.contains_key(&entity_set.entity_type) {
                        entity_set.entity_type.clone()
                    } else {
                        let short = entity_set
                            .entity_type
                            .rsplit('.')
                            .next()
                            .unwrap_or(&entity_set.entity_type);
                        if complete_sources.contains_key(short)
                            && qualify_entity_type(&structural_csdl, short).as_deref()
                                == Ok(entity_set.entity_type.as_str())
                        {
                            short.to_string()
                        } else if qualify_entity_type(&structural_csdl, short).is_err() {
                            entity_set.entity_type.clone()
                        } else {
                            short.to_string()
                        }
                    };
                    entity_set_map.insert(entity_set.name.clone(), type_name);
                }
            }
        }

        if let Some(existing_config) = self.tenants.get_mut(&tenant) {
            // Acquire every actor-visible table before changing any revision
            // state. Once all guards are held, no actor can observe a partial
            // multi-entity activation; every table is updated before release.
            let locked_names = existing_config.entities.keys().cloned().collect::<Vec<_>>();
            let locked_tables = locked_names
                .iter()
                .map(|name| existing_config.entities[name].swap_controller().current())
                .collect::<Vec<_>>();
            let mut locked_guards = Vec::with_capacity(locked_tables.len());
            for (entity_type, table) in locked_names.iter().zip(&locked_tables) {
                locked_guards.push(table.write().map_err(|_| {
                    RegistryError::TableLockPoisoned {
                        tenant: tenant_name.clone(),
                        entity_type: entity_type.clone(),
                    }
                })?);
            }

            existing_config.csdl = Arc::new(csdl);
            existing_config.csdl_xml = Arc::new(csdl_xml);
            existing_config.entity_set_map = entity_set_map;
            existing_config.reactions = if merge {
                merge_reaction_rules(&existing_config.reactions, reactions)
            } else {
                reactions
            };
            existing_config.relation_graph = relation_graph;
            // In merge mode, an incoming payload without cross-invariants must
            // not wipe the ones previously loaded for the tenant — otherwise a
            // follow-up merge (e.g. Agent OS app bootstrap) silently disables
            // user-loaded enforcement. In replace mode, the caller is the new
            // source of truth and the overwrite is intentional.
            if !merge || cross_invariants.is_some() {
                existing_config.cross_invariants = cross_invariants;
                existing_config.cross_invariants_source = cross_invariants_source;
            }

            for (entity_type, ioa_source) in &complete_sources {
                let qualified = qualify_entity_type(&structural_csdl, entity_type)
                    .unwrap_or_else(|_| entity_type.clone());
                let automaton = canonical_model
                    .behavioral_entity(&qualified)
                    .and_then(|entity| entity.automaton().cloned())
                    .expect("linked source must have a parsed automaton");
                let mut table = TransitionTable::from_automaton(&automaton);
                table.schema_digest = Some(global_schema_digest(
                    &existing_config.csdl_xml,
                    entity_type,
                    ioa_source,
                ));
                let integrations = automaton.integrations.clone();

                if let Some(existing_spec) = existing_config.entities.get_mut(entity_type) {
                    let position = locked_names
                        .binary_search(entity_type)
                        .expect("existing entity table must be batch locked");
                    *locked_guards[position] = table;
                    existing_spec.swap_controller().record_batch_swap();
                    // Update metadata on the existing spec.
                    existing_spec.automaton = automaton;
                    existing_spec.integrations = integrations;
                    existing_spec.ioa_source = ioa_source.to_string();
                } else {
                    // New entity type — create fresh EntitySpec.
                    existing_config.entities.insert(
                        entity_type.to_string(),
                        EntitySpec {
                            automaton,
                            integrations,
                            swap: Arc::new(SwapController::new(table)),
                            ioa_source: ioa_source.clone(),
                        },
                    );
                }
            }

            // A CSDL-only hot reload changes the deployed schema identity even
            // when an entity's IOA was omitted from a merge payload.
            for (entity_type, spec) in &mut existing_config.entities {
                let digest =
                    global_schema_digest(&existing_config.csdl_xml, entity_type, &spec.ioa_source);
                let Ok(position) = locked_names.binary_search(entity_type) else {
                    continue;
                };
                if locked_guards[position].schema_digest.as_deref() != Some(digest.as_str()) {
                    let mut table = locked_guards[position].clone();
                    table.schema_digest = Some(digest);
                    *locked_guards[position] = table;
                    spec.swap_controller().record_batch_swap();
                }
            }

            if !merge {
                // Replace mode: remove entities no longer in the spec set.
                let new_entity_types: std::collections::BTreeSet<String> =
                    complete_sources.keys().cloned().collect();
                existing_config
                    .entities
                    .retain(|k, _| new_entity_types.contains(k));
            }

            // Rebuild webhook route index.
            existing_config.webhook_routes = build_webhook_routes(&existing_config.entities);

            if merge {
                // Merge mode: only reset verification for entities in this submission.
                for (entity_type, _) in ioa_sources {
                    existing_config
                        .verification
                        .insert(entity_type.to_string(), VerificationStatus::Pending);
                }
            } else {
                // Replace mode: reset verification for all entities.
                existing_config.verification = existing_config
                    .entities
                    .keys()
                    .map(|k| (k.clone(), VerificationStatus::Pending))
                    .collect();
            }
            existing_config.canonical_model = Arc::new(canonical_model);
            existing_config.creation_manifests = creation_manifests;
            drop(locked_guards);
        } else {
            // First registration: create new TenantConfig.
            let mut entities = BTreeMap::new();
            for (entity_type, ioa_source) in &complete_sources {
                let qualified = qualify_entity_type(&structural_csdl, entity_type)
                    .unwrap_or_else(|_| entity_type.clone());
                let automaton = canonical_model
                    .behavioral_entity(&qualified)
                    .and_then(|entity| entity.automaton().cloned())
                    .expect("linked source must have a parsed automaton");
                let mut table = TransitionTable::from_automaton(&automaton);
                table.schema_digest =
                    Some(global_schema_digest(&csdl_xml, entity_type, ioa_source));
                let integrations = automaton.integrations.clone();
                entities.insert(
                    entity_type.to_string(),
                    EntitySpec {
                        automaton,
                        integrations,
                        swap: Arc::new(SwapController::new(table)),
                        ioa_source: ioa_source.clone(),
                    },
                );
            }

            let verification = entities
                .keys()
                .map(|k| (k.clone(), VerificationStatus::Pending))
                .collect();

            let webhook_routes = build_webhook_routes(&entities);
            self.tenants.insert(
                tenant,
                TenantConfig {
                    canonical_model: Arc::new(canonical_model),
                    creation_manifests,
                    csdl: Arc::new(csdl),
                    csdl_xml: Arc::new(csdl_xml),
                    entity_set_map,
                    entities,
                    reactions,
                    relation_graph,
                    cross_invariants,
                    cross_invariants_source,
                    webhook_routes,
                    verification,
                },
            );
        }

        Ok(())
    }

    /// Register a tenant with CSDL, IOA specs, and reaction rules.
    pub fn register_tenant_with_reactions(
        &mut self,
        tenant: impl Into<TenantId>,
        csdl: CsdlDocument,
        csdl_xml: String,
        ioa_sources: &[(&str, &str)],
        reactions: Vec<ReactionRule>,
    ) {
        self.try_register_tenant_with_reactions_and_constraints(
            tenant,
            csdl,
            csdl_xml,
            ioa_sources,
            reactions,
            None,
            false,
        )
        .unwrap_or_else(|e| panic!("{e}"));
    }

    /// Fallible variant of [`register_tenant_with_reactions`](Self::register_tenant_with_reactions).
    pub fn try_register_tenant_with_reactions(
        &mut self,
        tenant: impl Into<TenantId>,
        csdl: CsdlDocument,
        csdl_xml: String,
        ioa_sources: &[(&str, &str)],
        reactions: Vec<ReactionRule>,
    ) -> Result<(), RegistryError> {
        self.try_register_tenant_with_reactions_and_constraints(
            tenant,
            csdl,
            csdl_xml,
            ioa_sources,
            reactions,
            None,
            false,
        )
    }
}
