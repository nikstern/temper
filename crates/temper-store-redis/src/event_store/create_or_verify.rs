//! Redis atomic create-or-verify transaction.

use fred::interfaces::HashesInterface;
use serde::Serialize;
use temper_runtime::persistence::schema_deployment::split_scoped_journal_entity_id;
use temper_runtime::persistence::{
    CreateOrVerifyRequest, CreateOrVerifyStoreOutcome, PersistenceError, storage_error,
};

use super::{EntityRef, RedisEventStore, SegmentRecord, contract_record_json};
use crate::keys::encode_lex_component;

pub(super) const CREATE_OR_VERIFY_LUA: &str = r#"
local requested_id = ARGV[1]
local requested_contract = cjson.decode(ARGV[2])
local requested_digest = ARGV[3]
local idempotency_key = ARGV[4]
local event_json = ARGV[5]
local entity_ref = ARGV[6]
local journal_member = ARGV[7]
local unscoped_member = ARGV[8]
local key_rows = cjson.decode(ARGV[9])
local segment_json = ARGV[10]
local schema_identity = ARGV[11]
local contract_revision = tonumber(ARGV[12])
local key_signature = ARGV[13]
local journal_lower = ARGV[14]
local journal_upper = ARGV[15]
local sequence_prefix = ARGV[16]
local projection_json = ARGV[17]

local journal_count = redis.call('ZLEXCOUNT', KEYS[4], journal_lower, journal_upper)
local prior_covered_write_version = 0
if journal_count > 0 then
    local coverage_json = redis.call('GET', KEYS[14])
    if not coverage_json then return {'migration_required'} end
    local coverage = cjson.decode(coverage_json)
    local matching_contracts = 0
    local reconciled_write_version = 0
    for _, stored_json in ipairs(redis.call('HVALS', KEYS[7])) do
        local stored = cjson.decode(stored_json)
        if stored.schema_identity == schema_identity
            and stored.contract_revision == contract_revision
            and stored.declared_key_signature == key_signature then
            matching_contracts = matching_contracts + 1
            reconciled_write_version = reconciled_write_version
                + tonumber(stored.source_write_version or '0')
        end
    end
    if coverage.schema_identity ~= schema_identity
        or coverage.contract_revision ~= contract_revision
        or coverage.key_signature ~= key_signature
        or coverage.source_write_version ~= coverage.covered_write_version
        or coverage.covered_write_version ~= reconciled_write_version
        or matching_contracts ~= journal_count
        or redis.call('HLEN', KEYS[7]) ~= journal_count then
        return {'migration_required'}
    end
    prior_covered_write_version = coverage.covered_write_version
end

local function conflict(fields)
    table.sort(fields)
    local unique = {}
    local bounded = {}
    local total = 0
    for _, field in ipairs(fields) do
        if not unique[field] then
            unique[field] = true
            total = total + 1
            if #bounded < 32 then table.insert(bounded, field) end
        end
    end
    return {'conflict', cjson.encode(bounded), total > 32 and '1' or '0'}
end

local function compare_contracts(stored, requested, alternate_owner)
    if stored.version ~= 1 or requested.version ~= 1 then
        return {'migration_required'}
    end
    for _, field in ipairs(stored.fields) do
        if field.create_required == nil then return {'migration_required'} end
    end
    for _, field in ipairs(requested.fields) do
        if field.create_required == nil then return {'migration_required'} end
    end
    if stored.digest == requested.digest then
        return {'match'}
    end
    local previous = {}
    for _, field in ipairs(stored.fields) do previous[field.name] = field end
    local fields = {}
    for _, target in ipairs(requested.fields) do
        local prior = previous[target.name]
        if not prior then
            if target.create_required == nil or target.create_required then
                return {'migration_required'}
            end
            if target.value_digest ~= target.default_digest then
                table.insert(fields, target.name)
            end
        elseif prior.type_descriptor ~= target.type_descriptor
            or prior.value_source ~= target.value_source then
            return {'migration_required'}
        elseif prior.nullable ~= target.nullable
            or prior.create_required == nil
            or prior.create_required ~= target.create_required
            or prior.default_digest ~= target.default_digest
            or (not (alternate_owner and target.value_source == 'entity_id')
                and prior.value_digest ~= target.value_digest) then
            table.insert(fields, target.name)
        end
    end
    if #fields == 0 then return {'match'} end
    return conflict(fields)
