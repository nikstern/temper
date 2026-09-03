use serde::Serialize;
use temper_runtime::persistence::schema_deployment::split_scoped_journal_entity_id;
use temper_runtime::persistence::{FirstEventCommit, PersistenceError, storage_error};

use super::{EntityRef, RedisEventStore, SegmentRecord, contract_record_json};
use crate::keys::encode_lex_component;

/// Ordinary first-event transaction. This intentionally has no idempotency
/// key and never compares an existing owner: a non-empty stream returns the
/// same optimistic concurrency conflict as ordinary create.
pub(super) const COMMIT_FIRST_EVENT_LUA: &str = r#"
local current = tonumber(redis.call('GET', KEYS[1]) or '0')
if current ~= 0 then return {0, current} end
local entity_id = ARGV[1]
local key_rows = cjson.decode(ARGV[7])
local prior_journal_count = redis.call('ZLEXCOUNT', KEYS[4], ARGV[12], ARGV[13])
local can_advance_coverage = prior_journal_count == 0
local prior_covered_write_version = 0
if prior_journal_count > 0 then
    local coverage_json = redis.call('GET', KEYS[13])
    if coverage_json then
        local coverage = cjson.decode(coverage_json)
        local matching_contracts = 0
        local reconciled_write_version = 0
        for _, stored_json in ipairs(redis.call('HVALS', KEYS[7])) do
            local stored = cjson.decode(stored_json)
            if stored.schema_identity == ARGV[9]
                and stored.contract_revision == tonumber(ARGV[10])
                and stored.declared_key_signature == ARGV[11] then
                matching_contracts = matching_contracts + 1
                reconciled_write_version = reconciled_write_version
                    + tonumber(stored.source_write_version or '0')
            end
        end
        if coverage.schema_identity == ARGV[9]
            and coverage.contract_revision == tonumber(ARGV[10])
            and coverage.key_signature == ARGV[11]
            and coverage.source_write_version == coverage.covered_write_version
            and coverage.covered_write_version == reconciled_write_version
            and matching_contracts == prior_journal_count
            and redis.call('HLEN', KEYS[7]) == prior_journal_count then
            can_advance_coverage = true
            prior_covered_write_version = coverage.covered_write_version
        end
    end
end
for _, row in ipairs(key_rows) do
    local owner = redis.call('HGET', KEYS[8], row.owner_field)
    if owner and owner ~= entity_id then return {-2, current} end
end
if ARGV[6] ~= '' then
    local fence_json = redis.call('GET', KEYS[10])
    if fence_json then
        local fence = cjson.decode(fence_json)
        local event = cjson.decode(ARGV[3])
        if event.event_type == fence.publication_action
            and (not event.metadata.kernel or event.metadata.kernel == cjson.null) then
            return {-1, current}
        end
    end
end
local coverage_json = ''
if can_advance_coverage then
    local final_write_version = prior_covered_write_version + 1
    coverage_json = cjson.encode({
        schema_identity=ARGV[9],
        contract_revision=tonumber(ARGV[10]),
        key_signature=ARGV[11],
        cursor=entity_id,
        source_write_version=final_write_version,
        covered_write_version=final_write_version
    })
end
temper_commit_first_event({
    sequence=KEYS[1], events=KEYS[2], entities=KEYS[3], journals=KEYS[4],
    unscoped=KEYS[5], generation=KEYS[6], contracts=KEYS[7], owners=KEYS[8],
    entity_keys=KEYS[9], segment=KEYS[11], current_segment=KEYS[12],
    coverage=KEYS[13], projections=KEYS[14]
}, {
    sequence=1, events={ARGV[3]}, entity_ref=ARGV[4], journal_member=ARGV[5],
    unscoped_member=ARGV[6], entity_id=entity_id, contract_json=ARGV[2],
    projection_json=ARGV[14], key_rows=key_rows, segment_json=ARGV[8],
    coverage_json=coverage_json, replace_keys=true
})
return {1, 1}
"#;

