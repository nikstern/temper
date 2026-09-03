use std::collections::BTreeSet;

use temper_authz::SecurityContext;
use temper_wasm_sdk::data::{DataOperationKind, DataOperationV1, ModuleDataErrorKind};

use super::create_or_verify_tests::durable_invocation_with_store;
use super::tests::{CSDL, IOA, call, invocation, response_error};
use super::{ApplicationDataInvocation, ModuleInvocationAuthority};

async fn assert_replayed_customer(
    store: &temper_store_sim::SimEventStore,
    id: &str,
    sequence: u64,
    name: &str,
) {
    store.restore_faults(temper_store_sim::SimFaultConfig::none());
    let restarted = durable_invocation_with_store(
        BTreeSet::from([DataOperationKind::EntityGet]),
        SecurityContext::system(),
        store.clone(),
    );
    let response = call(
        &restarted,
        DataOperationV1::EntityGet {
            entity_type: "Temper.Example.Customer".into(),
            entity_id: id.into(),
            at_least_sequence: Some(sequence),
        },
    )
    .await;
    let temper_wasm_sdk::data::DataOutcomeV1::Ok {
        result:
            temper_wasm_sdk::data::DataResultV1::Entity {
                value,
                sequence: actual,
            },
    } = response.outcome
    else {
        panic!("durable replay must recover the committed write: {response:?}")
    };
    assert_eq!(actual, sequence);
    assert_eq!(value["Name"], name);
}

fn assert_commit_phase_contract(
    error: &temper_wasm_sdk::data::ModuleDataError,
    expected_outcome: temper_wasm_sdk::FailureOutcome,
) {
    let (code, retryability) = match expected_outcome {
        temper_wasm_sdk::FailureOutcome::Applied => (
            "PostCommitDataServiceFailure",
            temper_wasm_sdk::FailureRetryability::Never,
        ),
        temper_wasm_sdk::FailureOutcome::Unknown => (
            "DataAcknowledgementUnknown",
            temper_wasm_sdk::FailureRetryability::Reconcile,
        ),
        temper_wasm_sdk::FailureOutcome::NotApplied => (
            "DataServiceFailure",
            temper_wasm_sdk::FailureRetryability::Never,
        ),
    };
    assert_eq!(error.outcome(), expected_outcome, "{error:?}");
    assert_eq!(error.code().as_str(), code);
    assert_eq!(error.retryability(), retryability);
}

#[tokio::test]
async fn ordinary_create_precommit_store_failure_is_not_applied() {
    let store = temper_store_sim::SimEventStore::new(
        90,
        temper_store_sim::SimFaultConfig {
            write_failure_prob: 1.0,
            ..temper_store_sim::SimFaultConfig::none()
        },
    );
    let invocation = durable_invocation_with_store(
        BTreeSet::from([DataOperationKind::EntityCreate]),
        SecurityContext::system(),
        store.clone(),
    );
    let id = "018f1f80-7b2d-7000-8000-000000000090";
    let response = call(
        &invocation,
        DataOperationV1::EntityCreate {
            entity_type: "Temper.Example.Customer".into(),
            value: serde_json::json!({"Id": id, "Name": "Ada"})
                .as_object()
                .expect("object")
                .clone(),
        },
    )
    .await;
    let error = response_error(response);
    assert_eq!(error.outcome(), temper_wasm_sdk::FailureOutcome::NotApplied);
    assert_eq!(error.code().as_str(), "DataServiceFailure");
    assert_eq!(
        error.retryability(),
        temper_wasm_sdk::FailureRetryability::Never
    );
    assert!(
        store
            .dump_journal(&format!("default:Customer:{id}"))
            .is_empty()
    );
}

