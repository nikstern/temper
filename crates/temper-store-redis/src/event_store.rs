//! Redis-backed implementation of the [`EventStore`] trait.
//!
//! Uses Redis primitives:
//! - `LIST` per entity for ordered event journal entries
//! - `STRING` per entity for latest sequence number
//! - `STRING` per entity for snapshots
//! - `SET` per tenant to track distinct `(entity_type, entity_id)` pairs
//!
//! The `append()` operation uses a Lua script (`EVALSHA`) to atomically
//! check-and-set the sequence number, preventing lost-update races between
//! concurrent writers on the same entity.

use std::sync::Arc;

use fred::prelude::*;
use fred::types::scripts::Script;
use serde::Serialize;
use temper_runtime::persistence::schema_deployment::{
    SchemaScope, StreamPublicationFence, scoped_journal_pin_prefix, split_scoped_journal_entity_id,
};
use temper_runtime::persistence::{
    CreateOrVerifyRequest, CreateOrVerifyStoreOutcome, EventStore, PersistenceAppend,
    PersistenceAppendResult, PersistenceEnvelope, PersistenceError, storage_error,
};
use temper_runtime::tenant::parse_persistence_id_parts;

use crate::keys::{decode_lex_component, encode_lex_component};

fn redis_pre_commit(error: impl std::fmt::Display) -> PersistenceError {
    PersistenceError::PreCommit(error.to_string())
}

fn redis_post_commit(error: impl std::fmt::Display) -> PersistenceError {
    PersistenceError::PostCommit(error.to_string())
}

fn redis_acknowledgement_unknown(error: impl std::fmt::Display) -> PersistenceError {
    PersistenceError::AcknowledgementUnknown(error.to_string())
}

fn redis_malformed_result(
    result: &[i64],
    rejection_tags: &[i64],
    operation: &str,
) -> PersistenceError {
    match result.first() {
        Some(1) => redis_post_commit(format!(
            "malformed committed Redis {operation} result: {result:?}"
        )),
        Some(tag) if rejection_tags.contains(tag) => redis_pre_commit(format!(
            "malformed rejected Redis {operation} result: {result:?}"
        )),
        _ => redis_acknowledgement_unknown(format!(
            "unexpected Redis {operation} result: {result:?}"
        )),
    }
}

#[macro_use]
#[path = "event_store/event_store_core.rs"]
mod event_store_core;
#[macro_use]
#[path = "event_store/event_store_schema.rs"]
mod event_store_schema;
#[path = "event_store/batch.rs"]
mod batch;
#[path = "event_store/create_or_verify.rs"]
mod create_or_verify;
#[path = "event_store/create_or_verify_result.rs"]
mod create_or_verify_result;
#[path = "event_store/first_event.rs"]
mod first_event;
#[path = "event_store/first_event_mutation.rs"]
mod first_event_mutation;
#[path = "event_store/key_migration.rs"]
mod key_migration;
#[path = "event_store/keyspace.rs"]
mod keyspace;
#[path = "event_store/projection.rs"]
mod projection;
#[path = "event_store/records.rs"]
mod records;
#[path = "event_store/scripts.rs"]
mod scripts;
use records::{
    EntityRef, SegmentRecord, SnapshotHistoryRecord, SnapshotRecord, contract_record_json,
};

/// Redis-backed event store.
#[derive(Clone)]
pub struct RedisEventStore {
    client: Arc<fred::clients::Client>,
    append_script: Script,
    append_with_keys_script: Script,
    append_batch_script: Script,
    activate_unscoped_fence_script: Script,
    backfill_unscoped_index_script: Script,
    create_or_verify_script: Script,
    commit_first_event_script: Script,
    reconcile_creation_script: Script,
    publish_creation_coverage_script: Script,
    clustered: bool,
}

