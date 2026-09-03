use temper_failure::{
    BoundedDetailString, DetailKey, FailureCategory, FailureContractError, FailureDetailValue,
    FailureOutcome, FailureRetryability, GuestFailureDeclarationV1,
};

use super::{
    DataObject, ModuleDataError, ModuleDataErrorKind, ModuleDataErrorV1, Retryability,
    module_data_error::category_for,
};

const KINDS: [ModuleDataErrorKind; 13] = [
    ModuleDataErrorKind::InvalidRequest,
    ModuleDataErrorKind::SchemaMismatch,
    ModuleDataErrorKind::NotFound,
    ModuleDataErrorKind::AlreadyExists,
    ModuleDataErrorKind::AuthorizationDenied,
    ModuleDataErrorKind::GuardRejected,
    ModuleDataErrorKind::RelationViolation,
    ModuleDataErrorKind::VerificationFailed,
    ModuleDataErrorKind::Conflict,
    ModuleDataErrorKind::ConsistencyUnavailable,
    ModuleDataErrorKind::BudgetExceeded,
    ModuleDataErrorKind::TransientUnavailable,
    ModuleDataErrorKind::Internal,
];

#[test]
fn every_kind_has_one_stable_category_for_known_outcomes() {
    for kind in KINDS {
        for outcome in [FailureOutcome::NotApplied, FailureOutcome::Applied] {
            let error = ModuleDataError::new(
                kind,
                "StableCode",
                "safe diagnostic",
                FailureRetryability::Never,
                outcome,
            )
            .expect("known outcome contract");
            let declaration = GuestFailureDeclarationV1::from(error);
            assert_eq!(declaration.category, category_for(kind), "{kind:?}");
            assert_eq!(declaration.outcome, outcome, "{kind:?}");
            assert_eq!(declaration.retryability, FailureRetryability::Never);
        }
    }
}

#[test]
fn every_kind_is_forced_to_ambiguous_reconciliation_when_outcome_is_unknown() {
    for kind in KINDS {
        let error = ModuleDataError::new(
            kind,
            "StableCode",
            "safe diagnostic",
            FailureRetryability::Reconcile,
            FailureOutcome::Unknown,
        )
        .expect("unknown outcome contract");
        let declaration = GuestFailureDeclarationV1::from(error);
        assert_eq!(declaration.category, FailureCategory::Ambiguous, "{kind:?}");
        assert_eq!(declaration.outcome, FailureOutcome::Unknown, "{kind:?}");
        assert_eq!(declaration.retryability, FailureRetryability::Reconcile);
    }
}

#[test]
fn construction_rejects_invalid_codes_and_outcome_retry_disagreement() {
    assert!(matches!(
        ModuleDataError::new(
            ModuleDataErrorKind::Internal,
            "",
            "diagnostic",
            FailureRetryability::Never,
            FailureOutcome::NotApplied,
        ),
        Err(FailureContractError::EmptyToken { .. })
    ));
    assert!(matches!(
        ModuleDataError::new(
            ModuleDataErrorKind::Internal,
            "StableCode",
            "diagnostic",
            FailureRetryability::Never,
            FailureOutcome::Unknown,
        ),
        Err(FailureContractError::InvalidReconciliationGuidance)
    ));
    assert!(matches!(
        ModuleDataError::new(
            ModuleDataErrorKind::Internal,
            "StableCode",
            "diagnostic",
            FailureRetryability::Reconcile,
            FailureOutcome::Applied,
        ),
        Err(FailureContractError::InvalidReconciliationGuidance)
    ));
}

#[test]
fn every_kind_retryability_and_outcome_combination_has_exact_validity() {
    let retryabilities = [
        FailureRetryability::Never,
        FailureRetryability::AfterRefresh,
        FailureRetryability::WithBackoff,
        FailureRetryability::AfterAuthorization,
        FailureRetryability::Reconcile,
    ];
    let outcomes = [
        FailureOutcome::NotApplied,
        FailureOutcome::Applied,
        FailureOutcome::Unknown,
    ];
    for kind in KINDS {
        for retryability in retryabilities {
            for outcome in outcomes {
                let expected_valid = (outcome == FailureOutcome::Unknown)
                    == (retryability == FailureRetryability::Reconcile);
                let actual =
                    ModuleDataError::new(kind, "StableCode", "diagnostic", retryability, outcome);
                assert_eq!(
                    actual.is_ok(),
                    expected_valid,
                    "kind={kind:?} retryability={retryability:?} outcome={outcome:?}"
                );
            }
        }
    }
}

