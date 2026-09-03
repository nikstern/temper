//! Resumable creation-contract and exact-key reconciliation requests.

use serde::{Deserialize, Serialize};

use super::{FirstEventCommit, FirstEventMetadata};

/// One legacy stream repair derived from its immutable sequence-1 event and
/// authoritative state at `source_sequence`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreationMetadataRepair {
    /// Shared first-event metadata and exact current declared-key rows.
    pub first_event: FirstEventCommit,
    /// Latest stream sequence used to derive the exact key set.
    pub source_sequence: u64,
}

/// Stable full-pass proof published only after every stream was reconciled.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreationCoveragePublication {
    /// Tenant covered by the pass.
    pub tenant: String,
    /// Runtime entity type covered by the pass.
    pub entity_type: String,
    /// Contract/key schema covered by the pass.
    pub metadata: FirstEventMetadata,
    /// Last entity ID visited in deterministic order.
    pub cursor: String,
    /// Authoritative stream count observed before and after the pass.
    pub source_write_version: u64,
}