impl RedisEventStore {
    /// Connect to Redis using a URL such as `redis://localhost:6379/0`.
    pub async fn new(redis_url: &str) -> Result<Self, PersistenceError> {
        let config = Config::from_url(redis_url).map_err(storage_error)?;
        let clustered = config.server.is_clustered();
        let client = Builder::from_config(config)
            .build()
            .map_err(storage_error)?;
        client.init().await.map_err(storage_error)?;
        Ok(Self {
            client: Arc::new(client),
            append_script: Script::from_lua(scripts::APPEND_LUA),
            append_with_keys_script: Script::from_lua(scripts::APPEND_WITH_KEYS_LUA),
            append_batch_script: Script::from_lua(first_event_mutation::compose(
                scripts::APPEND_BATCH_LUA,
            )),
            activate_unscoped_fence_script: Script::from_lua(scripts::ACTIVATE_UNSCOPED_FENCE_LUA),
            backfill_unscoped_index_script: Script::from_lua(scripts::BACKFILL_UNSCOPED_INDEX_LUA),
            create_or_verify_script: Script::from_lua(first_event_mutation::compose(
                create_or_verify::CREATE_OR_VERIFY_LUA,
            )),
            commit_first_event_script: Script::from_lua(first_event_mutation::compose(
                first_event::COMMIT_FIRST_EVENT_LUA,
            )),
            reconcile_creation_script: Script::from_lua(
                first_event::RECONCILE_CREATION_METADATA_LUA,
            ),
            publish_creation_coverage_script: Script::from_lua(
                first_event::PUBLISH_CREATION_COVERAGE_LUA,
            ),
            clustered,
        })
    }

    /// Return a reference to the underlying Redis client.
    pub fn client(&self) -> &fred::clients::Client {
        &self.client
    }

    async fn update_segment_after_append(
        &self,
        tenant: &str,
        entity_type: &str,
        entity_id: &str,
        expected_sequence: u64,
        new_sequence: u64,
    ) -> Result<(), PersistenceError> {
        if new_sequence == expected_sequence {
            return Ok(());
        }
        let current_segment_key = Self::current_segment_key(tenant, entity_type, entity_id);
        let current_segment_raw: Option<String> = self
            .client
            .get(&current_segment_key)
            .await
            .map_err(storage_error)?;
        let segment_index = current_segment_raw
            .and_then(|raw| raw.parse::<u64>().ok())
            .unwrap_or(0);
        let segment_key = Self::segment_key(tenant, entity_type, entity_id, segment_index);
        let existing: Option<String> =
            self.client.get(&segment_key).await.map_err(storage_error)?;
        let mut record = existing
            .as_deref()
            .map(serde_json::from_str::<SegmentRecord>)
            .transpose()
            .map_err(|error| PersistenceError::Serialization(error.to_string()))?
            .unwrap_or_else(|| SegmentRecord {
                segment_index,
                start_sequence_nr: (expected_sequence + 1).max(1),
                end_sequence_nr: None,
                snapshot_sequence: None,
                event_count: 0,
                sealed_at: None,
                created_at: chrono::Utc::now(),
            });
        record.end_sequence_nr = Some(new_sequence);
        record.event_count = new_sequence.saturating_sub(record.start_sequence_nr) + 1;
        let encoded = serde_json::to_string(&record)
            .map_err(|error| PersistenceError::Serialization(error.to_string()))?;
        let _: () = self
            .client
            .set(&segment_key, encoded, None, None, false)
            .await
            .map_err(storage_error)?;
        let _: () = self
            .client
            .set(
                &current_segment_key,
                segment_index.to_string(),
                None,
                None,
                false,
            )
            .await
            .map_err(storage_error)?;
        Ok(())
    }

