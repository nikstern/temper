//! Redis scripts for ordinary, indexed, batched, and fenced appends.

/// Lua script for atomic append: check expected sequence, append events, and index the journal.
///
/// KEYS[1] = seq_key, KEYS[2] = events_key, KEYS[3] = entities_key,
/// KEYS[4] = journals_key, KEYS[5] = unscoped index, KEYS[6] = generation,
/// KEYS[7] = publication fence
/// ARGV[1] = expected_seq (string-encoded integer)
/// ARGV[2] = entity_ref_json (for SADD into entities set)
/// ARGV[3] = journal_member (order-preserving encoded type and id)
/// ARGV[4] = unscoped member or empty, ARGV[5..N] = serialized event JSONs
///
/// Returns: `{1, new_seq}` on success, `{0, current_seq}` on conflict.
pub(super) const APPEND_LUA: &str = r#"
local seq_key = KEYS[1]
local events_key = KEYS[2]
local entities_key = KEYS[3]
local journals_key = KEYS[4]
local unscoped_index_key = KEYS[5]
local generation_key = KEYS[6]
local fence_key = KEYS[7]
local expected = tonumber(ARGV[1])
local entity_ref = ARGV[2]
local journal_member = ARGV[3]
local unscoped_member = ARGV[4]

local current = tonumber(redis.call('GET', seq_key) or '0')
if current ~= expected then
    return {0, current}
end

if unscoped_member ~= '' then
    local fence_json = redis.call('GET', fence_key)
    if fence_json then
        local fence = cjson.decode(fence_json)
        for i = 5, #ARGV do
            local event = cjson.decode(ARGV[i])
            if event.event_type == fence.publication_action
                and (not event.metadata.kernel or event.metadata.kernel == cjson.null) then
                return {-1, current}
            end
        end
    end
end

for i = 5, #ARGV do
    redis.call('RPUSH', events_key, ARGV[i])
end

local event_count = #ARGV - 4
local new_seq = expected + event_count
redis.call('SET', seq_key, tostring(new_seq))
redis.call('SADD', entities_key, entity_ref)
redis.call('ZADD', journals_key, 0, journal_member)
if unscoped_member ~= '' then
    redis.call('ZADD', unscoped_index_key, 0, unscoped_member)
    redis.call('INCRBY', generation_key, event_count)
end

return {1, new_seq}
"#;

/// Atomic append with exact-set declared-key ownership maintenance.
pub(super) const APPEND_WITH_KEYS_LUA: &str = r#"
local expected = tonumber(ARGV[1])
local current = tonumber(redis.call('GET', KEYS[1]) or '0')
if current ~= expected then return {0, current} end
local entity_id = ARGV[5]
local key_rows = cjson.decode(ARGV[6])
local can_advance_coverage = false
local prior_covered_write_version = 0
local contract_json = redis.call('HGET', KEYS[10], entity_id)
if contract_json and ARGV[7] ~= '' then
    local contract = cjson.decode(contract_json)
    if contract.schema_identity == ARGV[7]
        and contract.contract_revision == tonumber(ARGV[8])
        and contract.declared_key_signature == ARGV[9] then
        local coverage_json = redis.call('GET', KEYS[11])
        local journal_count = redis.call('ZLEXCOUNT', KEYS[4], ARGV[10], ARGV[11])
        if coverage_json then
            local coverage = cjson.decode(coverage_json)
            local matching_contracts = 0
            local reconciled_write_version = 0
            for _, stored_json in ipairs(redis.call('HVALS', KEYS[10])) do
                local stored = cjson.decode(stored_json)
                if stored.schema_identity == ARGV[7]
                    and stored.contract_revision == tonumber(ARGV[8])
                    and stored.declared_key_signature == ARGV[9] then
                    matching_contracts = matching_contracts + 1
                    reconciled_write_version = reconciled_write_version
                        + tonumber(stored.source_write_version or '0')
                end
            end
            if coverage.schema_identity == ARGV[7]
                and coverage.contract_revision == tonumber(ARGV[8])
                and coverage.key_signature == ARGV[9]
                and coverage.source_write_version == coverage.covered_write_version
                and coverage.covered_write_version == reconciled_write_version
                and matching_contracts == journal_count
                and redis.call('HLEN', KEYS[10]) == journal_count then
                can_advance_coverage = true
                prior_covered_write_version = coverage.covered_write_version
            end
        end
    end
end
for _, row in ipairs(key_rows) do
    local owner = redis.call('HGET', KEYS[8], row.owner_field)
    if owner and owner ~= entity_id then return {-2, current} end
end
if ARGV[4] ~= '' then
    local fence_json = redis.call('GET', KEYS[7])
    if fence_json then
        local fence = cjson.decode(fence_json)
        for i = 12, #ARGV do
            local event = cjson.decode(ARGV[i])
            if event.event_type == fence.publication_action
                and (not event.metadata.kernel or event.metadata.kernel == cjson.null) then
                return {-1, current}
            end
        end
    end
