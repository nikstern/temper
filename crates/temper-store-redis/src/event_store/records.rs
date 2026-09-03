//! Redis serialization records shared by event-store submodules.

use serde::{Deserialize, Serialize};
use temper_runtime::persistence::{CreationContract, PersistenceError};

pub(super) fn contract_record_json(
    contract: &CreationContract,
    contract_revision: u32,
    schema_identity: &str,
    declared_key_signature: &str,
    source_write_version: u64,
) -> Result<String, PersistenceError> {
    let mut value = serde_json::to_value(contract)
        .map_err(|error| PersistenceError::Serialization(error.to_string()))?;
    let object = value.as_object_mut().ok_or_else(|| {
        PersistenceError::Serialization("creation contract must encode as an object".into())
    })?;
    object.insert("contract_revision".into(), contract_revision.into());
    object.insert("schema_identity".into(), schema_identity.into());
    object.insert(
        "declared_key_signature".into(),
        declared_key_signature.into(),
    );
    object.insert("source_write_version".into(), source_write_version.into());
    serde_json::to_string(&value)
        .map_err(|error| PersistenceError::Serialization(error.to_string()))
}

#[derive(Debug, Serialize, Deserialize)]
pub(super) struct SnapshotRecord {
    pub(super) sequence_nr: u64,
    pub(super) snapshot: Vec<u8>,
}

#[derive(Debug, Serialize, Deserialize)]
pub(super) struct SnapshotHistoryRecord {
    pub(super) sequence_nr: u64,
    pub(super) snapshot: Vec<u8>,
    pub(super) created_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Serialize, Deserialize)]
pub(super) struct SegmentRecord {
    pub(super) segment_index: u64,
    pub(super) start_sequence_nr: u64,
    pub(super) end_sequence_nr: Option<u64>,
    pub(super) snapshot_sequence: Option<u64>,
    pub(super) event_count: u64,
    pub(super) sealed_at: Option<chrono::DateTime<chrono::Utc>>,
    pub(super) created_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Serialize, Deserialize)]
pub(super) struct EntityRef {
    pub(super) entity_type: String,
    pub(super) entity_id: String,
}
