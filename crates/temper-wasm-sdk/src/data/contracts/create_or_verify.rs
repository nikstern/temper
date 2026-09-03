use serde::{Deserialize, Serialize};

use super::super::{CommitToken, DataObject};

/// Wire-level atomic create-or-verify outcome.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum CreateOrVerifyResultV1 {
    /// This invocation committed the sequence-1 creation event.
    Created {
        /// Durable commit for the authoritative entity.
        commit: CommitToken,
        /// Canonical authoritative entity value.
        value: DataObject,
    },
    /// A prior creation has the same canonical creation contract.
    AlreadyMatches {
        /// Durable commit for the authoritative entity, which may use another ID.
        commit: CommitToken,
        /// Canonical authoritative entity value.
        value: DataObject,
    },
    /// An identity, declared-key owner, or creation field does not match.
    Conflict {
        /// Sorted, bounded canonical schema-owned field identifiers.
        fields: Vec<String>,
        /// Whether additional conflicting fields were withheld by the disclosure budget.
        truncated: bool,
    },
}