#[tokio::test]
async fn ordinary_patch_precommit_store_failure_is_not_applied() {
    let store = temper_store_sim::SimEventStore::no_faults(91);
    let invocation = durable_invocation_with_store(
        BTreeSet::from([
            DataOperationKind::EntityCreate,
            DataOperationKind::EntityPatch,
        ]),
        SecurityContext::system(),
        store.clone(),
    );
    let id = "018f1f80-7b2d-7000-8000-000000000091";
    let created = call(
        &invocation,
        DataOperationV1::EntityCreate {
            entity_type: "Temper.Example.Customer".into(),
            value: serde_json::json!({
                "Id": id,
                "Name": "Ada"
            })
            .as_object()
            .expect("object")
            .clone(),
        },
    )
    .await;
    let temper_wasm_sdk::data::DataOutcomeV1::Ok {
        result: temper_wasm_sdk::data::DataResultV1::Write { commit, .. },
    } = created.outcome
    else {
        panic!("fixture create must succeed before fault injection")
    };
    store.restore_faults(temper_store_sim::SimFaultConfig {
        write_failure_prob: 1.0,
        ..temper_store_sim::SimFaultConfig::none()
    });
    let response = call(
        &invocation,
        DataOperationV1::EntityPatch {
            entity_type: "Temper.Example.Customer".into(),
            entity_id: id.into(),
            expected_sequence: Some(commit.sequence),
            value: serde_json::json!({"Name": "Grace"})
                .as_object()
                .expect("object")
                .clone(),
        },
    )
    .await;
    let error = response_error(response);
    assert_eq!(error.outcome(), temper_wasm_sdk::FailureOutcome::NotApplied);
    assert_eq!(error.code().as_str(), "DataServiceFailure");
    assert_eq!(
        error.retryability(),
        temper_wasm_sdk::FailureRetryability::Never
    );
}

async fn assert_ordinary_create_fault(
    faults: temper_store_sim::SimFaultConfig,
    expected_outcome: temper_wasm_sdk::FailureOutcome,
) {
    let store = temper_store_sim::SimEventStore::new(192, faults);
    let invocation = durable_invocation_with_store(
        BTreeSet::from([DataOperationKind::EntityCreate]),
        SecurityContext::system(),
        store.clone(),
    );
    let id = "018f1f80-7b2d-7000-8000-000000000192";
    let response = call(
        &invocation,
        DataOperationV1::EntityCreate {
            entity_type: "Temper.Example.Customer".into(),
            value: serde_json::json!({"Id": id, "Name": "Ada"})
                .as_object()
                .expect("object")
                .clone(),
        },
    )
    .await;
    let error = response_error(response);
    assert_commit_phase_contract(&error, expected_outcome);
    assert_eq!(
        store.dump_journal(&format!("default:Customer:{id}")).len(),
        1,
        "both injected failure modes occur after the first event"
    );
    assert_replayed_customer(&store, id, 1, "Ada").await;
}

async fn assert_ordinary_patch_fault(
    faults: temper_store_sim::SimFaultConfig,
    expected_outcome: temper_wasm_sdk::FailureOutcome,
    batched: bool,
) {
    let store = temper_store_sim::SimEventStore::no_faults(193);
    let invocation = durable_invocation_with_store(
        BTreeSet::from([
            DataOperationKind::EntityCreate,
            DataOperationKind::EntityPatch,
            DataOperationKind::Batch,
        ]),
        SecurityContext::system(),
        store.clone(),
    );
    let id = "018f1f80-7b2d-7000-8000-000000000193";
    let created = call(
        &invocation,
        DataOperationV1::EntityCreate {
            entity_type: "Temper.Example.Customer".into(),
            value: serde_json::json!({"Id": id, "Name": "Ada"})
                .as_object()
                .expect("object")
                .clone(),
        },
    )
    .await;
    let temper_wasm_sdk::data::DataOutcomeV1::Ok {
        result: temper_wasm_sdk::data::DataResultV1::Write { commit, .. },
    } = created.outcome
    else {
        panic!("fixture create must succeed before fault injection")
    };
    store.restore_faults(faults);
    let patch = temper_wasm_sdk::data::BatchItemV1::EntityPatch {
        entity_type: "Temper.Example.Customer".into(),
        entity_id: id.into(),
        expected_sequence: Some(commit.sequence),
        value: serde_json::json!({"Name": "Grace"})
            .as_object()
            .expect("object")
            .clone(),
    };
    let response = if batched {
        call(&invocation, DataOperationV1::Batch { items: vec![patch] }).await
    } else {
        call(
            &invocation,
            DataOperationV1::EntityPatch {
                entity_type: "Temper.Example.Customer".into(),
                entity_id: id.into(),
                expected_sequence: Some(commit.sequence),
                value: serde_json::json!({"Name": "Grace"})
                    .as_object()
                    .expect("object")
                    .clone(),
            },
        )
        .await
    };
    let error = if batched {
        let temper_wasm_sdk::data::DataOutcomeV1::Ok {
            result: temper_wasm_sdk::data::DataResultV1::Batch { mut outcomes },
        } = response.outcome
        else {
            panic!("batch envelope must succeed")
        };
        let temper_wasm_sdk::data::DataOutcomeV1::Error { error } = outcomes.remove(0) else {
            panic!("faulted batch member must fail")
        };
        error
    } else {
        response_error(response)
    };
    assert_commit_phase_contract(&error, expected_outcome);
    assert_eq!(
        store.dump_journal(&format!("default:Customer:{id}")).len(),
        2,
        "both injected failure modes occur after the append"
    );
    assert_replayed_customer(&store, id, 2, "Grace").await;
}