end

local function compare(owner, alternate_owner)
    local stored_json = redis.call('HGET', KEYS[7], owner)
    if not stored_json then return {'migration_required'} end
    return compare_contracts(cjson.decode(stored_json), requested_contract, alternate_owner)
end

local function creation_sequence(owner)
    if tonumber(redis.call('GET', sequence_prefix .. owner) or '0') < 1 then
        return nil
    end
    return '1'
end

local replay_json = redis.call('HGET', KEYS[8], idempotency_key)
if replay_json then
    local replay = cjson.decode(replay_json)
    if not replay.requested_id or replay.requested_id ~= requested_id then
        return conflict({'Id'})
    end
    if not replay.requested_contract then return {'migration_required'} end
    local result = compare_contracts(replay.requested_contract, requested_contract, false)
    if result[1] ~= 'match' then return result end
    result = compare(replay.entity_id, replay.entity_id ~= replay.requested_id)
    if result[1] ~= 'match' then return result end
    local creation_seq = creation_sequence(replay.entity_id)
    if not creation_seq then return {'migration_required'} end
    return {'already_matches', replay.entity_id, creation_seq,
        replay.notification_pending and '1' or '0'}
end

local owners = {}
local owner_fields = {}
local function add_owner(owner, field)
    owners[owner] = true
    if not owner_fields[owner] then owner_fields[owner] = {} end
    table.insert(owner_fields[owner], field)
end
if tonumber(redis.call('GET', KEYS[1]) or '0') > 0 then
    add_owner(requested_id, 'Id')
end
for _, row in ipairs(key_rows) do
    local owner = redis.call('HGET', KEYS[9], row.owner_field)
    if owner then add_owner(owner, row.key_name) end
end
local owner_count = 0
local winning_owner = nil
for owner, _ in pairs(owners) do
    owner_count = owner_count + 1
    winning_owner = owner
end
if owner_count > 1 then
    local fields = {}
    for _, names in pairs(owner_fields) do
        for _, name in ipairs(names) do table.insert(fields, name) end
    end
    return conflict(fields)
end
if winning_owner then
    local alternate_owner = true
    for _, name in ipairs(owner_fields[winning_owner]) do
        if name == 'Id' then alternate_owner = false end
    end
    local result = compare(winning_owner, alternate_owner)
    if result[1] ~= 'match' then return result end
    redis.call('HSET', KEYS[8], idempotency_key,
        cjson.encode({
            entity_id=winning_owner,
            requested_id=requested_id,
            requested_contract=requested_contract,
            digest=requested_digest,
            notification_pending=false
        }))
    local creation_seq = creation_sequence(winning_owner)
    if not creation_seq then return {'migration_required'} end
    return {'already_matches', winning_owner, creation_seq, '0'}
end

if unscoped_member ~= '' then
    local fence_json = redis.call('GET', KEYS[11])
    if fence_json then
        local fence = cjson.decode(fence_json)
        local event = cjson.decode(event_json)
        if event.event_type == fence.publication_action
            and (not event.metadata.kernel or event.metadata.kernel == cjson.null) then
            return {'publication_fenced'}
        end
    end
end

local covered_count = 0
for _, stored_json in ipairs(redis.call('HVALS', KEYS[7])) do
    local stored = cjson.decode(stored_json)
    if stored.schema_identity == schema_identity
        and stored.contract_revision == contract_revision
        and stored.declared_key_signature == key_signature then
        covered_count = covered_count + 1
    end
