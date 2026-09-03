use std::collections::{BTreeMap, BTreeSet};

use temper_authz::SecurityContext;
use temper_runtime::persistence::{
    EventMetadata, EventStore, PersistenceAppend, PersistenceEnvelope,
};
use temper_runtime::scheduler::{sim_now, sim_uuid};
use temper_wasm_sdk::data::DataOperationKind;
use tokio_stream::StreamExt;

use super::durable_invocation_with_store;

#[tokio::test]
async fn durable_sse_replay_resolves_scoped_journal_and_skips_private_journal() {
    let store = temper_store_sim::SimEventStore::no_faults(82);
    let public_id = "018f1f80-7b2d-7000-8000-000000000086";
    let journal_id = "~task~scope-1~bundle-1~018f1f80-7b2d-7000-8000-000000000086";
    let mut request = temper_runtime::persistence::conformance::request(
        "default",
        public_id,
        "scoped-sse",
        "Ada",
    );
    request.entity_type = "Customer".into();
    request.entity_id = journal_id.into();
    request.persistence_id = format!("default:Customer:{journal_id}");
    request.event.metadata.actor_id = request.persistence_id.clone();
    request.event.payload = serde_json::to_value(crate::entity_actor::EntityEvent {
        action: "Created".into(),
        from_status: String::new(),
        to_status: "Active".into(),
        timestamp: sim_now(),
        params: serde_json::json!({"Id": public_id, "Name": "Ada"}),
        idempotency_key: None,
    })
    .unwrap();
    store
        .commit_first_event(&request.first_event)
        .await
        .unwrap();
    let private_id = "default:__reaction_delivery:private-delivery";
    let timestamp = sim_now();
    store
        .append(
            private_id,
            0,
            &[PersistenceEnvelope {
                sequence_nr: 1,
                event_type: "ReactionDelivery::Pending".into(),
                payload: serde_json::json!({"private": true}),
                metadata: EventMetadata {
                    event_id: sim_uuid(),
                    causation_id: sim_uuid(),
                    correlation_id: sim_uuid(),
                    timestamp,
                    actor_id: private_id.into(),
                    kernel: None,
                },
            }],
        )
        .await
        .unwrap();

    let invocation = durable_invocation_with_store(
        BTreeSet::from([DataOperationKind::EntityCreateOrVerify]),
        SecurityContext::system(),
        store,
    );
    let replay = crate::events::replay_durable_entity_changes(
        &invocation.state,
        "default",
        "Customer",
        public_id,
        0,
    )
    .await
    .unwrap();
    assert_eq!(replay.len(), 1);
    assert_eq!(replay[0].entity_id, public_id);
    assert_eq!(replay[0].action, "Created");
    assert_eq!(replay[0].seq, 1);
    let tenant_replay = crate::events::replay_durable_tenant_changes(
        &invocation.state,
        "default",
        Some("Customer"),
        None,
    )
    .await
    .unwrap();
    assert_eq!(tenant_replay.len(), 1);
    assert_eq!(tenant_replay[0].entity_id, public_id);
}

fn entity_envelope(sequence_nr: u64, persistence_id: &str) -> PersistenceEnvelope {
    let timestamp = sim_now();
    PersistenceEnvelope {
        sequence_nr,
        event_type: if sequence_nr == 1 {
            "Created"
        } else {
            "Update"
        }
        .into(),
        payload: serde_json::to_value(crate::entity_actor::EntityEvent {
            action: if sequence_nr == 1 {
                "Created"
            } else {
                "Update"
            }
            .into(),
            from_status: if sequence_nr == 1 { "" } else { "Active" }.into(),
            to_status: "Active".into(),
            timestamp,
            params: if sequence_nr == 1 {
                serde_json::json!({"Id": "lag-entity", "Name": "Ada"})
            } else {
                serde_json::json!({"Name": format!("Ada-{sequence_nr}")})
            },
            idempotency_key: None,
        })
        .unwrap(),
        metadata: EventMetadata {
            event_id: sim_uuid(),
            causation_id: sim_uuid(),
            correlation_id: sim_uuid(),
            timestamp,
            actor_id: persistence_id.into(),
            kernel: None,
        },
    }
}

