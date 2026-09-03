use std::collections::BTreeSet;

use temper_authz::SecurityContext;
use temper_wasm_sdk::data::{DataOperationKind, DataOperationV1};

use super::create_or_verify_tests::durable_invocation_with_store;
use super::service::{ApplicationDataWriteError, GovernedApplicationDataService};
use super::tests::call;

#[tokio::test]
async fn unknown_create_reconciliation_read_failure_stays_unknown() {
    let store = temper_store_sim::SimEventStore::no_faults(9_303);
    let invocation = durable_invocation_with_store(
        BTreeSet::from([DataOperationKind::EntityCreate]),
        SecurityContext::system(),
        store.clone(),
    );
    let id = "018f1f80-7b2d-7000-8000-000000009303";
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
    assert!(matches!(
        created.outcome,
        temper_wasm_sdk::data::DataOutcomeV1::Ok { .. }
    ));
    store.fail_next_reads(&format!("default:Customer:{id}"), 1);

    let error = GovernedApplicationDataService::new(&invocation.state)
        .reconcile_unknown_create_failure(
            &temper_runtime::tenant::TenantId::default(),
            "Customer",
            id,
            None,
            "acknowledgement unavailable".into(),
        )
        .await;

    assert!(matches!(error, ApplicationDataWriteError::Unknown(_)));
}