end
local final_journal_count = redis.call('ZLEXCOUNT', KEYS[4], journal_lower, journal_upper)
local coverage_json = ''
if covered_count == final_journal_count then
    local final_write_version = prior_covered_write_version + 1
    coverage_json = cjson.encode({
        schema_identity=schema_identity,
        contract_revision=contract_revision,
        key_signature=key_signature,
        cursor=requested_id,
        source_write_version=final_write_version,
        covered_write_version=final_write_version
    })
end
temper_commit_first_event({
    sequence=KEYS[1], events=KEYS[2], entities=KEYS[3], journals=KEYS[4],
    unscoped=KEYS[5], generation=KEYS[6], contracts=KEYS[7], owners=KEYS[9],
    entity_keys=KEYS[10], segment=KEYS[12], current_segment=KEYS[13],
    coverage=KEYS[14], projections=KEYS[15]
}, {
    sequence=1, events={event_json}, entity_ref=entity_ref, journal_member=journal_member,
    unscoped_member=unscoped_member, entity_id=requested_id, contract_json=ARGV[2],
    projection_json=projection_json, key_rows=key_rows, segment_json=segment_json,
    coverage_json=coverage_json, replace_keys=true
})
redis.call('HSET', KEYS[8], idempotency_key,
    cjson.encode({
        entity_id=requested_id,
        requested_id=requested_id,
        requested_contract=requested_contract,
        digest=requested_digest,
        notification_pending=true
    }))
return {'created', requested_id, '1'}
"#;

impl RedisEventStore {
    pub(super) async fn create_or_verify_inner(
        &self,
        request: &CreateOrVerifyRequest,
    ) -> Result<CreateOrVerifyStoreOutcome, PersistenceError> {
        request.first_event.validate()?;
        #[derive(Serialize)]
        struct LuaKeyRow<'a> {
            key_name: &'a str,
            owner_field: String,
        }

