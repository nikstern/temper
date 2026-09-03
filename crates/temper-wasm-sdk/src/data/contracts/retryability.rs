use serde::{Deserialize, Serialize};

/// Whether retrying an operation can be useful.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Retryability {
    /// Repeating the same request cannot help.
    Never,
    /// Retry only after refreshing state or a commit token.
    AfterRefresh,
    /// Retry with bounded exponential backoff.
    WithBackoff,
}
