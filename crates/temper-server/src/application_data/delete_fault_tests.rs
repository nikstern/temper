use std::collections::BTreeSet;

use temper_authz::SecurityContext;
use temper_wasm_sdk::data::{DataOperationKind, DataOperationV1, DataOutcomeV1};

use super::create_or_verify_tests::durable_invocation_with_store;
use super::tests::call;

async fn assert_delete_fault(
    faults: temper_store_sim::SimFaultConfig,
    expected: temper_failure::FailureOutcome,
) {
    let store = temper_store_sim::SimEventStore::no_faults(9_302);
    let invocation = durable_invocation_with_store(
        BTreeSet::from([DataOperationKind::EntityCreate]),
        SecurityContext::system(),
        store.clone(),
    );
    let tenant = temper_runtime::tenant::TenantId::default();
    let id = "018f1f80-7b2d-7000-8000-000000009302";
    let created = call(
        &invocation,
        DataOperationV1::EntityCreate {
            entity_type: "Temper.Example.Customer".into(),
            value: serde_json::json!({"Id": id, "Name": "Ada"})
                .as_object()
                .unwrap()
                .clone(),
        },
    )
    .await;
    assert!(matches!(created.outcome, DataOutcomeV1::Ok { .. }));
    let state = invocation
        .state
        .get_tenant_entity_state(&tenant, "Customer", id)
        .await
        .expect("created actor state");
    let precondition =
        crate::entity_actor::effects::entity_authorization_precondition(&state.state);
    store.restore_faults(faults);

    let response = invocation
        .state
        .delete_tenant_entity_if_current(&tenant, "Customer", id, precondition)
        .await
        .expect("actor must return the structural failure response");
    assert!(!response.success);
    assert_eq!(response.failure_outcome, Some(expected));
    assert_eq!(response.state.status, "Deleted");
    assert_eq!(response.state.sequence_nr, 2);

    store.restore_faults(temper_store_sim::SimFaultConfig::none());
    let replayed = invocation
        .state
        .get_tenant_entity_state(&tenant, "Customer", id)
        .await
        .expect("live actor remains readable for verification");
    assert_eq!(replayed.state.status, "Deleted");
    assert_eq!(replayed.state.sequence_nr, 2);
    let journal = store.dump_journal(&format!("default:Customer:{id}"));
    assert_eq!(journal.len(), 2);
    assert_eq!(journal.last().unwrap().event_type, "Deleted");
}

#[tokio::test]
async fn delete_post_commit_and_acknowledgement_loss_reconcile_live_state() {
    assert_delete_fault(
        temper_store_sim::SimFaultConfig {
            append_post_commit_failure_prob: 1.0,
            ..temper_store_sim::SimFaultConfig::none()
        },
        temper_failure::FailureOutcome::Applied,
    )
    .await;
    assert_delete_fault(
        temper_store_sim::SimFaultConfig {
            append_acknowledgement_loss_prob: 1.0,
            ..temper_store_sim::SimFaultConfig::none()
        },
        temper_failure::FailureOutcome::Unknown,
    )
    .await;
}
