use super::*;

pub(super) struct MigratedTimeoutContext<'a> {
    pub(super) tenant: &'a str,
    pub(super) entity_type: &'a str,
    pub(super) entity_id: &'a str,
    pub(super) source_sequence: u64,
    pub(super) event: &'a EntityEvent,
    pub(super) source_fields: &'a serde_json::Value,
    pub(super) table: &'a temper_jit::table::TransitionTable,
    pub(super) schema_pin: temper_runtime::persistence::schema_deployment::SchemaEventPin,
}

pub(super) fn attach_migrated_state_timeout_intents(
    payload: &mut serde_json::Value,
    context: MigratedTimeoutContext<'_>,
) -> Result<(), ServiceError> {
    let intents = crate::trigger::delivery::state_timeout_intents(
        crate::trigger::delivery::StateTimeoutIntentContext {
            tenant: context.tenant,
            entity_type: context.entity_type,
            entity_id: context.entity_id,
            source_sequence: context.source_sequence,
            event: context.event,
            source_fields: context.source_fields,
            table: context.table,
            schema_pin: Some(context.schema_pin),
            triggering_authority: None,
            durable_idempotency_evidence: &BTreeMap::new(),
        },
    )
    .map_err(|error| ServiceError::new("migration_rejected", error, false))?;
    if !intents.is_empty() {
        crate::trigger::delivery::attach_intents(payload, &intents)
            .map_err(|error| ServiceError::new("migration_rejected", error, false))?;
    }
    Ok(())
}
