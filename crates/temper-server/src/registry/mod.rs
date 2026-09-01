//! Per-tenant specification registry.
//!
//! The [`SpecRegistry`] maps `(TenantId, EntityType)` to parsed specifications
//! and transition tables. It replaces the flat `BTreeMap<String, TransitionTable>` // determinism-ok
//! in `ServerState`, enabling multi-tenant deployments where each tenant has
//! its own entity types and specs.

mod registration;
mod relations;
mod scoped;
pub mod types;

use std::collections::BTreeMap;
use std::sync::{Arc, RwLock};

use sha2::{Digest, Sha256};
use tracing::instrument;

use temper_jit::swap::SwapController;
use temper_jit::table::TransitionTable;
use temper_runtime::persistence::schema_deployment::SchemaScope;
use temper_runtime::tenant::TenantId;
use temper_spec::CanonicalSpecModel;
use temper_spec::FieldInvariant;
use temper_spec::bundle::IoaSourceInput;
use temper_spec::cross_invariant::parse_cross_invariants;
use temper_spec::csdl::{CsdlDocument, merge_csdl};

use crate::trigger::ReactionRegistry;
use crate::trigger::types::ReactionRule;

pub use types::*;

use relations::{build_relation_graph, build_webhook_routes, synthesize_action_trigger_reaction};

#[derive(Clone, Copy, PartialEq, Eq)]
enum RegistryCanonicalization {
    Legacy,
    PersistedV1,
    StrictV2,
}

fn global_schema_digest(csdl_xml: &str, entity_type: &str, ioa_source: &str) -> String {
    let mut hasher = Sha256::new();
    for component in [
        csdl_xml.as_bytes(),
        entity_type.as_bytes(),
        ioa_source.as_bytes(),
    ] {
        hasher.update((component.len() as u64).to_be_bytes());
        hasher.update(component);
    }
    format!("sha256:{:x}", hasher.finalize())
}

fn merge_reaction_rules(
    existing: &[ReactionRule],
    incoming: Vec<ReactionRule>,
) -> Vec<ReactionRule> {
    let mut merged: BTreeMap<String, ReactionRule> = existing
        .iter()
        .cloned()
        .map(|rule| (rule.name.clone(), rule))
        .collect();
    for rule in incoming {
        merged.insert(rule.name.clone(), rule);
    }
    merged.into_values().collect()
}

fn qualify_entity_type(csdl: &CsdlDocument, submitted: &str) -> Result<String, String> {
    if submitted.contains('.') {
        return Ok(submitted.to_string());
    }
    let matches = csdl
        .schemas
        .iter()
        .flat_map(|schema| {
            schema
                .entity_types
                .iter()
                .filter(move |entity| entity.name == submitted)
                .map(move |entity| format!("{}.{}", schema.namespace, entity.name))
        })
        .collect::<Vec<_>>();
    match matches.as_slice() {
        [qualified] => Ok(qualified.clone()),
        [] => Err(format!("IOA entity '{submitted}' is absent from CSDL")),
        _ => Err(format!(
            "IOA short name '{submitted}' is ambiguous across CSDL namespaces"
        )),
    }
}

fn link_registry_model(
    tenant: &str,
    csdl: &CsdlDocument,
    sources: &BTreeMap<String, String>,
) -> Result<CanonicalSpecModel, RegistryError> {
    let qualified = sources
        .iter()
        .map(|(entity_type, source)| {
            qualify_entity_type(csdl, entity_type)
                .map(|entity_type| IoaSourceInput {
                    entity_type,
                    source: source.clone(),
                })
                .map_err(|source| RegistryError::CanonicalLink {
                    tenant: tenant.to_string(),
                    source,
                })
        })
        .collect::<Result<Vec<_>, _>>()?;
    CanonicalSpecModel::link_v2_sources(csdl, &qualified).map_err(|error| {
        RegistryError::CanonicalLink {
            tenant: tenant.to_string(),
            source: error.to_string(),
        }
    })
}

