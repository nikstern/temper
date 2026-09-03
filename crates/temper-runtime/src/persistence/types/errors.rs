/// Errors that can occur during event persistence operations.
#[derive(Debug, thiserror::Error)]
pub enum PersistenceError {
    /// The store rejected an operation before making any durable change.
    #[error("pre-commit storage error: {0}")]
    PreCommit(String),
    /// A durable commit is known to have occurred before a later store failure.
    #[error("post-commit storage error: {0}")]
    PostCommit(String),
    /// The store lost the commit acknowledgement and cannot prove the outcome.
    #[error("commit acknowledgement unknown: {0}")]
    AcknowledgementUnknown(String),
    /// The optimistic concurrency precondition was not satisfied.
    #[error("optimistic concurrency violation: expected sequence {expected}, got {actual}")]
    ConcurrencyViolation { expected: u64, actual: u64 },
    /// The request could not be serialized before persistence.
    #[error("serialization error: {0}")]
    Serialization(String),
    /// A historical caller supplied no causal phase evidence.
    #[error("storage error: {0}")]
    Storage(String),
}

/// Convert phase-less backend-specific errors into [`PersistenceError::Storage`].
pub fn storage_error(error: impl std::fmt::Display) -> PersistenceError {
    PersistenceError::Storage(error.to_string())
}