#[test]
fn source_detail_budgets_reserve_conversion_capacity() {
    let mut error = ModuleDataError::new(
        ModuleDataErrorKind::Internal,
        "StableCode",
        "diagnostic",
        FailureRetryability::Never,
        FailureOutcome::NotApplied,
    )
    .expect("valid error")
    .with_decision_id("PD-123")
    .expect("valid decision id");
    for index in 0..13 {
        error
            .try_insert_detail(
                DetailKey::new(format!("key_{index:02}")).expect("valid key"),
                FailureDetailValue::Unsigned(index),
            )
            .expect("source entry budget");
    }
    assert!(matches!(
        error.try_insert_detail(
            DetailKey::new("overflow").expect("valid key"),
            FailureDetailValue::Bool(true),
        ),
        Err(FailureContractError::TooManyDetails {
            max: 13,
            actual: 14
        })
    ));

    let declaration = GuestFailureDeclarationV1::from(error);
    assert_eq!(declaration.details.values().len(), 16);
    assert!(
        declaration
            .details
            .values()
            .contains_key(&DetailKey::new("decision_id").expect("static key"))
    );
}

#[test]
fn oversized_and_reserved_source_details_are_omitted_with_evidence() {
    let mut error = ModuleDataError::new(
        ModuleDataErrorKind::Internal,
        "StableCode",
        "diagnostic",
        FailureRetryability::Never,
        FailureOutcome::NotApplied,
    )
    .expect("valid error");
    assert!(matches!(
        error.try_insert_detail(
            DetailKey::new("decision_id").expect("valid key"),
            FailureDetailValue::Bool(true),
        ),
        Err(FailureContractError::DetailsEncoding(_))
    ));
    let long = BoundedDetailString::new("x".repeat(256)).expect("bounded detail string");
    for index in 0..7 {
        error.insert_detail_or_omit(
            DetailKey::new(format!("large_{index}")).expect("valid key"),
            FailureDetailValue::String(long.clone()),
        );
    }
    assert!(error.details_omitted());
    assert!(error.details().values().len() < 7);
    let declaration = GuestFailureDeclarationV1::from(error);
    assert!(matches!(
        declaration
            .details
            .values()
            .get(&DetailKey::new("details_omitted").expect("static key")),
        Some(FailureDetailValue::Bool(true))
    ));
}

#[test]
fn serialization_is_deterministic_and_rejects_contradictory_omission() {
    let mut first = ModuleDataError::new(
        ModuleDataErrorKind::Conflict,
        "SequenceConflict",
        "refresh first",
        FailureRetryability::AfterRefresh,
        FailureOutcome::NotApplied,
    )
    .expect("valid error");
    let mut second = first.clone();
    for (key, value) in [("z", 2), ("a", 1)] {
        first
            .try_insert_detail(
                DetailKey::new(key).expect("valid key"),
                FailureDetailValue::Signed(value),
            )
            .expect("valid detail");
    }
    for (key, value) in [("a", 1), ("z", 2)] {
        second
            .try_insert_detail(
                DetailKey::new(key).expect("valid key"),
                FailureDetailValue::Signed(value),
            )
            .expect("valid detail");
    }
    assert_eq!(
        serde_json::to_vec(&first).expect("serialize first"),
        serde_json::to_vec(&second).expect("serialize second")
    );

    let contradictory = r#"{"kind":"internal","code":"StableCode","diagnostic":"present","diagnostic_omitted":true,"retryability":"never","outcome":"not_applied","details":{},"details_omitted":false}"#;
    assert!(serde_json::from_str::<ModuleDataError>(contradictory).is_err());
}

#[test]
fn legacy_failures_promote_conservatively_and_retry_projection_is_exact() {
    let legacy = ModuleDataErrorV1 {
        kind: ModuleDataErrorKind::Conflict,
        code: "SequenceConflict".into(),
        message: "refresh first".into(),
        retryability: Retryability::AfterRefresh,
        decision_id: Some("PD-123".into()),
        details: Some(Box::new(DataObject::from_iter([(
            "legacy_nested".into(),
            serde_json::json!({"unsafe": [1, 2, 3]}),
        )]))),
    };
    let promoted = ModuleDataError::try_from(legacy).expect("promote legacy error");
    assert_eq!(promoted.outcome(), FailureOutcome::Unknown);
    assert_eq!(promoted.retryability(), FailureRetryability::Reconcile);
    assert_eq!(
        promoted.decision_id().expect("decision id").as_str(),
        "PD-123"
    );
    assert!(promoted.details_omitted());

    for (retryability, expected) in [
        (FailureRetryability::Never, Retryability::Never),
        (
            FailureRetryability::AfterRefresh,
            Retryability::AfterRefresh,
        ),
        (FailureRetryability::WithBackoff, Retryability::WithBackoff),
        (FailureRetryability::AfterAuthorization, Retryability::Never),
        (FailureRetryability::Reconcile, Retryability::Never),
    ] {
        let outcome = if retryability == FailureRetryability::Reconcile {
            FailureOutcome::Unknown
        } else {
            FailureOutcome::NotApplied
        };
        let error = ModuleDataError::new(
            ModuleDataErrorKind::Internal,
            "StableCode",
            "diagnostic",
            retryability,
            outcome,
        )
        .expect("valid retry contract");
        assert_eq!(ModuleDataErrorV1::from(&error).retryability, expected);
    }
}
