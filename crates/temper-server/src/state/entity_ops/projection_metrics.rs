use std::time::Instant;

use temper_runtime::tenant::TenantId;

pub(super) fn record_projection_update_started(
    tenant: &TenantId,
    entity_type: &str,
    operation: &str,
    source: &str,
) {
    crate::query_projection_metrics::record_update_started(
        tenant.as_str(),
        entity_type,
        operation,
        source,
    );
}

pub(super) fn record_projection_update_success(
    tenant: &TenantId,
    entity_type: &str,
    operation: &str,
    source: &str,
    sequence_nr: u64,
    started_at: Instant,
) {
    let duration = started_at.elapsed();
    crate::query_projection_metrics::record_update_duration(
        tenant.as_str(),
        entity_type,
        operation,
        source,
        "ok",
        duration,
    );
    crate::query_projection_metrics::record_update_end_to_end_duration(
        tenant.as_str(),
        entity_type,
        operation,
        source,
        "ok",
        duration,
    );
    crate::query_projection_metrics::record_update_applied_sequence(
        tenant.as_str(),
        entity_type,
        operation,
        source,
        sequence_nr,
    );
}

pub(super) fn record_projection_update_error(
    tenant: &TenantId,
    entity_type: &str,
    operation: &str,
    source: &str,
    started_at: Instant,
) {
    let duration = started_at.elapsed();
    crate::query_projection_metrics::record_update_duration(
        tenant.as_str(),
        entity_type,
        operation,
        source,
        "error",
        duration,
    );
    crate::query_projection_metrics::record_update_end_to_end_duration(
        tenant.as_str(),
        entity_type,
        operation,
        source,
        "error",
        duration,
    );
    crate::query_projection_metrics::record_update_error(
        tenant.as_str(),
        entity_type,
        operation,
        source,
    );
}
