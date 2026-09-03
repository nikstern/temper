use std::collections::BTreeSet;

use axum::body::Body;
use axum::http::Request;
use temper_authz::SecurityContext;
use temper_wasm_sdk::data::{
    CreateOrVerifyResultV1, DataOperationKind, DataOperationV1, DataOutcomeV1, DataResultV1,
    ModuleDataErrorKind,
};

use super::tests::{call, invocation, response_error};
use super::{ApplicationDataInvocation, ModuleInvocationAuthority};
use temper_runtime::TenantId;
use temper_runtime::persistence::{EventMetadata, EventStore, PersistenceEnvelope};
use temper_runtime::scheduler::{sim_now, sim_uuid};
use tower::ServiceExt;

mod durable_sse_tests;
mod response_reservation_tests;

fn durable_invocation(
    operations: BTreeSet<DataOperationKind>,
    security: SecurityContext,
) -> std::sync::Arc<ApplicationDataInvocation> {
    durable_invocation_with_store(
        operations,
        security,
        temper_store_sim::SimEventStore::no_faults(82),
    )
}

pub(super) fn durable_invocation_with_store(
    operations: BTreeSet<DataOperationKind>,
    security: SecurityContext,
    store: temper_store_sim::SimEventStore,
) -> std::sync::Arc<ApplicationDataInvocation> {
    let template = invocation(operations, security);
    let mut state = template.state.clone();
    state.set_storage_stack(crate::storage::StorageStack::from_sim(store, None));
    ApplicationDataInvocation::new(
        state,
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
    )
}

fn durable_invocation_with_response_budget(
    store: temper_store_sim::SimEventStore,
    max_response_bytes: u32,
) -> std::sync::Arc<ApplicationDataInvocation> {
    let template = invocation(
        BTreeSet::from([DataOperationKind::EntityCreateOrVerify]),
        SecurityContext::system(),
    );
    let mut state = template.state.clone();
    state.set_storage_stack(crate::storage::StorageStack::from_sim(store, None));
    let mut binding = template.authority.binding.clone();
    binding.grant.budgets.max_response_bytes = max_response_bytes;
    ApplicationDataInvocation::new(
        state,
        ModuleInvocationAuthority::new(
            template.authority.tenant.clone(),
            template.authority.module_name.clone(),
            template.authority.artifact_digest.clone(),
            template.authority.trigger.clone(),
            template.authority.triggering_entity_type.clone(),
            template.authority.security.clone(),
            binding,
            template.authority.target.clone(),
        ),
    )
}

#[tokio::test]
async fn committed_reply_loss_retries_after_server_restart() {
    let store = temper_store_sim::SimEventStore::new(
        82,
        temper_store_sim::SimFaultConfig {
            create_or_verify_reply_loss_prob: 1.0,
            ..temper_store_sim::SimFaultConfig::none()
        },
    );
    let operations = BTreeSet::from([DataOperationKind::EntityCreateOrVerify]);
    let id = "018f1f80-7b2d-7000-8000-000000000085";
    let persistence_id = format!("default:Customer:{id}");
    let first =
        durable_invocation_with_store(operations.clone(), SecurityContext::system(), store.clone());
    let mut first_changes = first.state.event_tx.subscribe();
    let response = call(&first, operation(id, "request-85", "Ada")).await;
    assert_eq!(response_error(response).kind, ModuleDataErrorKind::Internal);
    assert_eq!(store.dump_journal(&persistence_id).len(), 1);
    assert!(matches!(
        first_changes.try_recv(),
        Err(tokio::sync::broadcast::error::TryRecvError::Empty)
    ));

    let timestamp = sim_now();
    store
        .append(
            &persistence_id,
            1,
            &[PersistenceEnvelope {
                sequence_nr: 2,
                event_type: "Disable".into(),
                payload: serde_json::to_value(crate::entity_actor::EntityEvent {
                    action: "Disable".into(),
                    from_status: "Active".into(),
                    to_status: "Disabled".into(),
                    timestamp,
                    params: serde_json::json!({}),
                    idempotency_key: None,
                })
                .unwrap(),
                metadata: EventMetadata {
                    event_id: sim_uuid(),
                    causation_id: sim_uuid(),
                    correlation_id: sim_uuid(),
                    timestamp,
                    actor_id: persistence_id.clone(),
                    kernel: None,
                },
            }],
        )
        .await
        .unwrap();

    store.disable_faults();
    drop(first);
    let restarted =
        durable_invocation_with_store(operations, SecurityContext::system(), store.clone());
    let mut recovered_changes = restarted.state.event_tx.subscribe();
    let replay = call(&restarted, operation(id, "request-85", "Ada")).await;
    let DataOutcomeV1::Ok {
        result:
            DataResultV1::CreateOrVerify {
                outcome: CreateOrVerifyResultV1::AlreadyMatches { commit, value },
            },
    } = replay.outcome
    else {
        panic!("restart retry must match the committed entity: {replay:?}");
    };
    assert_eq!(commit.entity_id, id);
    assert_eq!(commit.sequence, 2);
    assert_eq!(value["Name"], "Ada");
    assert_eq!(value["Status"], "Disabled");
    let recovered = recovered_changes
        .recv()
        .await
        .expect("recovered Created notification");
    assert_eq!(recovered.entity_id, id);
    assert_eq!(recovered.action, "Created");
    assert_eq!(recovered.seq, 1);
    assert_eq!(recovered.status, "Active");
    let second_replay = call(&restarted, operation(id, "request-85", "Ada")).await;
    assert!(matches!(second_replay.outcome, DataOutcomeV1::Ok { .. }));
    assert!(matches!(
        recovered_changes.try_recv(),
        Err(tokio::sync::broadcast::error::TryRecvError::Empty)
    ));
    assert_eq!(store.dump_journal(&persistence_id).len(), 2);
    drop(restarted);

    let final_restart = durable_invocation_with_store(
        BTreeSet::from([DataOperationKind::EntityCreateOrVerify]),
        SecurityContext::system(),
        store,
    );
    let durable_replay = crate::events::replay_durable_entity_changes(
        &final_restart.state,
        "default",
        "Customer",
        id,
        0,
    )
    .await
    .unwrap();
    assert_eq!(durable_replay[0].action, "Created");
    assert_eq!(durable_replay[0].seq, 1);
}