        let mut event = request.event.clone();
        event.sequence_nr = 1;
        let event_json = serde_json::to_string(&event)
            .map_err(|error| PersistenceError::Serialization(error.to_string()))?;
        let contract_json = contract_record_json(
            &request.contract,
            request.contract_revision,
            &request.schema_identity,
            &request.declared_key_signature,
            1,
        )?;
        let key_rows = request
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
            entity_type: request.entity_type.clone(),
            entity_id: request.entity_id.clone(),
        })
        .map_err(|error| PersistenceError::Serialization(error.to_string()))?;
        let unscoped = split_scoped_journal_entity_id(&request.entity_id).is_none();
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
            Self::seq_key(&request.tenant, &request.entity_type, &request.entity_id),
            Self::events_key(&request.tenant, &request.entity_type, &request.entity_id),
            Self::tenant_entities_key(&request.tenant),
            Self::tenant_journals_key(&request.tenant),
            Self::unscoped_journals_key(&request.tenant, &request.entity_type),
            Self::unscoped_generation_key(&request.tenant, &request.entity_type),
            Self::create_or_verify_hash_key(&request.tenant, &request.entity_type, "contracts"),
            Self::create_or_verify_hash_key(
                &request.tenant,
                &format!("{}:{}", request.module_name, request.entity_type),
                "idempotency",
            ),
            Self::create_or_verify_hash_key(&request.tenant, &request.entity_type, "owners"),
            Self::create_or_verify_hash_key(&request.tenant, &request.entity_type, "entity_keys"),
            Self::unscoped_fence_key(&request.tenant, &request.entity_type),
            Self::segment_key(&request.tenant, &request.entity_type, &request.entity_id, 0),
            Self::current_segment_key(&request.tenant, &request.entity_type, &request.entity_id),
            Self::creation_coverage_key(
                &request.tenant,
                &request.entity_type,
                &request.schema_identity,
                request.contract_revision,
                &request.declared_key_signature,
            ),
            Self::create_or_verify_hash_key(&request.tenant, &request.entity_type, "projections"),
        ];
        let args = vec![
            request.entity_id.clone(),
            contract_json,
            request.contract.digest.clone(),
            request.idempotency_key.clone(),
            event_json,
            entity_ref,
            Self::journal_member(&request.entity_type, &request.entity_id),
            if unscoped {
                encode_lex_component(&request.entity_id)
            } else {
                String::new()
            },
            key_rows_json,
            segment_json,
            request.schema_identity.clone(),
            request.contract_revision.to_string(),
            request.declared_key_signature.clone(),
            format!("[{}!", encode_lex_component(&request.entity_type)),
            format!("[{}!\u{10ffff}", encode_lex_component(&request.entity_type)),
            format!(
                "{}:{}:events_seq:{}:{}:",
                crate::keys::PREFIX,
                Self::tenant_hash_tag(&request.tenant),
                request.tenant,
                request.entity_type
            ),
            request
                .projection
                .as_ref()
                .map(serde_json::to_string)
                .transpose()
                .map_err(|error| PersistenceError::Serialization(error.to_string()))?
                .unwrap_or_default(),
        ];
        let result: Vec<String> = self
            .create_or_verify_script
            .evalsha_with_reload(&self.client, keys, args)
            .await
            .map_err(storage_error)?;
        match result.as_slice() {
            [status, entity_id, sequence] if status == "created" => {
                Ok(CreateOrVerifyStoreOutcome::Created {
                    entity_id: entity_id.clone(),
                    sequence_nr: sequence.parse().map_err(storage_error)?,
                })
            }
            [status, entity_id, sequence, pending] if status == "already_matches" => {
                Ok(CreateOrVerifyStoreOutcome::AlreadyMatches {
                    entity_id: entity_id.clone(),
                    sequence_nr: sequence.parse().map_err(storage_error)?,
                    notification_pending: pending == "1",
                })
            }
            [status, fields, truncated] if status == "conflict" => {
                Ok(CreateOrVerifyStoreOutcome::Conflict {
                    fields: serde_json::from_str(fields)
                        .map_err(|error| PersistenceError::Serialization(error.to_string()))?,
                    truncated: truncated == "1",
                })
            }
            [status] if status == "migration_required" => {
                Ok(CreateOrVerifyStoreOutcome::CreationContractMigrationRequired)
            }
            [status] if status == "publication_fenced" => Err(PersistenceError::Storage(
                "stream descriptor publication fence".to_string(),
            )),
            other => Err(PersistenceError::Storage(format!(
                "unexpected create-or-verify Lua result: {other:?}"
            ))),
        }
    }

    pub(super) async fn acknowledge_notification_inner(
        &self,
        request: &CreateOrVerifyRequest,
    ) -> Result<(), PersistenceError> {
        let key = Self::create_or_verify_hash_key(
            &request.tenant,
            &format!("{}:{}", request.module_name, request.entity_type),
            "idempotency",
        );
        let Some(encoded) = self
            .client
            .hget::<Option<String>, _, _>(&key, &request.idempotency_key)
            .await
            .map_err(storage_error)?
        else {
            return Err(PersistenceError::Storage(
                "create-or-verify notification acknowledgement lost its request".into(),
            ));
        };
        let mut record: serde_json::Value = serde_json::from_str(&encoded)
            .map_err(|error| PersistenceError::Serialization(error.to_string()))?;
        if record
            .get("requested_id")
            .and_then(serde_json::Value::as_str)
            != Some(request.entity_id.as_str())
        {
            return Err(PersistenceError::Storage(
                "create-or-verify notification acknowledgement request mismatch".into(),
            ));
        }
        record["notification_pending"] = serde_json::Value::Bool(false);
        let encoded = serde_json::to_string(&record)
            .map_err(|error| PersistenceError::Serialization(error.to_string()))?;
        let _: i64 = self
            .client
            .hset(&key, (&request.idempotency_key, encoded))
            .await
            .map_err(storage_error)?;
        Ok(())
    }
}
