use serde::{Deserialize, Serialize};

/// Per-entity post-commit consistency token.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CommitToken {
    /// Fully qualified entity type.
    pub entity_type: String,
    /// Canonical entity identifier.
    pub entity_id: String,
    /// Durable entity stream sequence.
    pub sequence: u64,
}
