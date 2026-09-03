//! Shared Lua mutation primitive for every Redis sequence-one commit path.

/// Common mutation function prepended to ordinary-create, create-or-verify,
/// and batch scripts. Callers perform their path-specific validation first and
/// pass only fully resolved values into this function.
pub(super) const LUA: &str = r#"
local function temper_commit_first_event(keys, values)
    for _, event_json in ipairs(values.events) do
        redis.call('RPUSH', keys.events, event_json)
    end
    redis.call('SET', keys.sequence, tostring(values.sequence))
    redis.call('SADD', keys.entities, values.entity_ref)
    redis.call('ZADD', keys.journals, 0, values.journal_member)
    if values.unscoped_member ~= '' then
        redis.call('ZADD', keys.unscoped, 0, values.unscoped_member)
        redis.call('INCRBY', keys.generation, #values.events)
    end
    if values.contract_json ~= '' then
        redis.call('HSET', keys.contracts, values.entity_id, values.contract_json)
    end
    if keys.projections and values.projection_json ~= '' then
        redis.call('HSET', keys.projections, values.entity_id, values.projection_json)
    end
    if values.replace_keys then
        local old_keys_json = redis.call('HGET', keys.entity_keys, values.entity_id)
        if old_keys_json then
            for _, owner_field in ipairs(cjson.decode(old_keys_json)) do
                if redis.call('HGET', keys.owners, owner_field) == values.entity_id then
                    redis.call('HDEL', keys.owners, owner_field)
                end
            end
        end
        local current_fields = {}
        for _, row in ipairs(values.key_rows) do
            redis.call('HSET', keys.owners, row.owner_field, values.entity_id)
            table.insert(current_fields, row.owner_field)
        end
        redis.call('HSET', keys.entity_keys, values.entity_id, cjson.encode(current_fields))
    end
    if keys.segment and values.segment_json ~= '' then
        redis.call('SET', keys.segment, values.segment_json)
        redis.call('SET', keys.current_segment, '0')
    end
    if keys.coverage and values.coverage_json ~= '' then
        redis.call('SET', keys.coverage, values.coverage_json)
    end
end
"#;

pub(super) fn compose(body: &str) -> String {
    format!("{LUA}\n{body}")
}
