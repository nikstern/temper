use super::*;
use crate::data::{DataOutcomeV2, DataResponseV1, DataResponseV2, DataResultV2};
use temper_failure::{BoundedDetailString, DetailKey, FailureDetailValue};
use temper_failure::{FailureOutcome, FailureRetryability};

#[test]
fn exact_entity_get_encoding() {
    let request = DataRequestV1::new(DataOperationV1::EntityGet {
        entity_type: "Temper.App.Task".into(),
        entity_id: "task-1".into(),
        at_least_sequence: Some(7),
    });
    assert_eq!(
        serde_json::to_string(&request).expect("serialize request"),
        r#"{"abi":1,"operation":{"kind":"entity_get","entity_type":"Temper.App.Task","entity_id":"task-1","at_least_sequence":7}}"#
    );
}

#[test]
fn unknown_fields_fail_closed() {
    let input = r#"{"abi":1,"operation":{"kind":"entity_get","entity_type":"Task","entity_id":"1","tenant":"other"}}"#;
    assert!(serde_json::from_str::<DataRequestV1>(input).is_err());
}

#[test]
fn exact_error_encoding() {
    let error = ModuleDataError::new(
        ModuleDataErrorKind::NotFound,
        "EntityNotFound",
        "entity not found",
        FailureRetryability::Never,
        FailureOutcome::NotApplied,
    )
    .expect("valid canonical error");
    let response = DataResponseV1::error(error);
    assert_eq!(
        serde_json::to_string(&response).expect("serialize response"),
        r#"{"abi":1,"outcome":{"kind":"error","error":{"kind":"not_found","code":"EntityNotFound","message":"entity not found","retryability":"never"}}}"#
    );
}

#[test]
fn exact_v1_error_encoding_preserves_historical_scalar_details() {
    let mut error = ModuleDataError::new(
        ModuleDataErrorKind::Conflict,
        "SequenceConflict",
        "refresh first",
        FailureRetryability::AfterRefresh,
        FailureOutcome::NotApplied,
    )
    .expect("valid canonical error")
    .with_decision_id("PD-123")
    .expect("valid decision id");
    for (key, value) in [
        (
            "label",
            FailureDetailValue::String(
                BoundedDetailString::new("safe").expect("valid detail string"),
            ),
        ),
        ("signed", FailureDetailValue::Signed(-2)),
        ("unsigned", FailureDetailValue::Unsigned(7)),
        ("flag", FailureDetailValue::Bool(true)),
    ] {
        error
            .try_insert_detail(DetailKey::new(key).expect("valid key"), value)
            .expect("valid source detail");
    }
    let response = DataResponseV1::error(error);
    assert_eq!(
        serde_json::to_string(&response).expect("serialize response"),
        r#"{"abi":1,"outcome":{"kind":"error","error":{"kind":"conflict","code":"SequenceConflict","message":"refresh first","retryability":"after_refresh","decision_id":"PD-123","details":{"flag":true,"label":"safe","signed":-2,"unsigned":7}}}}"#
    );
}

#[test]
fn exact_v1_success_encoding_is_pinned() {
    let response = DataResponseV1::ok(DataResultV1::Entity {
        value: DataObject::from_iter([("Id".into(), "task-1".into())]),
        sequence: 7,
    });
    assert_eq!(
        serde_json::to_string(&response).expect("serialize response"),
        r#"{"abi":1,"outcome":{"kind":"ok","result":{"kind":"entity","value":{"Id":"task-1"},"sequence":7}}}"#
    );
}

#[test]
fn exact_v2_request_and_error_encoding_are_pinned() {
    let request = DataRequestV2::new(DataOperationV1::EntityGet {
        entity_type: "Temper.App.Task".into(),
        entity_id: "task-1".into(),
        at_least_sequence: Some(7),
    });
    assert_eq!(
        serde_json::to_string(&request).expect("serialize request"),
        r#"{"abi":2,"operation":{"kind":"entity_get","entity_type":"Temper.App.Task","entity_id":"task-1","at_least_sequence":7}}"#
    );

    let error = ModuleDataError::new(
        ModuleDataErrorKind::AuthorizationDenied,
        "AuthorizationDenied",
        "approval required",
        FailureRetryability::AfterAuthorization,
        FailureOutcome::NotApplied,
    )
    .expect("valid canonical error")
    .with_decision_id("PD-123")
    .expect("valid decision id");
    let response = DataResponseV2::error(error);
    assert_eq!(
        serde_json::to_string(&response).expect("serialize response"),
        r#"{"abi":2,"outcome":{"kind":"error","error":{"kind":"authorization_denied","code":"AuthorizationDenied","diagnostic":"approval required","diagnostic_omitted":false,"retryability":"after_authorization","outcome":"not_applied","decision_id":"PD-123","details":{},"details_omitted":false}}}"#
    );
}

#[test]
fn v2_batch_errors_retain_host_owned_outcomes() {
    let error = ModuleDataError::new(
        ModuleDataErrorKind::TransientUnavailable,
        "DataAcknowledgementUnknown",
        "acknowledgement was not observed",
        FailureRetryability::Reconcile,
        FailureOutcome::Unknown,
    )
    .expect("valid canonical error");
    let response = DataResponseV2::ok(DataResultV1::Batch {
        outcomes: vec![DataOutcomeV1::Error {
            error: error.clone(),
        }],
    });
    let encoded = serde_json::to_vec(&response).expect("serialize v2 batch");
    let decoded: DataResponseV2 = serde_json::from_slice(&encoded).expect("decode v2 batch");
    let DataOutcomeV2::Ok {
        result: DataResultV2::Batch { outcomes },
    } = decoded.outcome
    else {
        panic!("expected v2 batch response")
    };
    let DataOutcomeV2::Error { error: decoded } = &outcomes[0] else {
        panic!("expected nested error")
    };
    assert_eq!(decoded, &error);
}