    /// Persist a trajectory entry as JSON into a capped Redis list.
    ///
    /// Uses RPUSH + LTRIM to maintain a bounded list of recent entries.
    pub async fn persist_trajectory(
        &self,
        tenant: &str,
        entry_json: &str,
        max_entries: i64,
    ) -> Result<(), PersistenceError> {
        let key = Self::trajectory_key(tenant);
        let _: i64 = self
            .client
            .rpush(&key, entry_json.to_string())
            .await
            .map_err(storage_error)?;
        // Trim to keep only the last `max_entries` items.
        let _: () = self
            .client
            .ltrim(&key, -max_entries, -1)
            .await
            .map_err(storage_error)?;
        Ok(())
    }

    /// Load recent trajectory entries from Redis (newest last).
    pub async fn load_recent_trajectories(
        &self,
        tenant: &str,
        limit: i64,
    ) -> Result<Vec<String>, PersistenceError> {
        let key = Self::trajectory_key(tenant);
        let entries: Vec<String> = self
            .client
            .lrange(&key, -limit, -1)
            .await
            .map_err(storage_error)?;
        Ok(entries)
    }

    async fn append_with_keys_inner(
        &self,
        persistence_id: &str,
        expected_sequence: u64,
        events: &[PersistenceEnvelope],
        key_rows: &[temper_runtime::persistence::EntityKeyRow],
    ) -> Result<u64, PersistenceError> {
        #[derive(Serialize)]
        struct LuaKeyRow<'a> {
            owner_field: String,
            #[serde(skip)]
            _key_name: &'a str,
        }

        let (tenant, entity_type, entity_id) =
            parse_persistence_id_parts(persistence_id).map_err(PersistenceError::PreCommit)?;
        let contracts_key = Self::create_or_verify_hash_key(tenant, entity_type, "contracts");
        let stored_contract: Option<String> = self
            .client
            .hget(&contracts_key, entity_id)
            .await
            .map_err(redis_pre_commit)?;
        let coverage_metadata = stored_contract
            .as_deref()
            .map(serde_json::from_str::<serde_json::Value>)
            .transpose()
            .map_err(|error| PersistenceError::Serialization(error.to_string()))?;
        let metadata_string = |name: &str| {
            coverage_metadata
                .as_ref()
                .and_then(|value| value.get(name))
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_string()
        };
        let schema_identity = metadata_string("schema_identity");
        let declared_key_signature = metadata_string("declared_key_signature");
        let contract_revision = coverage_metadata
            .as_ref()
            .and_then(|value| value.get("contract_revision"))
            .and_then(serde_json::Value::as_u64)
            .and_then(|value| u32::try_from(value).ok());
        let encoded_keys = key_rows
            .iter()
            .map(|row| LuaKeyRow {
                owner_field: format!("{}\0{}", row.key_name, row.key_hash),
                _key_name: &row.key_name,
            })
            .collect::<Vec<_>>();
        let mut args = vec![
            expected_sequence.to_string(),
            serde_json::to_string(&EntityRef {
                entity_type: entity_type.to_string(),
                entity_id: entity_id.to_string(),
            })
            .map_err(|error| PersistenceError::Serialization(error.to_string()))?,
            Self::journal_member(entity_type, entity_id),
            if split_scoped_journal_entity_id(entity_id).is_none() {
                encode_lex_component(entity_id)
            } else {
                String::new()
            },
            entity_id.to_string(),
            serde_json::to_string(&encoded_keys)
                .map_err(|error| PersistenceError::Serialization(error.to_string()))?,
            schema_identity.clone(),
            contract_revision.map_or_else(String::new, |value| value.to_string()),
            declared_key_signature.clone(),
            format!("[{}!", encode_lex_component(entity_type)),
            format!("[{}!\u{10ffff}", encode_lex_component(entity_type)),
        ];
        let mut sequence = expected_sequence;
        for event in events {
            sequence += 1;
            let mut event = event.clone();
            event.sequence_nr = sequence;
            args.push(
                serde_json::to_string(&event)
                    .map_err(|error| PersistenceError::Serialization(error.to_string()))?,
            );
        }
        let keys = vec![
            Self::seq_key(tenant, entity_type, entity_id),
            Self::events_key(tenant, entity_type, entity_id),
            Self::tenant_entities_key(tenant),
            Self::tenant_journals_key(tenant),
            Self::unscoped_journals_key(tenant, entity_type),
            Self::unscoped_generation_key(tenant, entity_type),
            Self::unscoped_fence_key(tenant, entity_type),
            Self::create_or_verify_hash_key(tenant, entity_type, "owners"),
            Self::create_or_verify_hash_key(tenant, entity_type, "entity_keys"),
            contracts_key,
            contract_revision.map_or_else(
                || {
                    Self::create_or_verify_hash_key(
                        tenant,
                        &format!("{entity_type}:{entity_id}"),
                        "unused_coverage",
                    )
                },
                |revision| {
                    Self::creation_coverage_key(
                        tenant,
                        entity_type,
                        &schema_identity,
                        revision,
                        &declared_key_signature,
                    )
                },
            ),
        ];
        let result: Vec<i64> = self
            .append_with_keys_script
            .evalsha_with_reload(&self.client, keys, args)
            .await
            .map_err(redis_acknowledgement_unknown)?;
        match result.as_slice() {
            [1, new_sequence] => {
                let new_sequence = u64::try_from(*new_sequence).map_err(redis_post_commit)?;
                self.update_segment_after_append(
                    tenant,
                    entity_type,
                    entity_id,
                    expected_sequence,
                    new_sequence,
                )
                .await
                .map_err(redis_post_commit)?;
                Ok(new_sequence)
            }
            [0, actual] => Err(PersistenceError::ConcurrencyViolation {
                expected: expected_sequence,
                actual: u64::try_from(*actual).map_err(redis_pre_commit)?,
            }),
            [-1, _] => Err(PersistenceError::PreCommit(
                "stream descriptor publication fence".to_string(),
            )),
            [-2, _] => Err(PersistenceError::PreCommit(
                "duplicate declared key".to_string(),
            )),
            _ => Err(redis_malformed_result(
                &result,
                &[0, -1, -2],
                "append-with-keys Lua",
            )),
        }
    }
}