pub(super) const RECONCILE_CREATION_METADATA_LUA: &str = r#"
local current = tonumber(redis.call('GET', KEYS[1]) or '0')
local expected = tonumber(ARGV[2])
if current ~= expected then return {0, current} end
local first_json = redis.call('LINDEX', KEYS[2], 0)
if not first_json then return {-1, current} end
local first = cjson.decode(first_json)
if first.sequence_nr ~= 1 or first.metadata.event_id ~= ARGV[3] then
    return {-1, current}
end
local entity_id = ARGV[1]
local requested_contract = cjson.decode(ARGV[4])
local stored_json = redis.call('HGET', KEYS[3], entity_id)
if stored_json then
    local stored = cjson.decode(stored_json)
    local requested_fields = {}
    for _, field in ipairs(requested_contract.fields) do
        requested_fields[field.name] = field
    end
    for _, prior in ipairs(stored.fields) do
        local target = requested_fields[prior.name]
        if not target
            or prior.type_descriptor ~= target.type_descriptor
            or prior.value_source ~= target.value_source
            or prior.nullable ~= target.nullable
            or prior.create_required == nil
            or prior.create_required ~= target.create_required
            or prior.default_digest ~= target.default_digest
            or prior.value_digest ~= target.value_digest then
            return {-2, current}
        end
        requested_fields[prior.name] = nil
    end
    for _, added in pairs(requested_fields) do
        if added.create_required or added.default_digest ~= added.value_digest then
            return {-2, current}
        end
    end
    stored.contract_revision = requested_contract.contract_revision
    stored.schema_identity = requested_contract.schema_identity
    stored.declared_key_signature = requested_contract.declared_key_signature
    stored.source_write_version = expected
    requested_contract = stored
end
local key_rows = cjson.decode(ARGV[5])
for _, row in ipairs(key_rows) do
    local owner = redis.call('HGET', KEYS[4], row.owner_field)
    if owner and owner ~= entity_id then return {-3, current} end
end
redis.call('HSET', KEYS[3], entity_id, cjson.encode(requested_contract))
local prior_json = redis.call('HGET', KEYS[5], entity_id)
if prior_json then
    for _, owner_field in ipairs(cjson.decode(prior_json)) do
        if redis.call('HGET', KEYS[4], owner_field) == entity_id then
            redis.call('HDEL', KEYS[4], owner_field)
        end
    end
end
local current_fields = {}
for _, row in ipairs(key_rows) do
    redis.call('HSET', KEYS[4], row.owner_field, entity_id)
    table.insert(current_fields, row.owner_field)
end
redis.call('HSET', KEYS[5], entity_id, cjson.encode(current_fields))
return {1, current}
"#;

pub(super) const PUBLISH_CREATION_COVERAGE_LUA: &str = r#"
local streams = redis.call('ZLEXCOUNT', KEYS[1], ARGV[1], ARGV[2])
local total_contracts = redis.call('HLEN', KEYS[2])
local expected = tonumber(ARGV[3])
local metadata = cjson.decode(ARGV[4])
local contracts = 0
local reconciled_write_version = 0
for _, stored_json in ipairs(redis.call('HVALS', KEYS[2])) do
    local stored = cjson.decode(stored_json)
    if stored.schema_identity == metadata.schema_identity
        and stored.contract_revision == metadata.contract_revision
        and stored.declared_key_signature == metadata.key_signature then
        contracts = contracts + 1
        reconciled_write_version = reconciled_write_version
            + tonumber(stored.source_write_version or '0')
    end
end
if streams ~= total_contracts or contracts ~= streams
    or reconciled_write_version ~= expected then
    return {0, reconciled_write_version}
end
redis.call('SET', KEYS[3], ARGV[4])
return {1, streams}
"#;

#[derive(Serialize)]
struct LuaKeyRow<'a> {
    key_name: &'a str,
    owner_field: String,
}

