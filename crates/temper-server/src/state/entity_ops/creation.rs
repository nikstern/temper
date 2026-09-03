use sha2::{Digest, Sha256};
use temper_runtime::persistence::schema_deployment::SchemaExecutionPin;
use temper_runtime::persistence::{EntityKeyRow, FirstEventCommit, PersistenceEnvelope};
use temper_runtime::tenant::TenantId;

use crate::entity_actor::EntityState;
use temper_wasm_sdk::data::ManifestValueSourceV1;

use super::ServerState;

pub(crate) fn actor_creation_contract(
    state: &ServerState,
    tenant: &TenantId,
    entity_type: &str,
    entity_id: &str,
    initial_fields: &serde_json::Value,
    schema_pin: Option<&SchemaExecutionPin>,
) -> Result<temper_runtime::persistence::CreationContract, String> {
    let mut fields = initial_fields
        .as_object()
        .ok_or_else(|| "creation fields are not an object".to_string())?
        .clone();
    let (entity, schema_digest) = {
        let registry = state
            .registry
            .read()
            .map_err(|error| format!("registry lock poisoned: {error}"))?;
        let config = match schema_pin {
            Some(pin) => Some(
                registry
                    .get_scoped_config_at_digest(tenant, &pin.scope, &pin.bundle_digest)
                    .ok_or_else(|| "scoped schema configuration is unavailable".to_string())?,
            ),
            None => registry.get_tenant(tenant),
        };
        match config {
            Some(config) => (
                creation_manifest(&config.creation_manifests, entity_type)?.clone(),
                schema_pin.map_or_else(
                    || format!("{:x}", Sha256::digest(config.csdl_xml.as_bytes())),
                    |pin| pin.bundle_digest.clone(),
                ),
            ),
            None if schema_pin.is_none() => (
                creation_manifest(&state.legacy_creation_manifests, entity_type)?.clone(),
                format!("{:x}", Sha256::digest(state.csdl_xml.as_bytes())),
            ),
            None => return Err("scoped schema configuration is unavailable".to_string()),
        }
    };
    for property in &entity.properties {
        if property.source == ManifestValueSourceV1::EntityId {
            fields
                .entry(property.canonical_name.clone())
                .or_insert_with(|| serde_json::Value::String(entity_id.to_string()));
        }
    }
    crate::application_data::materialize_actor_creation_fields(&entity, &mut fields);
    crate::application_data::compile_creation_contract(&entity, &schema_digest, &fields)
        .map_err(|error| error.to_string())
}

fn creation_manifest<'a>(
    manifests: &'a std::collections::BTreeMap<String, temper_wasm_sdk::data::ManifestEntityV1>,
    entity_type: &str,
) -> Result<&'a temper_wasm_sdk::data::ManifestEntityV1, String> {
    manifests
        .get(entity_type)
        .or_else(|| {
            manifests
                .values()
                .find(|candidate| candidate.entity_type.rsplit('.').next() == Some(entity_type))
        })
        .ok_or_else(|| format!("creation manifest is unavailable for '{entity_type}'"))
}

pub(super) fn actor_creation_contract_for_spawn(
    state: &ServerState,
    tenant: &TenantId,
    entity_type: &str,
    initial_fields: &serde_json::Value,
    schema_pin: Option<&SchemaExecutionPin>,
    entity_id: &str,
    recovered_stream: bool,
) -> Option<Option<temper_runtime::persistence::CreationContract>> {
    let mut canonical_fields = initial_fields.clone();
    canonical_fields
        .as_object_mut()?
        .entry("Id")
        .or_insert_with(|| serde_json::Value::String(entity_id.to_string()));
    let contract = actor_creation_contract(
        state,
        tenant,
        entity_type,
        entity_id,
        &canonical_fields,
        schema_pin,
    );
    if let Err(error) = &contract
        && state.event_journal().is_some()
        && !recovered_stream
    {
        tracing::error!(
            tenant = %tenant,
            entity_type,
            entity_id,
            error,
            "refusing to spawn a persistent actor without a creation contract"
        );
        return None;
    }
    Some(contract.ok())
}

pub(super) struct FirstEventInput<'a> {
    pub(super) entity_state: &'a EntityState,
    pub(super) persistence_id: &'a str,
    pub(super) event: PersistenceEnvelope,
}

pub(super) fn first_event_commit(
    state: &ServerState,
    tenant: &TenantId,
    entity_type: &str,
    entity_id: &str,
    input: FirstEventInput<'_>,
) -> Option<FirstEventCommit> {
    let contract = actor_creation_contract(
        state,
        tenant,
        entity_type,
        entity_id,
        &input.entity_state.fields,
        None,
    )
    .ok()?;
    let declared_keys = state.declared_keys_for(tenant, entity_type);
    let mut key_rows = declared_keys
        .iter()
        .filter_map(|key| {
            input.entity_state.fields.as_object().and_then(|fields| {
                crate::key_index::canonical_key_hash(&key.name, &key.properties, fields).map(
                    |key_hash| EntityKeyRow {
                        key_name: key.name.clone(),
                        key_hash,
                    },
                )
            })
        })
        .collect::<Vec<_>>();
    key_rows.sort_by(|left, right| {
        (&left.key_name, &left.key_hash).cmp(&(&right.key_name, &right.key_hash))
    });
    Some(FirstEventCommit {
        tenant: tenant.to_string(),
        entity_type: entity_type.to_string(),
        entity_id: entity_id.to_string(),
        persistence_id: input.persistence_id.to_string(),
        event: input.event,
        contract_revision: contract.version,
        schema_identity: contract.schema_digest.clone(),
        declared_key_signature: crate::application_data::declared_key_signature(
            &declared_keys,
            &contract,
        ),
        contract,
        key_rows,
        vector_rows: Vec::new(),
        reconcile_vectors: false,
        projection: None,
    })
}

impl ServerState {
    /// Reconcile immutable sequence-1 contracts and exact keys for legacy streams.
    #[tracing::instrument(skip_all, fields(otel.name = "entity.populate_creation_contracts", tenant = %tenant))]
    pub async fn populate_creation_contracts(&self, tenant: &TenantId) {
        crate::state::projection_backfill::populate_creation_contracts(self, tenant).await;
    }
}
