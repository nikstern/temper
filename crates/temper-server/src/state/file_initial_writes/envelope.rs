use temper_runtime::persistence::{EventMetadata, KernelEventMetadata, PersistenceEnvelope};
use temper_runtime::scheduler::sim_uuid;

use crate::entity_actor::EntityEvent;

use super::FileStreamContentError;

/// Encode a synthetic File transition with its exact durable sequence and metadata.
pub(super) fn synthetic_envelope(
    persistence_id: &str,
    sequence_nr: u64,
    event: &EntityEvent,
    kernel_metadata: Option<&KernelEventMetadata>,
) -> Result<PersistenceEnvelope, FileStreamContentError> {
    let payload = serde_json::to_value(event).map_err(|error| {
        FileStreamContentError::State(format!("failed to serialize event: {error}"))
    })?;
    Ok(PersistenceEnvelope {
        sequence_nr,
        event_type: event.action.clone(),
        payload,
        metadata: EventMetadata {
            event_id: sim_uuid(),
            causation_id: sim_uuid(),
            correlation_id: sim_uuid(),
            timestamp: event.timestamp,
            actor_id: persistence_id.to_string(),
            kernel: kernel_metadata.cloned(),
        },
    })
}
