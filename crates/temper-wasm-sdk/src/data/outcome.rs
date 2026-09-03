//! Commit-preserving generated write and action outcomes.

use super::{CommitToken, TypedAction, TypedWrite};

/// Typed closed outcome from atomic create-or-verify.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CreateOrVerifyOutcome<T> {
    /// This invocation committed the entity creation.
    Created {
        /// Durable commit for the authoritative entity.
        commit: CommitToken,
        /// Canonical authoritative entity value.
        value: T,
    },
    /// An existing entity has the same immutable creation contract.
    AlreadyMatches {
        /// Durable commit for the authoritative entity, which may use another ID.
        commit: CommitToken,
        /// Canonical authoritative entity value.
        value: T,
    },
    /// The requested creation contract conflicts with existing ownership.
    Conflict {
        /// Sorted, bounded canonical schema-owned field identifiers.
        fields: Vec<String>,
        /// Whether additional conflicting fields were withheld.
        truncated: bool,
    },
}

/// A typed value paired with the durable commit that produced it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Committed<T> {
    /// Durable per-entity commit token.
    pub commit: CommitToken,
    /// Typed committed value.
    pub value: T,
}

/// Why a required committed response value is absent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommittedAbsenceReason {
    /// The response budget deliberately omitted the value.
    DeliberatelyOmitted,
    /// The host returned neither a value nor a deliberate-omission marker.
    UnexpectedlyAbsent,
}

/// Commit-preserving absence of a required response value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommittedAbsence {
    /// Durable commit that must not be retried as though it failed.
    pub commit: CommitToken,
    /// Explicit absence classification.
    pub reason: CommittedAbsenceReason,
}

/// Commit-preserving outcome for a nullable action result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommittedNullable<T> {
    /// A non-null result was present.
    Value(Committed<T>),
    /// The canonical result was explicitly null.
    Null { commit: CommitToken },
    /// The response budget deliberately omitted the result.
    DeliberatelyOmitted { commit: CommitToken },
    /// The host returned no result and no omission marker.
    UnexpectedlyAbsent { commit: CommitToken },
}

/// Malformed response classification for a void action.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommittedVoidErrorReason {
    /// A void action was marked as deliberately omitted.
    OmissionMarked,
    /// A void action unexpectedly carried a result value.
    UnexpectedValue,
}

/// Commit-preserving malformed void-action response.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommittedVoidError {
    /// Durable commit that must not be retried as though it failed.
    pub commit: CommitToken,
    /// Explicit malformed-response classification.
    pub reason: CommittedVoidErrorReason,
}

impl<T> TypedWrite<T> {
    /// Require the canonical written value without discarding commit evidence.
    pub fn required_value(self) -> Result<Committed<T>, CommittedAbsence> {
        match self.value {
            Some(value) => Ok(Committed {
                commit: self.commit,
                value,
            }),
            None => Err(CommittedAbsence {
                commit: self.commit,
                reason: if self.value_omitted {
                    CommittedAbsenceReason::DeliberatelyOmitted
                } else {
                    CommittedAbsenceReason::UnexpectedlyAbsent
                },
            }),
        }
    }
}

impl<T> TypedAction<T> {
    /// Require a non-null action result without discarding commit evidence.
    pub fn required_result(self) -> Result<Committed<T>, CommittedAbsence> {
        match self.result {
            Some(value) => Ok(Committed {
                commit: self.commit,
                value,
            }),
            None => Err(CommittedAbsence {
                commit: self.commit,
                reason: if self.result_omitted {
                    CommittedAbsenceReason::DeliberatelyOmitted
                } else {
                    CommittedAbsenceReason::UnexpectedlyAbsent
                },
            }),
        }
    }
}

impl<T> TypedAction<Option<T>> {
    /// Classify a nullable result without collapsing null into transport absence.
    pub fn nullable_result(self) -> CommittedNullable<T> {
        match self.result {
            Some(Some(value)) => CommittedNullable::Value(Committed {
                commit: self.commit,
                value,
            }),
            Some(None) => CommittedNullable::Null {
                commit: self.commit,
            },
            None if self.result_omitted => CommittedNullable::DeliberatelyOmitted {
                commit: self.commit,
            },
            None => CommittedNullable::UnexpectedlyAbsent {
                commit: self.commit,
            },
        }
    }
}

impl TypedAction<()> {
    /// Confirm a void action without fabricating a result value.
    pub fn void_result(self) -> Result<CommitToken, CommittedVoidError> {
        if self.result_omitted {
            return Err(CommittedVoidError {
                commit: self.commit,
                reason: CommittedVoidErrorReason::OmissionMarked,
            });
        }
        if self.result.is_some() {
            return Err(CommittedVoidError {
                commit: self.commit,
                reason: CommittedVoidErrorReason::UnexpectedValue,
            });
        }
        Ok(self.commit)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn commit() -> CommitToken {
        CommitToken {
            entity_type: "Temper.Task".into(),
            entity_id: "task-1".into(),
            sequence: 7,
        }
    }

    #[test]
    fn required_absence_always_retains_commit() {
        let outcome = TypedWrite::<String> {
            commit: commit(),
            value: None,
            value_omitted: true,
        }
        .required_value()
        .unwrap_err();
        assert_eq!(outcome.commit.sequence, 7);
        assert_eq!(outcome.reason, CommittedAbsenceReason::DeliberatelyOmitted);
    }

    #[test]
    fn nullable_result_distinguishes_null_omitted_and_absent() {
        let null = TypedAction::<Option<String>> {
            commit: commit(),
            result: Some(None),
            result_omitted: false,
        }
        .nullable_result();
        assert!(matches!(null, CommittedNullable::Null { .. }));

        let omitted = TypedAction::<Option<String>> {
            commit: commit(),
            result: None,
            result_omitted: true,
        }
        .nullable_result();
        assert!(matches!(
            omitted,
            CommittedNullable::DeliberatelyOmitted { .. }
        ));

        let absent = TypedAction::<Option<String>> {
            commit: commit(),
            result: None,
            result_omitted: false,
        }
        .nullable_result();
        assert!(matches!(
            absent,
            CommittedNullable::UnexpectedlyAbsent { .. }
        ));
    }
}