end
for i = 12, #ARGV do redis.call('RPUSH', KEYS[2], ARGV[i]) end
local event_count = #ARGV - 11
local new_seq = expected + event_count
redis.call('SET', KEYS[1], tostring(new_seq))
redis.call('SADD', KEYS[3], ARGV[2])
redis.call('ZADD', KEYS[4], 0, ARGV[3])
if ARGV[4] ~= '' then
    redis.call('ZADD', KEYS[5], 0, ARGV[4])
    redis.call('INCRBY', KEYS[6], event_count)
end
local old_keys_json = redis.call('HGET', KEYS[9], entity_id)
if old_keys_json then
    for _, owner_field in ipairs(cjson.decode(old_keys_json)) do
        if redis.call('HGET', KEYS[8], owner_field) == entity_id then
            redis.call('HDEL', KEYS[8], owner_field)
        end
    end
end
local current_fields = {}
for _, row in ipairs(key_rows) do
    redis.call('HSET', KEYS[8], row.owner_field, entity_id)
    table.insert(current_fields, row.owner_field)
end
redis.call('HSET', KEYS[9], entity_id, cjson.encode(current_fields))
if contract_json then
    local contract = cjson.decode(contract_json)
    contract.source_write_version = new_seq
    redis.call('HSET', KEYS[10], entity_id, cjson.encode(contract))
end
if can_advance_coverage then
    local final_write_version = prior_covered_write_version + event_count
    redis.call('SET', KEYS[11], cjson.encode({
        schema_identity=ARGV[7],
        contract_revision=tonumber(ARGV[8]),
        key_signature=ARGV[9],
        cursor=entity_id,
        source_write_version=final_write_version,
        covered_write_version=final_write_version
    }))
end
return {1, new_seq}
"#;

/// Atomically validate generations and replace an installed application's fences.
pub(super) const ACTIVATE_UNSCOPED_FENCE_LUA: &str = r#"
local current_pointer = redis.call('GET', KEYS[1]) or ''
if current_pointer ~= ARGV[1] then
    return {-2}
end
local binding_count = tonumber(ARGV[2])
for i = 1, binding_count do
    local expected = tonumber(ARGV[2 + (i - 1) * 2 + 1])
    if expected >= 0 then
        local actual = tonumber(redis.call('GET', KEYS[2 + (i - 1) * 2]) or '0')
        if actual ~= expected then
            return {0, i, actual}
        end
    end
end
for i = 1, binding_count do
    local fence_json = ARGV[2 + (i - 1) * 2 + 2]
    local fence_key = KEYS[3 + (i - 1) * 2]
    if fence_json == '' then
        redis.call('DEL', fence_key)
    else
        redis.call('SET', fence_key, fence_json)
    end
end
local new_pointer = ARGV[3 + binding_count * 2]
if new_pointer == '' then
    redis.call('DEL', KEYS[1])
else
    redis.call('SET', KEYS[1], new_pointer)
end
return {1}
"#;

/// Lua script for one atomic, same-tenant multi-journal append.
///
/// Keys are `(sequence, events)` pairs followed by the tenant entity and journal
/// indexes. Arguments are the append count followed by, for each append,
/// `(expected sequence, entity ref, journal member, event count, events...)`.
/// Every optimistic fence is checked before any key is mutated.
pub(super) const APPEND_BATCH_LUA: &str = r#"
local append_count = tonumber(ARGV[1])
local arg_index = 2
local key_index = 1
local batch_claims = {}

for append_index = 1, append_count do
    local expected = tonumber(ARGV[arg_index])
    local unscoped_member = ARGV[arg_index + 3]
    local event_count = tonumber(ARGV[arg_index + 4])
    local entity_id = ARGV[arg_index + 5]
    local key_rows = cjson.decode(ARGV[arg_index + 6])
    local first_metadata = ARGV[arg_index + 7]
    local coverage_metadata = ARGV[arg_index + 8]
    local current = tonumber(redis.call('GET', KEYS[key_index]) or '0')
    if current ~= expected then
        return {0, append_index, current}
    end
    if unscoped_member ~= '' then
        local fence_json = redis.call('GET', KEYS[key_index + 4])
        if fence_json then
            local fence = cjson.decode(fence_json)
            for event_offset = 1, event_count do
                local event = cjson.decode(ARGV[arg_index + 8 + event_offset])
                if event.event_type == fence.publication_action
                    and (not event.metadata.kernel or event.metadata.kernel == cjson.null) then
                    return {-1, append_index, current}
                end
            end
        end
    end
    if first_metadata ~= '' and (expected ~= 0 or event_count == 0) then
        return {-3, append_index, current}
    end
    for _, row in ipairs(key_rows) do
        local owner = redis.call('HGET', KEYS[key_index + 5], row.owner_field)
        if owner and owner ~= entity_id then
            return {-2, append_index, current}
        end
        local batch_owner = batch_claims[row.owner_field]
        if batch_owner and batch_owner ~= entity_id then
            return {-2, append_index, current}
        end
        batch_claims[row.owner_field] = entity_id
    end
    arg_index = arg_index + 9 + event_count
    key_index = key_index + 9
end

local entities_key = KEYS[append_count * 9 + 1]
local journals_key = KEYS[append_count * 9 + 2]
local result = {1}
arg_index = 2
key_index = 1

