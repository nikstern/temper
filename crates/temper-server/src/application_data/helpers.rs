//! Bounded envelope and result helpers.

use temper_wasm_sdk::data::{
    BatchItemV1, CommitToken, DataOperationV1, DataResponseV1, DataResultV1, ModuleDataError,
    ModuleDataErrorKind,
};
use temper_wasm_sdk::{FailureOutcome, FailureRetryability};

pub(super) const MAX_CANONICAL_IDENTIFIER_BYTES: usize = 256;
const COMPACT_OUTCOME_BYTES: usize = 3_840;

pub(super) fn reserve_compact_response(
    operation: &DataOperationV1,
    response_budget: usize,
) -> Result<(), ModuleDataError> {
    let outcomes = match operation {
        DataOperationV1::Batch { items } => items.len(),
        _ => 1,
    };
    let required = outcomes
        .saturating_mul(COMPACT_OUTCOME_BYTES)
        .saturating_add(256);
    if required > response_budget {
        return Err(not_applied_error(
            ModuleDataErrorKind::BudgetExceeded,
            "ResponseReservationExceeded",
            "compact operation acknowledgement exceeds the response budget",
        ));
    }
    Ok(())
}

pub(super) fn validate_operation_identifiers(
    operation: &DataOperationV1,
) -> Result<(), ModuleDataError> {
    fn identifier(value: &str) -> Result<(), ModuleDataError> {
        if value.is_empty() || value.len() > MAX_CANONICAL_IDENTIFIER_BYTES {
            return Err(not_applied_error(
                ModuleDataErrorKind::InvalidRequest,
                "InvalidIdentifier",
                "canonical identifiers must contain between 1 and 256 UTF-8 bytes",
            ));
        }
        Ok(())
    }
    fn batch_item(item: &BatchItemV1) -> Result<(), ModuleDataError> {
        match item {
            BatchItemV1::EntityGet {
                entity_type,
                entity_id,
                ..
            }
            | BatchItemV1::EntityPatch {
                entity_type,
                entity_id,
                ..
            } => {
                identifier(entity_type)?;
                identifier(entity_id)
            }
            BatchItemV1::EntityCreate { entity_type, value } => {
                identifier(entity_type)?;
                super::extract_id(value).map(|_| ())
            }
            BatchItemV1::ActionInvoke {
                entity_type,
                entity_id,
                action,
                ..
            } => {
                identifier(entity_type)?;
                identifier(entity_id)?;
                identifier(action)
            }
        }
    }
    match operation {
        DataOperationV1::EntityGet {
            entity_type,
            entity_id,
            ..
        }
        | DataOperationV1::EntityPatch {
            entity_type,
            entity_id,
            ..
        } => {
            identifier(entity_type)?;
            identifier(entity_id)
        }
        DataOperationV1::EntityQuery {
            entity_type,
            order_by,
            ..
        } => {
            identifier(entity_type)?;
            for order in order_by {
                if let temper_wasm_sdk::data::OrderV1::Property { field, .. } = order {
                    identifier(field)?;
                }
            }
            Ok(())
        }
        DataOperationV1::EntityCreate { entity_type, value } => {
            identifier(entity_type)?;
            super::extract_id(value).and_then(|id| identifier(&id))
        }
        DataOperationV1::EntityCreateOrVerify {
            entity_type,
            idempotency_key,
            value,
        } => {
            identifier(entity_type)?;
            identifier(idempotency_key)?;
            super::extract_id(value).and_then(|id| identifier(&id))
        }
        DataOperationV1::ActionInvoke {
            entity_type,
            entity_id,
            action,
            ..
        }
        | DataOperationV1::CompositeInvoke {
            entity_type,
            entity_id,
            action,
            ..
        } => {
            identifier(entity_type)?;
            identifier(entity_id)?;
            identifier(action)
        }
        DataOperationV1::Batch { items } => items.iter().try_for_each(batch_item),
        DataOperationV1::FileReadOpen {
            file_id,
            version_id,
        } => {
            identifier(file_id)?;
            if let Some(version_id) = version_id {
                identifier(version_id)?;
            }
            Ok(())
        }
        DataOperationV1::FileWriteOpen { file_id, .. } => identifier(file_id),
        DataOperationV1::FileWriteCommit { .. } | DataOperationV1::FileStreamAbort { .. } => Ok(()),
    }
}