async fn assert_ordinary_action_fault(
    faults: temper_store_sim::SimFaultConfig,
    expected_outcome: temper_wasm_sdk::FailureOutcome,
) {
    let store = temper_store_sim::SimEventStore::no_faults(194);
    let invocation = durable_invocation_with_store(
        BTreeSet::from([
            DataOperationKind::EntityCreate,
            DataOperationKind::ActionInvoke,
        ]),
        SecurityContext::system(),
        store.clone(),
    );
    let id = "018f1f80-7b2d-7000-8000-000000000194";
    let created = call(
        &invocation,
        DataOperationV1::EntityCreate {
            entity_type: "Temper.Example.Customer".into(),
            value: serde_json::json!({"Id": id, "Name": "Ada"})
                .as_object()
                .expect("object")
                .clone(),
        },
    )
    .await;
    let temper_wasm_sdk::data::DataOutcomeV1::Ok {
        result: temper_wasm_sdk::data::DataResultV1::Write { commit, .. },
    } = created.outcome
    else {
        panic!("fixture create must succeed before fault injection")
    };
    store.restore_faults(faults);
    let response = call(
        &invocation,
        DataOperationV1::ActionInvoke {
            entity_type: "Temper.Example.Customer".into(),
            entity_id: id.into(),
            action: "Rename".into(),
            expected_sequence: Some(commit.sequence),
            params: serde_json::json!({"Name": "Grace"})
                .as_object()
                .expect("object")
                .clone(),
        },
    )
    .await;
    let error = response_error(response);
    assert_commit_phase_contract(&error, expected_outcome);
    assert_eq!(
        store.dump_journal(&format!("default:Customer:{id}")).len(),
        2,
        "action append is durable despite the injected failure"
    );
    assert_replayed_customer(&store, id, 2, "Grace").await;
}

#[tokio::test]
async fn ordinary_and_batch_post_commit_failures_are_applied() {
    let faults = temper_store_sim::SimFaultConfig {
        append_post_commit_failure_prob: 1.0,
        ..temper_store_sim::SimFaultConfig::none()
    };
    assert_ordinary_create_fault(faults.clone(), temper_wasm_sdk::FailureOutcome::Applied).await;
    assert_ordinary_action_fault(faults.clone(), temper_wasm_sdk::FailureOutcome::Applied).await;
    assert_ordinary_patch_fault(
        faults.clone(),
        temper_wasm_sdk::FailureOutcome::Applied,
        false,
    )
    .await;
    assert_ordinary_patch_fault(faults, temper_wasm_sdk::FailureOutcome::Applied, true).await;
}

#[tokio::test]
async fn ordinary_and_batch_lost_acknowledgements_are_unknown() {
    let faults = temper_store_sim::SimFaultConfig {
        append_acknowledgement_loss_prob: 1.0,
        ..temper_store_sim::SimFaultConfig::none()
    };
    assert_ordinary_create_fault(faults.clone(), temper_wasm_sdk::FailureOutcome::Unknown).await;
    assert_ordinary_action_fault(faults.clone(), temper_wasm_sdk::FailureOutcome::Unknown).await;
    assert_ordinary_patch_fault(
        faults.clone(),
        temper_wasm_sdk::FailureOutcome::Unknown,
        false,
    )
    .await;
    assert_ordinary_patch_fault(faults, temper_wasm_sdk::FailureOutcome::Unknown, true).await;
}

