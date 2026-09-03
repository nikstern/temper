//! Structured module-data error adaptation.

use super::{ModuleDataError, ModuleDataErrorKind};
use temper_failure::{
    CausalOperationV1, FailureContractError, FailureEnvelopeV1, FailureProvenanceV1, FailureSource,
    GuestFailureDeclarationV1, ProvenanceToken,
};

/// Adapt a canonical module-data error without inspecting diagnostic text.
pub fn adapt_module_data_error(
    error: &ModuleDataError,
    operation: CausalOperationV1,
) -> Result<FailureEnvelopeV1, FailureContractError> {
    let declaration = GuestFailureDeclarationV1::from(error.clone());
    let provenance = FailureProvenanceV1 {
        source: FailureSource::ModuleData,
        component: ProvenanceToken::new("module-data")?,
        source_code: Some(ProvenanceToken::new(source_code(error.kind()))?),
    };
    let mut envelope = FailureEnvelopeV1::new(
        declaration.category,
        declaration.code,
        declaration.retryability,
        declaration.outcome,
        operation,
        provenance,
    )?;
    envelope.message = declaration.diagnostic;
    envelope.diagnostic_omitted = error.diagnostic_omitted();
    envelope.details = declaration.details;
    envelope.details_omitted = error.details_omitted();
    Ok(envelope)
}

fn source_code(kind: ModuleDataErrorKind) -> &'static str {
    match kind {
        ModuleDataErrorKind::InvalidRequest => "InvalidRequest",
        ModuleDataErrorKind::SchemaMismatch => "SchemaMismatch",
        ModuleDataErrorKind::NotFound => "NotFound",
        ModuleDataErrorKind::AlreadyExists => "AlreadyExists",
        ModuleDataErrorKind::AuthorizationDenied => "AuthorizationDenied",
        ModuleDataErrorKind::GuardRejected => "GuardRejected",
        ModuleDataErrorKind::RelationViolation => "RelationViolation",
        ModuleDataErrorKind::VerificationFailed => "VerificationFailed",
        ModuleDataErrorKind::Conflict => "Conflict",
        ModuleDataErrorKind::ConsistencyUnavailable => "ConsistencyUnavailable",
        ModuleDataErrorKind::BudgetExceeded => "BudgetExceeded",
        ModuleDataErrorKind::TransientUnavailable => "TransientUnavailable",
        ModuleDataErrorKind::Internal => "Internal",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use temper_failure::{
        DetailKey, FailureCategory, FailureDetailValue, FailureOutcome, FailureRetryability,
        OperationAttempt, OperationId, OperationKind,
    };

    fn operation() -> CausalOperationV1 {
        CausalOperationV1 {
            id: OperationId::new("module-call:42").expect("valid id"),
            kind: OperationKind::new("module_data.action").expect("valid kind"),
            attempt: OperationAttempt::new(1).expect("valid attempt"),
            parent_id: None,
        }
    }

    #[test]
    fn canonical_conversion_preserves_host_owned_facts() {
        let error = ModuleDataError::new(
            ModuleDataErrorKind::AuthorizationDenied,
            "AuthorizationDenied",
            "approval required",
            FailureRetryability::AfterAuthorization,
            FailureOutcome::NotApplied,
        )
        .expect("valid error")
        .with_decision_id("PD-123")
        .expect("valid decision id");
        let envelope = adapt_module_data_error(&error, operation()).expect("valid adaptation");
        assert_eq!(envelope.category, FailureCategory::Authorization);
        assert_eq!(
            envelope.retryability,
            FailureRetryability::AfterAuthorization
        );
        assert_eq!(envelope.outcome, FailureOutcome::NotApplied);
        assert!(matches!(
            envelope
                .details
                .values()
                .get(&DetailKey::new("decision_id").expect("static detail key")),
            Some(FailureDetailValue::String(value)) if value.as_str() == "PD-123"
        ));
    }

    #[test]
    fn unknown_outcome_is_ambiguous_reconciliation() {
        let error = ModuleDataError::new(
            ModuleDataErrorKind::TransientUnavailable,
            "AcknowledgementLost",
            "provider acknowledgement was not observed",
            FailureRetryability::Reconcile,
            FailureOutcome::Unknown,
        )
        .expect("valid error");
        let envelope = adapt_module_data_error(&error, operation()).expect("valid adaptation");
        assert_eq!(envelope.category, FailureCategory::Ambiguous);
        assert_eq!(envelope.retryability, FailureRetryability::Reconcile);
        assert_eq!(envelope.outcome, FailureOutcome::Unknown);
    }

    #[test]
    fn oversized_diagnostic_preserves_omission_evidence() {
        let error = ModuleDataError::new(
            ModuleDataErrorKind::Internal,
            "InternalFailure",
            "x".repeat(temper_failure::MAX_DIAGNOSTIC_BYTES + 1),
            FailureRetryability::Never,
            FailureOutcome::NotApplied,
        )
        .expect("optional oversized diagnostic is omitted");
        let envelope = adapt_module_data_error(&error, operation()).expect("valid adaptation");
        assert!(envelope.message.is_none());
        assert!(envelope.diagnostic_omitted);
        assert!(matches!(
            envelope
                .details
                .values()
                .get(&DetailKey::new("diagnostic_omitted").expect("static detail key")),
            Some(FailureDetailValue::Bool(true))
        ));
    }
}