for append_index = 1, append_count do
    local expected = tonumber(ARGV[arg_index])
    local entity_ref = ARGV[arg_index + 1]
    local journal_member = ARGV[arg_index + 2]
    local unscoped_member = ARGV[arg_index + 3]
    local event_count = tonumber(ARGV[arg_index + 4])
    local entity_id = ARGV[arg_index + 5]
    local key_rows = cjson.decode(ARGV[arg_index + 6])
    local first_metadata = ARGV[arg_index + 7]
    local coverage_metadata_json = ARGV[arg_index + 8]
    local can_advance_coverage = false
    local prior_covered_write_version = 0
    local coverage_metadata = nil
    if coverage_metadata_json ~= '' then
        coverage_metadata = cjson.decode(coverage_metadata_json)
        local prior_journal_count = redis.call(
            'ZLEXCOUNT', journals_key,
            coverage_metadata.journal_lower, coverage_metadata.journal_upper)
        if prior_journal_count == 0 then
            can_advance_coverage = true
        else
            local coverage_json = redis.call('GET', KEYS[key_index + 8])
            if coverage_json then
                local coverage = cjson.decode(coverage_json)
                local matching_contracts = 0
                local reconciled_write_version = 0
                for _, stored_json in ipairs(redis.call('HVALS', KEYS[key_index + 7])) do
                    local stored = cjson.decode(stored_json)
                    if stored.schema_identity == coverage_metadata.schema_identity
                        and stored.contract_revision == coverage_metadata.contract_revision
                        and stored.declared_key_signature
                            == coverage_metadata.declared_key_signature then
                        matching_contracts = matching_contracts + 1
                        reconciled_write_version = reconciled_write_version
                            + tonumber(stored.source_write_version or '0')
                    end
                end
                if coverage.schema_identity == coverage_metadata.schema_identity
                    and coverage.contract_revision == coverage_metadata.contract_revision
                    and coverage.key_signature == coverage_metadata.declared_key_signature
                    and coverage.source_write_version == coverage.covered_write_version
                    and coverage.covered_write_version == reconciled_write_version
                    and matching_contracts == prior_journal_count
                    and redis.call('HLEN', KEYS[key_index + 7]) == prior_journal_count then
                    can_advance_coverage = true
                    prior_covered_write_version = coverage.covered_write_version
                end
            end
        end
    end
    local event_jsons = {}
    for event_offset = 1, event_count do
        table.insert(event_jsons, ARGV[arg_index + 8 + event_offset])
    end
    local new_seq = expected + event_count
    local contract_json = ''
    local stored_contract_json = redis.call('HGET', KEYS[key_index + 7], entity_id)
    if stored_contract_json then
        local stored_contract = cjson.decode(stored_contract_json)
        stored_contract.source_write_version = new_seq
        contract_json = cjson.encode(stored_contract)
    end
    if first_metadata ~= '' then
        local metadata = cjson.decode(first_metadata)
        metadata.contract.contract_revision = metadata.contract_revision
        metadata.contract.schema_identity = metadata.schema_identity
        metadata.contract.declared_key_signature = metadata.declared_key_signature
        metadata.contract.source_write_version = new_seq
        contract_json = cjson.encode(metadata.contract)
    end
    local coverage_json = ''
    if can_advance_coverage and coverage_metadata then
        local final_write_version = prior_covered_write_version + event_count
        coverage_json = cjson.encode({
            schema_identity=coverage_metadata.schema_identity,
            contract_revision=coverage_metadata.contract_revision,
            key_signature=coverage_metadata.declared_key_signature,
            cursor=entity_id,
            source_write_version=final_write_version,
            covered_write_version=final_write_version
        })
    end
    temper_commit_first_event({
        sequence=KEYS[key_index], events=KEYS[key_index + 1], entities=entities_key,
        journals=journals_key, unscoped=KEYS[key_index + 2],
        generation=KEYS[key_index + 3], contracts=KEYS[key_index + 7],
        owners=KEYS[key_index + 5], entity_keys=KEYS[key_index + 6],
        coverage=KEYS[key_index + 8]
    }, {
        sequence=new_seq, events=event_jsons, entity_ref=entity_ref,
        journal_member=journal_member, unscoped_member=unscoped_member,
        entity_id=entity_id, contract_json=contract_json, projection_json='',
        key_rows=key_rows, segment_json='', coverage_json=coverage_json,
        replace_keys=expected == 0
    })
    table.insert(result, new_seq)
    arg_index = arg_index + 9 + event_count
    key_index = key_index + 9
end

return result
"#;

/// Atomically advances the bounded historical-journal index backfill.
pub(super) const BACKFILL_UNSCOPED_INDEX_LUA: &str = r#"
local current = redis.call('GET', KEYS[1]) or ''
if current ~= ARGV[1] then
    return {0}
end
for i = 4, #ARGV do
    redis.call('ZADD', KEYS[2], 0, ARGV[i])
end
redis.call('SET', KEYS[1], ARGV[2])
if ARGV[3] == '1' then
    redis.call('SET', KEYS[3], '1')
end
return {1}
"#;