#[tokio::test]
async fn partial_post_commit_blob_failure_is_healed_by_restart_retry() {
    let store = temper_store_sim::SimEventStore::no_faults(82);
    let template = invocation(
        BTreeSet::from([DataOperationKind::EntityCreateOrVerify]),
        SecurityContext::system(),
    );
    let dir = tempfile::tempdir().unwrap();
    let ioa = format!(
        "{IOA}\n[[state]]\nname = \"Name\"\ntype = \"string\"\ninitial = \"\"\noverflow_inline_max_bytes = 1024\n\n[[state]]\nname = \"FailureReason\"\ntype = \"string\"\ninitial = \"\"\noverflow_inline_max_bytes = 1024\n"
    );
    let mut state = crate::state::ServerState::with_specs(
        temper_runtime::ActorSystem::new("create-or-verify-blob-fault"),
        temper_spec::csdl::parse_csdl(CSDL).unwrap(),
        CSDL.into(),
        std::collections::BTreeMap::from([("Customer".into(), ioa)]),
    )
    .unwrap();
    let mut stack = crate::storage::StorageStack::from_sim(store.clone(), None);
    stack.backend = crate::storage::BackendLabel::Turso;
    state.set_storage_stack(stack);
    state.blob_store_override = Some(crate::blob_store::BlobStore::failing_local_fs(
        dir.path(),
        1,
    ));
    let invocation = ApplicationDataInvocation::new(
        state.clone(),
        ModuleInvocationAuthority::new(
            template.authority.tenant.clone(),
            template.authority.module_name.clone(),
            template.authority.artifact_digest.clone(),
            template.authority.trigger.clone(),
            template.authority.triggering_entity_type.clone(),
            template.authority.security.clone(),
            template.authority.binding.clone(),
            template.authority.target.clone(),
        ),
    );
    let id = "018f1f80-7b2d-7000-8000-000000000092";
    let response = call(
        &invocation,
        DataOperationV1::EntityCreateOrVerify {
            entity_type: "Temper.Example.Customer".into(),
            idempotency_key: "request-92".into(),
            value: serde_json::json!({
                "Id": id,
                "Name": "n".repeat(4_096),
                "FailureReason": "f".repeat(4_096)
            })
            .as_object()
            .unwrap()
            .clone(),
        },
    )
    .await;
    let error = response_error(response);
    assert_eq!(error.kind(), ModuleDataErrorKind::Internal, "{error:?}");
    assert_eq!(error.outcome(), temper_wasm_sdk::FailureOutcome::Applied);
    assert_eq!(
        error.retryability(),
        temper_wasm_sdk::FailureRetryability::Never
    );
    let persistence_id = format!("default:Customer:{id}");
    assert_eq!(store.dump_journal(&persistence_id).len(), 1);
    let projection = store
        .dump_first_event_projection(&persistence_id)
        .expect("projection is co-committed before external blob writes");
    let keys = ["Name", "FailureReason"].map(|field| {
        projection.fields[field][crate::blobs::FIELD_OVERFLOW_REF_KEY]
            .as_str()
            .expect("large field projection contains a blob reference")
            .to_string()
    });
    let healthy_blob_store = crate::blob_store::BlobStore::local_fs(dir.path());
    let mut before = 0;
    for key in &keys {
        before += usize::from(healthy_blob_store.get(key).await.unwrap().is_some());
    }
    assert_eq!(
        before, 1,
        "the injected fault occurs after one object write"
    );

    let mut restarted_state = state.clone();
    let mut restarted_stack = crate::storage::StorageStack::from_sim(store.clone(), None);
    restarted_stack.backend = crate::storage::BackendLabel::Turso;
    restarted_state.set_storage_stack(restarted_stack);
    restarted_state.blob_store_override = Some(healthy_blob_store.clone());
    let restarted = ApplicationDataInvocation::new(
        restarted_state,
        ModuleInvocationAuthority::new(
            template.authority.tenant.clone(),
            template.authority.module_name.clone(),
            template.authority.artifact_digest.clone(),
            template.authority.trigger.clone(),
            template.authority.triggering_entity_type.clone(),
            template.authority.security.clone(),
            template.authority.binding.clone(),
            template.authority.target.clone(),
        ),
    );
    let retry = call(
        &restarted,
        DataOperationV1::EntityCreateOrVerify {
            entity_type: "Temper.Example.Customer".into(),
            idempotency_key: "request-92".into(),
            value: serde_json::json!({
                "Id": id,
                "Name": "n".repeat(4_096),
                "FailureReason": "f".repeat(4_096)
            })
            .as_object()
            .unwrap()
            .clone(),
        },
    )
    .await;
    assert!(matches!(
        retry.outcome,
        temper_wasm_sdk::data::DataOutcomeV1::Ok { .. }
    ));
    assert_eq!(store.dump_journal(&persistence_id).len(), 1);
    for key in keys {
        assert!(
            healthy_blob_store.get(&key).await.unwrap().is_some(),
            "restart retry repairs every referenced object"
        );
    }
}
