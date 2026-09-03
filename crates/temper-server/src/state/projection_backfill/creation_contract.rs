//! Stable-pass backfill for immutable creation contracts and exact declared keys.

use std::collections::BTreeMap;

use temper_runtime::persistence::schema_deployment::split_scoped_journal_entity_id;
use temper_runtime::persistence::{
    CreationCoveragePublication, CreationMetadataRepair, EntityKeyRow, FirstEventCommit,
    FirstEventMetadata,
};
use temper_runtime::tenant::TenantId;

use crate::ServerState;
use crate::entity_actor::{EntityEvent, recover_entity_state_from_store};

/// Reconcile every known stream from sequence 1, then publish one coverage
/// record per exact global schema or scoped bundle only after a stable pass.
pub(in crate::state) async fn populate_creation_contracts(state: &ServerState, tenant: &TenantId) {
    let Some((store, backend)) = state.event_journal() else {
        return;
    };
    let entity_types = state
        .registry
        .read()
        .expect("registry lock poisoned")
        .entity_types(tenant)
        .into_iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>();
    for entity_type in entity_types {
        let Ok(mut entity_ids) = store
            .list_creation_source_ids_by_type(tenant.as_str(), &entity_type)
            .await
        else {
            tracing::warn!(tenant=%tenant, entity_type, "creation contract backfill enumeration failed");
            continue;
        };
        entity_ids.sort();
        let Ok(source_write_version) = store
            .creation_source_write_version(tenant.as_str(), &entity_type)
            .await
        else {
            tracing::warn!(tenant=%tenant, entity_type, "creation write-version read failed");
            continue;
        };
        let mut groups = BTreeMap::<(String, u32, String), (FirstEventMetadata, String)>::new();
        let mut failed = false;
        for journal_entity_id in &entity_ids {
            let (logical_entity_id, schema_pin) = split_scoped_journal_entity_id(journal_entity_id)
                .map_or((journal_entity_id.as_str(), None), |(id, pin)| {
                    (id, Some(pin))
                });
            let table = {
                let registry = state.registry.read().expect("registry lock poisoned");
                schema_pin
                    .as_ref()
                    .and_then(|pin| {
                        registry.get_scoped_table_at_digest(
                            tenant,
                            &pin.scope,
                            &pin.bundle_digest,
                            &entity_type,
                        )
                    })
                    .or_else(|| registry.get_table(tenant, &entity_type))
            };
            let Some(table) = table else {
                failed = true;
                break;
            };
            let persistence_id = format!("{tenant}:{entity_type}:{journal_entity_id}");
            let Ok(first_events) = store.read_events_limited(&persistence_id, 0, 1).await else {
                failed = true;
                break;
            };
            let Some(first_event) = first_events.into_iter().next() else {
                failed = true;
                break;
            };
            let Ok(created) = serde_json::from_value::<EntityEvent>(first_event.payload.clone())
            else {
                failed = true;
                break;
            };
            let Ok(current) = recover_entity_state_from_store(
                tenant.as_str(),
                &entity_type,
                journal_entity_id,
                &table,
                &store,
                backend,
                &serde_json::json!({}),
                state.blob_store_for_tenant(tenant).ok().as_ref(),
                true,
            )
            .await
            else {
                failed = true;
                break;
            };
            let Ok(contract) = crate::state::entity_ops::actor_creation_contract(
                state,
                tenant,
                &entity_type,
                journal_entity_id,
                &created.params,
                schema_pin.as_ref(),
            ) else {
                failed = true;
                break;
            };
            let declared_keys = table.keys.clone();
            let mut key_rows = if current.status == "Deleted" {
                Vec::new()
            } else {
                declared_keys
                    .iter()
                    .filter_map(|key| {
                        current.fields.as_object().and_then(|fields| {
                            crate::key_index::canonical_key_hash(&key.name, &key.properties, fields)
                                .map(|key_hash| EntityKeyRow {
                                    key_name: key.name.clone(),
                                    key_hash,
                                })
                        })
                    })
                    .collect::<Vec<_>>()
            };
            key_rows.sort_by(|left, right| {
                (&left.key_name, &left.key_hash).cmp(&(&right.key_name, &right.key_hash))
            });
            let metadata = FirstEventMetadata {
                contract_revision: contract.version,
                schema_identity: contract.schema_digest.clone(),
                declared_key_signature: crate::application_data::declared_key_signature(
                    &declared_keys,
                    &contract,
                ),
                contract: contract.clone(),
            };
            let commit = FirstEventCommit {
                tenant: tenant.to_string(),
                entity_type: entity_type.clone(),
                entity_id: journal_entity_id.clone(),
                persistence_id,
                event: first_event,
                contract,
                contract_revision: metadata.contract_revision,
                schema_identity: metadata.schema_identity.clone(),
                declared_key_signature: metadata.declared_key_signature.clone(),
                key_rows,
                vector_rows: Vec::new(),
                reconcile_vectors: false,
                projection: None,
            };
            if store
                .reconcile_creation_metadata(&CreationMetadataRepair {
                    first_event: commit,
                    source_sequence: current.sequence_nr,
                })
                .await
                .is_err()
            {
                failed = true;
                break;
            }
            let group = groups
                .entry((
                    metadata.schema_identity.clone(),
                    metadata.contract_revision,
                    metadata.declared_key_signature.clone(),
                ))
                .or_insert_with(|| (metadata, logical_entity_id.to_string()));
            group.1 = logical_entity_id.to_string();
        }
        let stable_ids = store
            .list_creation_source_ids_by_type(tenant.as_str(), &entity_type)
            .await
            .is_ok_and(|mut ids| {
                ids.sort();
                ids == entity_ids
            });
        let stable_version = store
            .creation_source_write_version(tenant.as_str(), &entity_type)
            .await
            .is_ok_and(|version| version == source_write_version);
        if failed || !stable_ids || !stable_version {
            tracing::warn!(tenant=%tenant, entity_type, "creation contract backfill remains incomplete");
            continue;
        }
        for (_, (metadata, cursor)) in groups {
            let publication = CreationCoveragePublication {
                tenant: tenant.to_string(),
                entity_type: entity_type.clone(),
                metadata,
                cursor,
                source_write_version,
            };
            if let Err(error) = store.publish_creation_coverage(&publication).await {
                tracing::warn!(tenant=%tenant, entity_type, error=%error, source_write_version,
                    "creation coverage publication lost its stable pass");
            }
        }
    }
}
