use temper_wasm_sdk::data::{ModuleDataError, ModuleDataErrorKind};

use crate::entity_actor::EntityEvent;
use crate::events::EntityStateChange;
use crate::storage::BoxedEventStore;

use super::super::{
    ApplicationDataInvocation, not_applied_error, not_applied_internal_error, short_type,
};

impl ApplicationDataInvocation {
    pub(super) async fn recover_creation_notification(
        &self,
        entity_type: &str,
        winning_journal_id: &str,
        entity_id: &str,
        store: &BoxedEventStore,
    ) -> Result<(), ModuleDataError> {
        const CREATION_SEQUENCE: u64 = 1;
        let runtime_type = short_type(entity_type);
        let persistence_id = format!(
            "{}:{runtime_type}:{winning_journal_id}",
            self.authority.tenant
        );
        let creation_event = store
            .read_events(&persistence_id, 0)
            .await
            .map_err(|error| not_applied_internal_error(error.to_string()))?
            .into_iter()
            .find(|event| event.sequence_nr == CREATION_SEQUENCE)
            .ok_or_else(|| {
                not_applied_error(
                    ModuleDataErrorKind::ConsistencyUnavailable,
                    "CreationNotificationUnavailable",
                    "the committed creation event is unavailable for notification recovery",
                )
            })?;
        let committed_created: EntityEvent = serde_json::from_value(creation_event.payload)
            .map_err(|error| not_applied_internal_error(error.to_string()))?;
        let intents = self
            .state
            .materialize_committed_reaction_intents(
                &self.authority.tenant,
                runtime_type,
                entity_id,
                CREATION_SEQUENCE,
                self.authority.target.schema_pin(),
            )
            .await
            .map_err(|error| not_applied_internal_error(error.to_string()))?;
        if !intents.is_empty()
            && let Some(dispatcher) = self
                .state
                .reaction_dispatcher
                .read()
                .map_err(|_| {
                    not_applied_internal_error("reaction dispatcher lock poisoned".to_string())
                })?
                .clone()
        {
            dispatcher.notify_recovery(&self.authority.tenant);
        }
        let change = EntityStateChange {
            seq: CREATION_SEQUENCE,
            entity_type: runtime_type.to_string(),
            entity_id: entity_id.to_string(),
            action: "Created".to_string(),
            status: committed_created.to_status,
            tenant: self.authority.tenant.to_string(),
            agent_id: None,
            session_id: None,
            intent: None,
            observation_metadata: None,
        };
        let newly_recorded = self.state.record_entity_observe_event_with_seq(
            self.authority.tenant.as_str(),
            runtime_type,
            entity_id,
            CREATION_SEQUENCE,
            "state_change",
            serde_json::to_value(&change).unwrap_or_default(),
        );
        if newly_recorded {
            let _ = self.state.event_tx.send(change);
        }
        Ok(())
    }
}