pub(super) fn operation(id: &str, key: &str, name: &str) -> DataOperationV1 {
    DataOperationV1::EntityCreateOrVerify {
        entity_type: "Temper.Example.Customer".to_string(),
        idempotency_key: key.to_string(),
        value: serde_json::json!({"Id": id, "Name": name})
            .as_object()
            .cloned()
            .expect("fixture is an object"),
    }
}

#[tokio::test]
async fn created_request_replays_as_already_matches_with_authoritative_state() {
    let invocation = durable_invocation(
        BTreeSet::from([DataOperationKind::EntityCreateOrVerify]),
        SecurityContext::system(),
    );
    let mut changes = invocation.state.event_tx.subscribe();
    let id = "018f1f80-7b2d-7000-8000-000000000082";
    let first = call(&invocation, operation(id, "request-82", "Ada")).await;
    let DataOutcomeV1::Ok {
        result:
            DataResultV1::CreateOrVerify {
                outcome: CreateOrVerifyResultV1::Created { commit, value },
            },
    } = first.outcome
    else {
        panic!("first request must create: {first:?}");
    };
    assert_eq!(commit.entity_id, id);
    assert_eq!(commit.sequence, 1);
    assert_eq!(value["Name"], "Ada");
    let change = changes.recv().await.expect("Created notification");
    assert_eq!(change.entity_id, id);
    assert_eq!(change.action, "Created");

    let replay = call(&invocation, operation(id, "request-82", "Ada")).await;
    assert!(matches!(
        replay.outcome,
        DataOutcomeV1::Ok {
            result: DataResultV1::CreateOrVerify {
                outcome: CreateOrVerifyResultV1::AlreadyMatches { .. }
            }
        }
    ));
    assert!(matches!(
        changes.try_recv(),
        Err(tokio::sync::broadcast::error::TryRecvError::Empty)
    ));
}

