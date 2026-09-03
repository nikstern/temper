/// Causal commit evidence retained by entity mutation boundaries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum EntityMutationError {
    /// The mutation was rejected before its event became durable.
    NotApplied(String),
    /// The event became durable before a later response or projection failed.
    Applied(String),
    /// The mutation acknowledgement was lost and its commit cannot be proven.
    Unknown(String),
}

impl EntityMutationError {
    /// Returns the bounded diagnostic source retained by the boundary.
    pub(crate) fn diagnostic(&self) -> &str {
        match self {
            Self::NotApplied(value) | Self::Applied(value) | Self::Unknown(value) => value,
        }
    }
}

impl std::fmt::Display for EntityMutationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.diagnostic())
    }
}
