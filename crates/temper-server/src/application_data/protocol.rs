//! Version-selecting request and response protocol helpers.

use temper_wasm_sdk::FailureOutcome;
use temper_wasm_sdk::data::{
    DATA_ABI_VERSION_V1, DATA_ABI_VERSION_V2, DataResponseV1, DataResponseV2, DataResultV1,
    ModuleDataError, ModuleDataErrorKind,
};

use super::not_applied_error;

/// Decode one version-selected request without substituting malformed fields.
pub(super) fn decode_request<T: serde::de::DeserializeOwned>(
    bytes: &[u8],
) -> Result<T, ModuleDataError> {
    serde_json::from_slice(bytes).map_err(|error| {
        not_applied_error(
            ModuleDataErrorKind::InvalidRequest,
            "InvalidRequest",
            &error.to_string(),
        )
    })
}

/// Build the canonical not-applied response for an artifact/request ABI mismatch.
pub(super) fn abi_mismatch() -> ModuleDataError {
    not_applied_error(
        ModuleDataErrorKind::SchemaMismatch,
        "AbiMismatch",
        "application-data request ABI does not match the artifact binding",
    )
}

/// Encode a response in the exact ABI selected by the artifact binding.
pub(super) fn encode_response(abi: u32, response: &DataResponseV1) -> Result<Vec<u8>, String> {
    match abi {
        DATA_ABI_VERSION_V1 => serde_json::to_vec(response),
        DATA_ABI_VERSION_V2 => serde_json::to_vec(&DataResponseV2::from(response.clone())),
        _ => serde_json::to_vec(&DataResponseV1::error(abi_mismatch())),
    }
    .map_err(|error| error.to_string())
}

/// Aggregate the causal outcome represented by a response and its batch members.
pub(super) fn response_outcome(response: &DataResponseV1) -> FailureOutcome {
    match &response.outcome {
        temper_wasm_sdk::data::DataOutcomeV1::Error { error } => error.outcome(),
        temper_wasm_sdk::data::DataOutcomeV1::Ok { result } => result_outcome(result),
    }
}

fn result_outcome(result: &DataResultV1) -> FailureOutcome {
    match result {
        DataResultV1::Write { .. }
        | DataResultV1::Action { .. }
        | DataResultV1::FileCommitted { .. } => FailureOutcome::Applied,
        DataResultV1::CreateOrVerify {
            outcome:
                temper_wasm_sdk::data::CreateOrVerifyResultV1::Created { .. }
                | temper_wasm_sdk::data::CreateOrVerifyResultV1::AlreadyMatches { .. },
        } => FailureOutcome::Applied,
        DataResultV1::Batch { outcomes } => {
            let mut any_applied = false;
            for outcome in outcomes {
                let item = match outcome {
                    temper_wasm_sdk::data::DataOutcomeV1::Ok { result } => result_outcome(result),
                    temper_wasm_sdk::data::DataOutcomeV1::Error { error } => error.outcome(),
                };
                if item == FailureOutcome::Unknown {
                    return FailureOutcome::Unknown;
                }
                if item == FailureOutcome::Applied {
                    any_applied = true;
                }
            }
            if any_applied {
                FailureOutcome::Applied
            } else {
                FailureOutcome::NotApplied
            }
        }
        _ => FailureOutcome::NotApplied,
    }
}

#[cfg(test)]
mod tests {
    use temper_wasm_sdk::FailureRetryability;
    use temper_wasm_sdk::data::{DataOutcomeV1, ModuleDataError};

    use super::*;

    fn error(outcome: FailureOutcome) -> DataOutcomeV1 {
        DataOutcomeV1::Error {
            error: ModuleDataError::new(
                temper_wasm_sdk::data::ModuleDataErrorKind::Internal,
                if outcome == FailureOutcome::Unknown {
                    "DataAcknowledgementUnknown"
                } else {
                    "DataServiceFailure"
                },
                "bounded diagnostic",
                if outcome == FailureOutcome::Unknown {
                    FailureRetryability::Reconcile
                } else {
                    FailureRetryability::Never
                },
                outcome,
            )
            .expect("valid test failure"),
        }
    }

    #[test]
    fn response_budget_outcome_aggregation_preserves_unknown_without_known_commit() {
        let response = DataResponseV1::ok(DataResultV1::Batch {
            outcomes: vec![
                error(FailureOutcome::NotApplied),
                error(FailureOutcome::Unknown),
            ],
        });
        assert_eq!(response_outcome(&response), FailureOutcome::Unknown);
    }

    #[test]
    fn response_budget_outcome_aggregation_keeps_unknown_dominant_over_known_commit() {
        let response = DataResponseV1::ok(DataResultV1::Batch {
            outcomes: vec![
                error(FailureOutcome::Unknown),
                DataOutcomeV1::Ok {
                    result: DataResultV1::Write {
                        commit: temper_wasm_sdk::data::CommitToken {
                            entity_type: "Temper.Example.Customer".into(),
                            entity_id: "customer-1".into(),
                            sequence: 1,
                        },
                        value: None,
                        value_omitted: true,
                    },
                },
            ],
        });
        assert_eq!(response_outcome(&response), FailureOutcome::Unknown);
    }
}
