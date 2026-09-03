use std::collections::VecDeque;

use crate::entity_actor::EntityEvent;

/// Format entity events into the history API response shape.
pub(super) fn format_history_response(
    entity_type: &str,
    entity_id: &str,
    events: &VecDeque<EntityEvent>,
) -> serde_json::Value {
    let formatted: Vec<serde_json::Value> = events
        .iter()
        .enumerate()
        .map(|(index, event)| {
            serde_json::json!({
                "sequence": index + 1,
                "action": event.action,
                "from_state": event.from_status,
                "to_state": event.to_status,
                "timestamp": event.timestamp,
                "params": event.params,
            })
        })
        .collect();

    serde_json::json!({
        "entity_type": entity_type,
        "entity_id": entity_id,
        "events": formatted,
    })
}