fn link_legacy_registry_model(
    tenant: &str,
    csdl: &CsdlDocument,
    csdl_xml: String,
    sources: &BTreeMap<String, String>,
) -> Result<CanonicalSpecModel, RegistryError> {
    let mut automata = BTreeMap::new();
    let mut lifecycle_properties = BTreeMap::new();
    for (submitted, source) in sources {
        let automaton =
            temper_spec::parse_automaton(source).map_err(|error| RegistryError::CanonicalLink {
                tenant: tenant.to_string(),
                source: error.to_string(),
            })?;
        let qualified = qualify_entity_type(csdl, submitted).unwrap_or_else(|_| submitted.clone());
        let lifecycle_property = csdl.schemas.iter().find_map(|schema| {
            schema
                .entity_types
                .iter()
                .find(|entity| format!("{}.{}", schema.namespace, entity.name) == qualified)
                .and_then(|entity| {
                    entity
                        .properties
                        .iter()
                        .find(|property| property.name.eq_ignore_ascii_case("status"))
                        .or_else(|| {
                            entity
                                .properties
                                .iter()
                                .find(|property| property.name.eq_ignore_ascii_case("state"))
                        })
                        .map(|property| property.name.clone())
                })
        });
        if let Some(lifecycle_property) = lifecycle_property {
            lifecycle_properties.insert(qualified.clone(), lifecycle_property);
        }
        automata.insert(qualified, automaton);
    }
    Ok(CanonicalSpecModel::from_legacy_v1_with_emitted_xml(
        csdl,
        csdl_xml,
        automata,
        lifecycle_properties,
    ))
}

fn entity_spec_for_type<'a>(config: &'a TenantConfig, entity_type: &str) -> Option<&'a EntitySpec> {
    if let Some(spec) = config.entities.get(entity_type) {
        return Some(spec);
    }
    if entity_type.contains('.') {
        return None;
    }
    let suffix = format!(".{entity_type}");
    let mut matches = config
        .entities
        .iter()
        .filter(|(qualified, _)| qualified.ends_with(&suffix))
        .map(|(_, spec)| spec);
    let matched = matches.next()?;
    matches.next().is_none().then_some(matched)
}

/// Multi-tenant specification registry.
///
/// Thread-safe for concurrent reads. Registration is done at startup;
/// hot-swap via [`SwapController`](temper_jit::SwapController) can update
/// individual tables without replacing the entire registry.
#[derive(Debug, Clone, Default)]
pub struct SpecRegistry {
    tenants: BTreeMap<TenantId, TenantConfig>,
    scoped_bundles: BTreeMap<(TenantId, SchemaScope, String), TenantConfig>,
    scoped_modules: BTreeMap<(TenantId, SchemaScope, String, String), ScopedModuleDescriptor>,
    active_scopes: BTreeMap<(TenantId, SchemaScope), String>,
    global_compatible_scopes: std::collections::BTreeSet<(TenantId, SchemaScope)>,
}

impl SpecRegistry {
    /// Create an empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Build a [`ReactionRegistry`] from all tenants' reaction rules,
    /// including synthesized rules from `[[agent_trigger]]` sections.
    pub fn build_reaction_registry(&self) -> ReactionRegistry {
        let mut registry = ReactionRegistry::new();
        for (tenant, config) in &self.tenants {
            let mut rules = config.reactions.clone();
            // ADR-0046: synthesize reaction rules from [[action.triggers]]
            // entity-kind blocks on every entity's actions. Wasm/Webhook
            // kinds are handled by a separate runtime path.
            for (entity_type, spec) in &config.entities {
                for action in &spec.automaton.actions {
                    for trigger in &action.triggers {
                        if let Some(rule) =
                            synthesize_action_trigger_reaction(entity_type, &action.name, trigger)
                        {
                            rules.push(rule);
                        }
                    }
                }
            }
            if !rules.is_empty() {
                registry.register_tenant_rules(tenant.clone(), rules);
            }
        }
        registry
    }

    /// Look up a tenant's configuration.
    pub fn get_tenant(&self, tenant: &TenantId) -> Option<&TenantConfig> {
        self.tenants.get(tenant)
    }

    /// Look up a transition table for a specific tenant and entity type.
    ///
    /// Returns a snapshot of the current table. If a hot-swap has occurred
    /// since the last call, this returns the new table.
    pub fn get_table(&self, tenant: &TenantId, entity_type: &str) -> Option<Arc<TransitionTable>> {
        self.tenants
            .get(tenant)
            .and_then(|tc| entity_spec_for_type(tc, entity_type))
            .map(|es| es.table())
    }