impl EventStore for RedisEventStore {
    async fn reconcile_creation_metadata(
        &self,
        repair: &temper_runtime::persistence::CreationMetadataRepair,
    ) -> Result<(), PersistenceError> {
        self.reconcile_creation_metadata_inner(repair).await
    }

    async fn publish_creation_coverage(
        &self,
        publication: &temper_runtime::persistence::CreationCoveragePublication,
    ) -> Result<(), PersistenceError> {
        self.publish_creation_coverage_inner(publication).await
    }

    async fn commit_first_event(
        &self,
        commit: &temper_runtime::persistence::FirstEventCommit,
    ) -> Result<u64, PersistenceError> {
        self.commit_first_event_inner(commit).await
    }

    async fn create_or_verify(
        &self,
        request: &CreateOrVerifyRequest,
    ) -> Result<CreateOrVerifyStoreOutcome, PersistenceError> {
        request.first_event.validate()?;
        self.create_or_verify_inner(request).await
    }

    async fn acknowledge_create_or_verify_notification(
        &self,
        request: &CreateOrVerifyRequest,
    ) -> Result<(), PersistenceError> {
        self.acknowledge_notification_inner(request).await
    }

    async fn append_with_index_rows(
        &self,
        persistence_id: &str,
        expected_sequence: u64,
        events: &[PersistenceEnvelope],
        key_rows: &[temper_runtime::persistence::EntityKeyRow],
        _vector_rows: &[temper_runtime::persistence::EntityVectorRow],
        _reconcile_vectors: bool,
    ) -> Result<u64, PersistenceError> {
        self.append_with_keys_inner(persistence_id, expected_sequence, events, key_rows)
            .await
    }

    redis_event_store_core_methods!();
    redis_event_store_schema_methods!();
}

#[cfg(test)]
#[path = "event_store/tests.rs"]
mod tests;
