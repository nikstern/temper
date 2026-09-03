use super::*;

#[tokio::test]
async fn not_found_conflict_and_batch_partial_failure_are_structured() {
    let invocation = invocation(
        BTreeSet::from([
            DataOperationKind::EntityCreate,
            DataOperationKind::EntityGet,
            DataOperationKind::EntityPatch,
            DataOperationKind::ActionInvoke,
            DataOperationKind::Batch,
        ]),
        SecurityContext::system(),
    );
    let missing = call(
        &invocation,
        DataOperationV1::EntityGet {
            entity_type: "Temper.Example.Customer".into(),
            entity_id: "018f1f80-7b2d-7000-8000-000000000099".into(),
            at_least_sequence: None,
        },
    )
    .await;
    let missing = response_error(missing);
    assert_eq!(missing.kind(), ModuleDataErrorKind::NotFound);
    assert_eq!(
        missing.outcome(),
        temper_wasm_sdk::FailureOutcome::NotApplied
    );

    let id = "018f1f80-7b2d-7000-8000-000000000001";
    let _ = call(
        &invocation,
        DataOperationV1::EntityCreate {
            entity_type: "Temper.Example.Customer".into(),
            value: serde_json::json!({"Id": id, "Name": "Ada"})
                .as_object()
                .cloned()
                .unwrap(),
        },
    )
    .await;
    let conflict = response_error(
        call(
            &invocation,
            DataOperationV1::EntityPatch {
                entity_type: "Temper.Example.Customer".into(),
                entity_id: id.into(),
                expected_sequence: Some(99),
                value: serde_json::json!({"Name": "Grace"})
                    .as_object()
                    .cloned()
                    .unwrap(),
            },
        )
        .await,
    );
    assert_eq!(conflict.kind(), ModuleDataErrorKind::Conflict);
    assert_eq!(
        conflict.outcome(),
        temper_wasm_sdk::FailureOutcome::NotApplied
    );
    assert_eq!(
        conflict.retryability(),
        temper_wasm_sdk::FailureRetryability::AfterRefresh
    );

    let rejected = response_error(
        call(
            &invocation,
            DataOperationV1::ActionInvoke {
                entity_type: "Temper.Example.Customer".into(),
                entity_id: id.into(),
                action: "Reject".into(),
                expected_sequence: None,
                params: serde_json::Map::new(),
            },
        )
        .await,
    );
    assert_eq!(rejected.kind(), ModuleDataErrorKind::GuardRejected);
    assert_eq!(
        rejected.outcome(),
        temper_wasm_sdk::FailureOutcome::NotApplied
    );

    let batch = call(
        &invocation,
        DataOperationV1::Batch {
            items: vec![
                temper_wasm_sdk::data::BatchItemV1::EntityGet {
                    entity_type: "Temper.Example.Customer".into(),
                    entity_id: id.into(),
                    at_least_sequence: None,
                },
                temper_wasm_sdk::data::BatchItemV1::EntityGet {
                    entity_type: "Temper.Example.Customer".into(),
                    entity_id: "018f1f80-7b2d-7000-8000-000000000099".into(),
                    at_least_sequence: None,
                },
            ],
        },
    )
    .await;
    let DataOutcomeV1::Ok {
        result: temper_wasm_sdk::data::DataResultV1::Batch { outcomes },
    } = batch.outcome
    else {
        panic!("batch should return per-item outcomes")
    };
    assert!(matches!(outcomes[0], DataOutcomeV1::Ok { .. }));
    let DataOutcomeV1::Error { error } = &outcomes[1] else {
        panic!("missing batch member should fail")
    };
    assert_eq!(error.outcome(), temper_wasm_sdk::FailureOutcome::NotApplied);
}