#[tokio::test]
async fn create_projection_survives_restart_and_is_consumed_by_filtered_odata() {
    let store = temper_store_sim::SimEventStore::no_faults(82);
    let operations = BTreeSet::from([DataOperationKind::EntityCreateOrVerify]);
    let first =
        durable_invocation_with_store(operations.clone(), SecurityContext::system(), store.clone());
    let id = "018f1f80-7b2d-7000-8000-000000000091";
    assert!(matches!(
        call(&first, operation(id, "request-91", "Ada"))
            .await
            .outcome,
        DataOutcomeV1::Ok { .. }
    ));
    drop(first);

    let restarted =
        durable_invocation_with_store(operations, SecurityContext::system(), store.clone());
    let response =
        super::tests::authenticated_router(restarted.state.clone(), SecurityContext::system())
            .oneshot(
                Request::get("/tdata/Customers?$filter=Name%20eq%20%27Ada%27")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
    assert_eq!(response.status(), axum::http::StatusCode::OK);
    let bytes = axum::body::to_bytes(response.into_body(), 1024 * 1024)
        .await
        .unwrap();
    let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(body["value"].as_array().map(Vec::len), Some(1));
    assert_eq!(body["value"][0]["fields"]["Id"], id, "{body}");
    assert_eq!(body["value"][0]["fields"]["Name"], "Ada");
}

#[tokio::test]
async fn redis_create_projection_is_consumed_by_filtered_odata() {
    let Ok(redis_url) = std::env::var("REDIS_URL") else {
        return;
    };
    let store = temper_store_redis::RedisEventStore::new(&redis_url)
        .await
        .expect("Redis store");
    let operations = BTreeSet::from([DataOperationKind::EntityCreateOrVerify]);
    let template = invocation(operations, SecurityContext::system());
    let mut state = template.state.clone();
    state.set_storage_stack(crate::storage::StorageStack::from_redis(store));
    let invocation = ApplicationDataInvocation::new(
        state,
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
    let id = uuid::Uuid::new_v4().to_string();
    assert!(matches!(
        call(&invocation, operation(&id, &format!("redis-{id}"), "Ada"))
            .await
            .outcome,
        DataOutcomeV1::Ok { .. }
    ));
    let response =
        super::tests::authenticated_router(invocation.state.clone(), SecurityContext::system())
            .oneshot(
                Request::get("/tdata/Customers?$filter=Name%20eq%20%27Ada%27")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
    assert_eq!(response.status(), axum::http::StatusCode::OK);
    let bytes = axum::body::to_bytes(response.into_body(), 1024 * 1024)
        .await
        .unwrap();
    let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert!(body["value"].as_array().is_some_and(|rows| {
        rows.iter()
            .any(|row| row["fields"]["Id"] == id && row["fields"]["Name"] == "Ada")
    }));
}

#[tokio::test]
async fn divergent_request_and_idempotency_reuse_return_closed_conflicts() {
    let invocation = durable_invocation(
        BTreeSet::from([DataOperationKind::EntityCreateOrVerify]),
        SecurityContext::system(),
    );
    let id = "018f1f80-7b2d-7000-8000-000000000083";
    call(&invocation, operation(id, "request-83", "Ada")).await;
    for key in ["request-83", "request-84"] {
        let response = call(&invocation, operation(id, key, "Grace")).await;
        let DataOutcomeV1::Ok {
            result:
                DataResultV1::CreateOrVerify {
                    outcome: CreateOrVerifyResultV1::Conflict { fields, truncated },
                },
        } = response.outcome
        else {
            panic!("divergent request must conflict: {response:?}");
        };
        assert_eq!(fields, vec!["Name"]);
        assert!(!truncated);
    }
}

#[tokio::test]
async fn capability_and_idempotency_validation_fail_before_storage_resolution() {
    let denied = durable_invocation(BTreeSet::new(), SecurityContext::system());
    let denied_response = call(&denied, operation("missing", "request", "Ada")).await;
    assert_eq!(response_error(denied_response).code, "CapabilityDenied");

    let admitted = durable_invocation(
        BTreeSet::from([DataOperationKind::EntityCreateOrVerify]),
        SecurityContext::system(),
    );
    for key in [String::new(), "x".repeat(257)] {
        let response = call(&admitted, operation("candidate", &key, "Ada")).await;
        let error = response_error(response);
        assert_eq!(error.kind, ModuleDataErrorKind::InvalidRequest);
        assert_eq!(error.code, "InvalidIdentifier");
    }
}

#[tokio::test]
async fn cedar_authorizes_materialized_defaults_and_denies_before_lookup() {
    let security = SecurityContext::from_resolved_identity("user-82", "test-agent", None);
    let store = temper_store_sim::SimEventStore::no_faults(82);
    let invocation = durable_invocation_with_store(
        BTreeSet::from([DataOperationKind::EntityCreateOrVerify]),
        security,
        store.clone(),
    );
    invocation
        .state
        .authz
        .reload_tenant_policies(
            TenantId::default().as_str(),
            r#"permit(principal, action == Action::"create_or_verify", resource is Customer)
                when { resource.Label == "unknown" };"#,
        )
        .expect("install create-or-verify Cedar policy");
    let id = "018f1f80-7b2d-7000-8000-000000000086";
    let created = call(&invocation, operation(id, "request-86", "Ada")).await;
    assert!(matches!(
        created.outcome,
        DataOutcomeV1::Ok {
            result: DataResultV1::CreateOrVerify { .. }
        }
    ));

    invocation
        .state
        .authz
        .reload_tenant_policies(
            TenantId::default().as_str(),
            r#"permit(principal, action == Action::"read", resource is Customer);"#,
        )
        .expect("install restrictive Cedar policy");
    for candidate in [
        operation(id, "request-existing", "Ada"),
        operation(
            "018f1f80-7b2d-7000-8000-000000000087",
            "request-absent",
            "Ada",
        ),
    ] {
        let error = response_error(call(&invocation, candidate).await);
        assert_eq!(error.kind, ModuleDataErrorKind::AuthorizationDenied);
        assert_eq!(error.code, "AuthorizationDenied");
    }
    assert_eq!(
        store.dump_journal(&format!("default:Customer:{id}")).len(),
        1
    );
    assert!(
        store
            .dump_journal("default:Customer:018f1f80-7b2d-7000-8000-000000000087")
            .is_empty()
    );
}