impl RedisEventStore {
    pub(super) async fn reconcile_creation_metadata_inner(
        &self,
        repair: &temper_runtime::persistence::CreationMetadataRepair,
    ) -> Result<(), PersistenceError> {
        repair.first_event.validate()?;
        let commit = &repair.first_event;
        let contract_json = contract_record_json(
            &commit.contract,
            commit.contract_revision,
            &commit.schema_identity,
            &commit.declared_key_signature,
            repair.source_sequence,
        )?;
        let key_rows = commit
            .key_rows
            .iter()
            .map(|row| LuaKeyRow {
                key_name: &row.key_name,
                owner_field: format!("{}\0{}", row.key_name, row.key_hash),
            })
            .collect::<Vec<_>>();
        let key_rows_json = serde_json::to_string(&key_rows)
            .map_err(|error| PersistenceError::Serialization(error.to_string()))?;
        let keys = vec![
            Self::seq_key(&commit.tenant, &commit.entity_type, &commit.entity_id),
            Self::events_key(&commit.tenant, &commit.entity_type, &commit.entity_id),
            Self::create_or_verify_hash_key(&commit.tenant, &commit.entity_type, "contracts"),
            Self::create_or_verify_hash_key(&commit.tenant, &commit.entity_type, "owners"),
            Self::create_or_verify_hash_key(&commit.tenant, &commit.entity_type, "entity_keys"),
        ];
        let args = vec![
            commit.entity_id.clone(),
            repair.source_sequence.to_string(),
            commit.event.metadata.event_id.to_string(),
            contract_json,
            key_rows_json,
        ];
        let result: Vec<i64> = self
            .reconcile_creation_script
            .evalsha_with_reload(&self.client, keys, args)
            .await
            .map_err(storage_error)?;
        match result.as_slice() {
            [1, _] => Ok(()),
            [0, actual] => Err(PersistenceError::ConcurrencyViolation {
                expected: repair.source_sequence,
                actual: u64::try_from(*actual).map_err(storage_error)?,
            }),
            [-1, _] => Err(PersistenceError::Storage(
                "creation repair sequence-1 event changed".into(),
            )),
            [-2, _] => Err(PersistenceError::Storage(
                "creation repair cannot replace an immutable contract".into(),
            )),
            [-3, _] => Err(PersistenceError::Storage(
                "duplicate declared key during creation repair".into(),
            )),
            other => Err(PersistenceError::Storage(format!(
                "unexpected creation repair Lua result: {other:?}"
            ))),
        }
    }

    pub(super) async fn publish_creation_coverage_inner(
        &self,
        publication: &temper_runtime::persistence::CreationCoveragePublication,
    ) -> Result<(), PersistenceError> {
        let coverage_json = serde_json::to_string(&serde_json::json!({
            "schema_identity": publication.metadata.schema_identity,
            "contract_revision": publication.metadata.contract_revision,
            "key_signature": publication.metadata.declared_key_signature,
            "cursor": publication.cursor,
            "source_write_version": publication.source_write_version,
            "covered_write_version": publication.source_write_version,
        }))
        .map_err(|error| PersistenceError::Serialization(error.to_string()))?;
        let prefix = encode_lex_component(&publication.entity_type);
        let keys = vec![
            Self::tenant_journals_key(&publication.tenant),
            Self::create_or_verify_hash_key(
                &publication.tenant,
                &publication.entity_type,
                "contracts",
            ),
            Self::creation_coverage_key(
                &publication.tenant,
                &publication.entity_type,
                &publication.metadata.schema_identity,
                publication.metadata.contract_revision,
                &publication.metadata.declared_key_signature,
            ),
        ];
        let args = vec![
            format!("[{prefix}!"),
            format!("[{prefix}!\u{10ffff}"),
            publication.source_write_version.to_string(),
            coverage_json,
        ];
        let result: Vec<i64> = self
            .publish_creation_coverage_script
            .evalsha_with_reload(&self.client, keys, args)
            .await
            .map_err(storage_error)?;
        match result.as_slice() {
            [1, _] => Ok(()),
            [0, actual] => Err(PersistenceError::ConcurrencyViolation {
                expected: publication.source_write_version,
                actual: u64::try_from(*actual).map_err(storage_error)?,
            }),
            other => Err(PersistenceError::Storage(format!(
                "unexpected coverage publication Lua result: {other:?}"
            ))),
        }
    }