#[tokio::test]
async fn broadcast_lag_recovers_from_the_durable_high_water_mark() {
    let store = temper_store_sim::SimEventStore::no_faults(82);
    let persistence_id = "default:Customer:lag-entity";
    let envelopes = (1..=3)
        .map(|sequence| entity_envelope(sequence, persistence_id))
        .collect::<Vec<_>>();
    store.append(persistence_id, 0, &envelopes).await.unwrap();
    let invocation = durable_invocation_with_store(
        BTreeSet::from([DataOperationKind::EntityCreateOrVerify]),
        SecurityContext::system(),
        store,
    );
    let (sender, receiver) = tokio::sync::broadcast::channel(1);
    let dummy = crate::events::EntityStateChange {
        seq: 99,
        entity_type: "Other".into(),
        entity_id: "other".into(),
        action: "Update".into(),
        status: "Active".into(),
        tenant: "default".into(),
        ..Default::default()
    };
    sender.send(dummy.clone()).unwrap();
    sender.send(dummy).unwrap();

    let stream = crate::events::durable_entity_change_stream(
        invocation.state.clone(),
        receiver,
        "default".into(),
        Some("Customer".into()),
        Some("lag-entity".into()),
        BTreeMap::from([(("Customer".into(), "lag-entity".into()), 1)]),
    );
    tokio::pin!(stream);
    assert_eq!(stream.next().await.unwrap().seq, 2);
    assert_eq!(stream.next().await.unwrap().seq, 3);
}

#[tokio::test]
async fn durable_replay_rejects_a_journal_beyond_the_event_budget() {
    let store = temper_store_sim::SimEventStore::no_faults(82);
    let persistence_id = "default:Customer:lag-entity";
    let envelopes = (1..=10_001)
        .map(|sequence| entity_envelope(sequence, persistence_id))
        .collect::<Vec<_>>();
    store.append(persistence_id, 0, &envelopes).await.unwrap();
    let invocation = durable_invocation_with_store(
        BTreeSet::from([DataOperationKind::EntityCreateOrVerify]),
        SecurityContext::system(),
        store,
    );
    let error = crate::events::replay_durable_entity_changes(
        &invocation.state,
        "default",
        "Customer",
        "lag-entity",
        0,
    )
    .await
    .unwrap_err();
    assert!(error.to_string().contains("event budget exhausted"));
}

#[tokio::test]
async fn scoped_resolution_rejects_a_scan_beyond_the_journal_budget() {
    let store = temper_store_sim::SimEventStore::no_faults(82);
    let appends = (0..10_001)
        .map(|index| {
            let persistence_id = format!("default:Customer:scoped-{index:05}");
            PersistenceAppend {
                persistence_id: persistence_id.clone(),
                expected_sequence: 0,
                events: vec![entity_envelope(1, &persistence_id)],
                key_rows: Vec::new(),
                vector_rows: Vec::new(),
                reconcile_vectors: false,
                first_event: None,
            }
        })
        .collect::<Vec<_>>();
    store.append_batch(&appends).await.unwrap();
    let invocation = durable_invocation_with_store(
        BTreeSet::from([DataOperationKind::EntityCreateOrVerify]),
        SecurityContext::system(),
        store,
    );
    let error = crate::events::replay_durable_entity_changes(
        &invocation.state,
        "default",
        "Customer",
        "missing-public-id",
        0,
    )
    .await
    .unwrap_err();
    assert!(error.to_string().contains("journal scan budget exhausted"));
    let tenant_error = crate::events::replay_durable_tenant_changes(
        &invocation.state,
        "default",
        Some("Customer"),
        None,
    )
    .await
    .unwrap_err();
    assert!(
        tenant_error
            .to_string()
            .contains("journal scan budget exhausted")
    );
}

#[tokio::test]
async fn tenant_replay_accepts_exactly_the_journal_scan_budget() {
    let store = temper_store_sim::SimEventStore::no_faults(82);
    let appends = (0..10_000)
        .map(|index| {
            let persistence_id = format!("default:Customer:scoped-{index:05}");
            PersistenceAppend {
                persistence_id: persistence_id.clone(),
                expected_sequence: 0,
                events: vec![entity_envelope(1, &persistence_id)],
                key_rows: Vec::new(),
                vector_rows: Vec::new(),
                reconcile_vectors: false,
                first_event: None,
            }
        })
        .collect::<Vec<_>>();
    store.append_batch(&appends).await.unwrap();
    let invocation = durable_invocation_with_store(
        BTreeSet::from([DataOperationKind::EntityCreateOrVerify]),
        SecurityContext::system(),
        store,
    );
    let replay = crate::events::replay_durable_tenant_changes(
        &invocation.state,
        "default",
        Some("Customer"),
        None,
    )
    .await
    .unwrap();
    assert_eq!(replay.len(), 1);
}