    /// Get a live reference to the transition table's `RwLock`.
    ///
    /// Unlike [`get_table()`](Self::get_table) which returns a cloned snapshot,
    /// this returns the `Arc<RwLock<TransitionTable>>` from the [`SwapController`].
    /// Actors holding this reference will see hot-swapped tables on their next read.
    pub fn get_table_live(
        &self,
        tenant: &TenantId,
        entity_type: &str,
    ) -> Option<Arc<RwLock<TransitionTable>>> {
        self.tenants
            .get(tenant)
            .and_then(|tc| entity_spec_for_type(tc, entity_type))
            .map(|es| es.swap_controller().current())
    }

    /// Look up the entity type name for an entity set in a tenant.
    pub fn resolve_entity_type(&self, tenant: &TenantId, entity_set: &str) -> Option<String> {
        self.tenants
            .get(tenant)
            .and_then(|tc| tc.entity_set_map.get(entity_set).cloned())
    }

    /// Look up the IOA spec for a tenant and entity type.
    pub fn get_spec(&self, tenant: &TenantId, entity_type: &str) -> Option<&EntitySpec> {
        self.tenants
            .get(tenant)
            .and_then(|tc| entity_spec_for_type(tc, entity_type))
    }

    /// Look up the `[[field_invariant]]` declarations for a tenant and entity
    /// type, returning a cloned snapshot so the caller does not need to hold a
    /// registry read lock across subsequent async work.
    pub fn field_invariants_for(
        &self,
        tenant: &TenantId,
        entity_type: &str,
    ) -> Option<Vec<FieldInvariant>> {
        self.tenants
            .get(tenant)
            .and_then(|tc| entity_spec_for_type(tc, entity_type))
            .map(|es| es.automaton.field_invariants.clone())
    }

    /// Mutable access to the IOA spec for a tenant and entity type.
    pub fn get_spec_mut(
        &mut self,
        tenant: &TenantId,
        entity_type: &str,
    ) -> Option<&mut EntitySpec> {
        self.tenants
            .get_mut(tenant)
            .and_then(|tc| tc.entities.get_mut(entity_type))
    }

    /// Remove a tenant and all its specs from the registry.
    ///
    /// Returns `true` if the tenant was found and removed, `false` otherwise.
    #[instrument(skip_all, fields(otel.name = "registry.remove_tenant", tenant = %tenant))]
    pub fn remove_tenant(&mut self, tenant: &TenantId) -> bool {
        self.tenants.remove(tenant).is_some()
    }

    /// List all registered tenant IDs.
    pub fn tenant_ids(&self) -> Vec<&TenantId> {
        self.tenants.keys().collect()
    }

    /// List all entity types for a tenant.
    pub fn entity_types(&self, tenant: &TenantId) -> Vec<&str> {
        self.tenants
            .get(tenant)
            .map(|tc| tc.entities.keys().map(|k| k.as_str()).collect())
            .unwrap_or_default()
    }

    /// Set verification status for a specific entity type.
    #[instrument(skip_all, fields(otel.name = "registry.set_verification_status", tenant = %tenant, entity_type))]
    pub fn set_verification_status(
        &mut self,
        tenant: &TenantId,
        entity_type: &str,
        status: VerificationStatus,
    ) {
        if let Some(config) = self.tenants.get_mut(tenant) {
            config.verification.insert(entity_type.to_string(), status);
        }
    }

    /// Get verification status for a specific entity type.
    pub fn get_verification_status(
        &self,
        tenant: &TenantId,
        entity_type: &str,
    ) -> Option<&VerificationStatus> {
        self.tenants
            .get(tenant)
            .and_then(|tc| tc.verification.get(entity_type))
    }

    /// Get all verification statuses for a tenant.
    pub fn verification_statuses(
        &self,
        tenant: &TenantId,
    ) -> Option<&BTreeMap<String, VerificationStatus>> {
        self.tenants.get(tenant).map(|tc| &tc.verification)
    }
}

#[cfg(test)]
#[path = "canonical_tests.rs"]
mod canonical_tests;
#[cfg(test)]
mod tests;
