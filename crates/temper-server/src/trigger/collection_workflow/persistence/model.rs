use temper_runtime::persistence::PersistenceAppendResult;

/// Reserved source-event field containing normalized collection starts.
pub(crate) const COLLECTION_START_INTENTS_FIELD: &str = "_temper_collection_starts_v1";
/// Reserved source-event field containing normalized collection controls.
pub(crate) const COLLECTION_CONTROL_INTENTS_FIELD: &str = "_temper_collection_controls_v1";
/// Reserved replay field retaining the active workflow identity.
pub(crate) const ACTIVE_COLLECTION_WORKFLOW_FIELD: &str = "_temper_active_collection_workflow_v1";
/// Private synthetic entity type used for one workflow journal.
pub(crate) const COLLECTION_WORKFLOW_ENTITY_TYPE: &str = "_CollectionWorkflow";
/// Maximum private workflow snapshots inspected to recover one owned intent.
pub(super) const MAX_COLLECTION_WORKFLOW_EVENTS: usize = 1_024;

/// Outcome of an atomic commit whose prior result may have been ambiguous.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CollectionLedgerCommitOutcome {
    Committed(Vec<PersistenceAppendResult>),
    Reconciled(Vec<PersistenceAppendResult>),
}