    pub(super) async fn commit_first_event_inner(
        &self,
        commit: &FirstEventCommit,
    ) -> Result<u64, PersistenceError> {
        commit.validate()?;
        let event_json = serde_json::to_string(&commit.event)
            .map_err(|error| PersistenceError::Serialization(error.to_string()))?;
        let contract_json = contract_record_json(
            &commit.contract,
            commit.contract_revision,
            &commit.schema_identity,
            &commit.declared_key_signature,
            1,
        )?;
        let key_rows = commit
            .key_rows
            .iter()
            .map(|row| LuaKeyRow {
                key_name: &row.key_name,
                owner_field: format!("{}\0{}", row.key_name, row.key_hash),
            })
            .collect::<Vec<_>>();
        let key_rows_json = serde_json::to_string(&key_rows)
            .map_err(|error| PersistenceError::Serialization(error.to_string()))?;
        let entity_ref = serde_json::to_string(&EntityRef {
            entity_type: commit.entity_type.clone(),
            entity_id: commit.entity_id.clone(),
        })
        .map_err(|error| PersistenceError::Serialization(error.to_string()))?;
        let unscoped = split_scoped_journal_entity_id(&commit.entity_id).is_none();
        let segment_json = serde_json::to_string(&SegmentRecord {
            segment_index: 0,
            start_sequence_nr: 1,
            end_sequence_nr: Some(1),
            snapshot_sequence: None,
            event_count: 1,
            sealed_at: None,
            created_at: chrono::Utc::now(),
        })
        .map_err(|error| PersistenceError::Serialization(error.to_string()))?;
        let keys = vec![
            Self::seq_key(&commit.tenant, &commit.entity_type, &commit.entity_id),
            Self::events_key(&commit.tenant, &commit.entity_type, &commit.entity_id),
            Self::tenant_entities_key(&commit.tenant),
            Self::tenant_journals_key(&commit.tenant),
            Self::unscoped_journals_key(&commit.tenant, &commit.entity_type),
            Self::unscoped_generation_key(&commit.tenant, &commit.entity_type),
            Self::create_or_verify_hash_key(&commit.tenant, &commit.entity_type, "contracts"),
            Self::create_or_verify_hash_key(&commit.tenant, &commit.entity_type, "owners"),
            Self::create_or_verify_hash_key(&commit.tenant, &commit.entity_type, "entity_keys"),
            Self::unscoped_fence_key(&commit.tenant, &commit.entity_type),
            Self::segment_key(&commit.tenant, &commit.entity_type, &commit.entity_id, 0),
            Self::current_segment_key(&commit.tenant, &commit.entity_type, &commit.entity_id),
            Self::creation_coverage_key(
                &commit.tenant,
                &commit.entity_type,
                &commit.schema_identity,
                commit.contract_revision,
                &commit.declared_key_signature,
            ),
            Self::create_or_verify_hash_key(&commit.tenant, &commit.entity_type, "projections"),
        ];
        let args = vec![
            commit.entity_id.clone(),
            contract_json,
            event_json,
            entity_ref,
            Self::journal_member(&commit.entity_type, &commit.entity_id),
            if unscoped {
                encode_lex_component(&commit.entity_id)
            } else {
                String::new()
            },
            key_rows_json,
            segment_json,
            commit.schema_identity.clone(),
            commit.contract_revision.to_string(),
            commit.declared_key_signature.clone(),
            format!("[{}!", encode_lex_component(&commit.entity_type)),
            format!("[{}!\u{10ffff}", encode_lex_component(&commit.entity_type)),
            commit
                .projection
                .as_ref()
                .map(serde_json::to_string)
                .transpose()
                .map_err(|error| PersistenceError::Serialization(error.to_string()))?
                .unwrap_or_default(),
        ];
        let result: Vec<i64> = self
            .commit_first_event_script
            .evalsha_with_reload(&self.client, keys, args)
            .await
            .map_err(storage_error)?;
        match result.as_slice() {
            [1, sequence] => u64::try_from(*sequence).map_err(storage_error),
            [0, actual] => Err(PersistenceError::ConcurrencyViolation {
                expected: 0,
                actual: u64::try_from(*actual).map_err(storage_error)?,
            }),
            [-1, _] => Err(PersistenceError::Storage(
                "stream descriptor publication fence".to_string(),
            )),
            [-2, _] => Err(PersistenceError::Storage(
                "duplicate declared key".to_string(),
            )),
            other => Err(PersistenceError::Storage(format!(
                "unexpected first-event Lua result: {other:?}"
            ))),
        }
    }
}