pub(super) fn validate_value_budget(
    value: &serde_json::Value,
    depth: usize,
    nodes: &mut usize,
    byte_budget: usize,
) -> Result<(), ModuleDataError> {
    *nodes = nodes.saturating_add(1);
    if depth > 16 || *nodes > byte_budget / 2 {
        return Err(not_applied_error(
            ModuleDataErrorKind::BudgetExceeded,
            "PayloadStructureBudgetExceeded",
            "request object depth or element budget exceeded",
        ));
    }
    match value {
        serde_json::Value::String(value) if value.len() > byte_budget => Err(not_applied_error(
            ModuleDataErrorKind::BudgetExceeded,
            "StringBudgetExceeded",
            "request string exceeds the byte budget",
        )),
        serde_json::Value::Array(values) => {
            for value in values {
                validate_value_budget(value, depth + 1, nodes, byte_budget)?;
            }
            Ok(())
        }
        serde_json::Value::Object(values) => {
            for (name, value) in values {
                if name.len() > byte_budget {
                    return Err(not_applied_error(
                        ModuleDataErrorKind::BudgetExceeded,
                        "StringBudgetExceeded",
                        "request property name exceeds the byte budget",
                    ));
                }
                validate_value_budget(value, depth + 1, nodes, byte_budget)?;
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

pub(super) fn compact_committed_results(response: &mut DataResponseV1) {
    fn compact_result(result: &mut DataResultV1) {
        match result {
            DataResultV1::Write {
                value,
                value_omitted,
                ..
            } => {
                if value.take().is_some() {
                    *value_omitted = true;
                }
            }
            DataResultV1::Action {
                result,
                result_omitted,
                ..
            } => {
                if result.take().is_some() {
                    *result_omitted = true;
                }
            }
            DataResultV1::FileCommitted { metadata, .. } => *metadata = None,
            DataResultV1::Batch { outcomes } => {
                for outcome in outcomes {
                    if let temper_wasm_sdk::data::DataOutcomeV1::Ok { result } = outcome {
                        compact_result(result);
                    }
                }
            }
            _ => {}
        }
    }
    if let temper_wasm_sdk::data::DataOutcomeV1::Ok { result } = &mut response.outcome {
        compact_result(result);
    }
}

pub(super) fn short_type(entity_type: &str) -> &str {
    entity_type.rsplit('.').next().unwrap_or(entity_type)
}

pub(super) fn extract_id(
    value: &serde_json::Map<String, serde_json::Value>,
) -> Result<String, ModuleDataError> {
    value
        .get("Id")
        .or_else(|| value.get("id"))
        .and_then(serde_json::Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .ok_or_else(|| {
            not_applied_error(
                ModuleDataErrorKind::InvalidRequest,
                "MissingEntityId",
                "entity create value must contain a string Id",
            )
        })
}

pub(super) fn commit(entity_type: &str, entity_id: &str, sequence: u64) -> CommitToken {
    CommitToken {
        entity_type: entity_type.into(),
        entity_id: entity_id.into(),
        sequence,
    }
}

pub(super) fn write_result(
    entity_type: &str,
    entity_id: &str,
    sequence: u64,
    value: serde_json::Value,
) -> DataResultV1 {
    DataResultV1::Write {
        commit: commit(entity_type, entity_id, sequence),
        value: value.as_object().cloned(),
        value_omitted: false,
    }
}

pub(super) fn not_applied_internal_error(error: String) -> ModuleDataError {
    tracing::error!(%error, "application-data internal operation failed");
    ModuleDataError::new(
        ModuleDataErrorKind::Internal,
        "DataServiceFailure",
        "application-data operation failed",
        FailureRetryability::Never,
        FailureOutcome::NotApplied,
    )
    .expect("static module-data failure contract must be valid")
}

pub(super) fn not_applied_error(
    kind: ModuleDataErrorKind,
    code: &str,
    message: &str,
) -> ModuleDataError {
    ModuleDataError::new(
        kind,
        code,
        message.chars().take(256).collect::<String>(),
        FailureRetryability::Never,
        FailureOutcome::NotApplied,
    )
    .expect("static module-data failure contract must be valid")
}

pub(super) fn applied_internal_error(error: String) -> ModuleDataError {
    tracing::error!(%error, "application-data post-commit operation failed");
    ModuleDataError::new(
        ModuleDataErrorKind::Internal,
        "PostCommitDataServiceFailure",
        "application-data response failed after commit",
        FailureRetryability::Never,
        FailureOutcome::Applied,
    )
    .expect("static post-commit module-data failure contract must be valid")
}

pub(super) fn error_with_outcome(
    kind: ModuleDataErrorKind,
    code: &str,
    message: &str,
    outcome: FailureOutcome,
) -> ModuleDataError {
    let retryability = if outcome == FailureOutcome::Unknown {
        FailureRetryability::Reconcile
    } else {
        FailureRetryability::Never
    };
    ModuleDataError::new(
        kind,
        code,
        message.chars().take(256).collect::<String>(),
        retryability,
        outcome,
    )
    .expect("static outcome-aware module-data failure contract must be valid")
}

pub(super) fn unknown_internal_error(error: String) -> ModuleDataError {
    tracing::error!(%error, "application-data acknowledgement is unknown");
    ModuleDataError::new(
        ModuleDataErrorKind::TransientUnavailable,
        "DataAcknowledgementUnknown",
        "application-data commit acknowledgement was not observed",
        FailureRetryability::Reconcile,
        FailureOutcome::Unknown,
    )
    .expect("static unknown-outcome module-data failure contract must be valid")
}

pub(super) fn read_service_error(
    error: super::service::ApplicationDataReadError,
) -> ModuleDataError {
    match error {
        super::service::ApplicationDataReadError::NotFound => not_applied_error(
            ModuleDataErrorKind::NotFound,
            "EntityNotFound",
            "application-data operation failed",
        ),
        super::service::ApplicationDataReadError::Internal(diagnostic) => {
            not_applied_internal_error(diagnostic)
        }
    }
}

pub(super) fn write_service_error(
    error: super::service::ApplicationDataWriteError,
) -> ModuleDataError {
    use super::service::{ApplicationDataRejection, ApplicationDataWriteError};
    match error {
        ApplicationDataWriteError::Applied(diagnostic) => applied_internal_error(diagnostic),
        ApplicationDataWriteError::Unknown(diagnostic) => unknown_internal_error(diagnostic),
        ApplicationDataWriteError::NotApplied { reason, diagnostic } => {
            tracing::error!(%diagnostic, ?reason, "application-data write rejected before commit");
            let (kind, code, retryability) = match reason {
                ApplicationDataRejection::Conflict => (
                    ModuleDataErrorKind::Conflict,
                    "SequenceConflict",
                    FailureRetryability::AfterRefresh,
                ),
                ApplicationDataRejection::AuthorizationDenied => (
                    ModuleDataErrorKind::AuthorizationDenied,
                    "AuthorizationDenied",
                    FailureRetryability::AfterAuthorization,
                ),
                ApplicationDataRejection::BudgetExceeded => (
                    ModuleDataErrorKind::BudgetExceeded,
                    "BudgetExceeded",
                    FailureRetryability::Never,
                ),
                ApplicationDataRejection::SchemaMismatch => (
                    ModuleDataErrorKind::SchemaMismatch,
                    "SchemaMismatch",
                    FailureRetryability::Never,
                ),
                ApplicationDataRejection::Internal => (
                    ModuleDataErrorKind::Internal,
                    "DataServiceFailure",
                    FailureRetryability::Never,
                ),
            };
            ModuleDataError::new(
                kind,
                code,
                "application-data write was rejected",
                retryability,
                FailureOutcome::NotApplied,
            )
            .expect("static write-rejection contract must be valid")
        }
    }
}

pub(super) fn mark_applied(error: ModuleDataError) -> ModuleDataError {
    error
        .with_outcome(FailureOutcome::Applied)
        .expect("canonical module-data error remains valid with a known applied outcome")
}

#[cfg(test)]
#[path = "helpers_tests.rs"]
mod tests;
