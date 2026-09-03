use temper_runtime::persistence::{
    CREATION_CONTRACT_VERSION_V1, CreateOrVerifyRequest, CreateOrVerifyStoreOutcome,
    CreationContract, CreationContractField, EntityKeyRow, EventMetadata, EventStore,
    FirstEventCommit, FirstEventProjection, PersistenceEnvelope,
};

use super::make_store;

fn request(entity_id: &str, idempotency_key: &str, binding: &str) -> CreateOrVerifyRequest {
    let persistence_id = format!("default:Candidate:{entity_id}");
    let contract = CreationContract {
        version: CREATION_CONTRACT_VERSION_V1,
        schema_digest: "schema".into(),
        fields: vec![
            CreationContractField {
                name: "Binding".into(),
                type_descriptor: "Edm.String".into(),
                value_source: "stored_field".into(),
                nullable: false,
                create_required: Some(true),
                default_digest: String::new(),
                value_digest: binding.into(),
            },
            CreationContractField {
                name: "Id".into(),
                type_descriptor: "Edm.String".into(),
                value_source: "entity_id".into(),
                nullable: false,
                create_required: Some(true),
                default_digest: String::new(),
                value_digest: entity_id.into(),
            },
        ],
        digest: format!("{entity_id}:{binding}"),
    };
    CreateOrVerifyRequest {
        module_name: "worker".into(),
        idempotency_key: idempotency_key.into(),
        first_event: FirstEventCommit {
            tenant: "default".into(),
            entity_type: "Candidate".into(),
            entity_id: entity_id.into(),
            persistence_id: persistence_id.clone(),
            event: PersistenceEnvelope {
                sequence_nr: 1,
                event_type: "Created".into(),
                payload: serde_json::json!({"Binding": binding}),
                metadata: EventMetadata {
                    event_id: uuid::Uuid::new_v4(),
                    causation_id: uuid::Uuid::new_v4(),
                    correlation_id: uuid::Uuid::new_v4(),
                    timestamp: chrono::Utc::now(),
                    actor_id: persistence_id,
                    kernel: None,
                },
            },
            contract,
            contract_revision: CREATION_CONTRACT_VERSION_V1,
            schema_identity: "schema".into(),
            declared_key_signature: "v1:BindingKey".into(),
            key_rows: vec![EntityKeyRow {
                key_name: "BindingKey".into(),
                key_hash: binding.into(),
            }],
            vector_rows: Vec::new(),
            reconcile_vectors: false,
            projection: Some(FirstEventProjection {
                status: "Ready".into(),
                fields: serde_json::json!({"Binding": binding}),
                state: serde_json::json!({"status": "Ready", "fields": {"Binding": binding}}),
                sequence_nr: 1,
            }),
        },
    }
}

#[tokio::test]
async fn backend_neutral_create_or_verify_conformance() {
    let store = make_store("create-or-verify-conformance").await;
    temper_runtime::persistence::conformance::run(&store, "turso-conformance")
        .await
        .unwrap();
}

#[tokio::test]
async fn create_replay_and_alternate_owner_match_atomically() {
    let store = make_store("create-or-verify-parity").await;
    let first = request("candidate-1", "request-1", "binding-a");
    assert!(matches!(
        store.create_or_verify(&first).await.unwrap(),
        CreateOrVerifyStoreOutcome::Created { sequence_nr: 1, .. }
    ));
    let projected = store
        .load_entity_catalog_rows("default", "Candidate", &["candidate-1".to_string()])
        .await
        .expect("load co-committed projection");
    assert_eq!(projected.len(), 1);
    assert_eq!(projected[0].status, "Ready");
    assert_eq!(projected[0].fields["Binding"], "binding-a");
    assert_eq!(projected[0].sequence_nr, 1);
    assert!(matches!(
        store.create_or_verify(&first).await.unwrap(),
        CreateOrVerifyStoreOutcome::AlreadyMatches { sequence_nr: 1, .. }
    ));
    let alternate = request("candidate-2", "request-2", "binding-a");
    assert_eq!(
        store.create_or_verify(&alternate).await.unwrap(),
        CreateOrVerifyStoreOutcome::AlreadyMatches {
            entity_id: "candidate-1".into(),
            sequence_nr: 1,
            notification_pending: false,
        }
    );
    assert!(
        store
            .read_events(&alternate.persistence_id, 0)
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn divergent_owner_conflicts_without_a_second_stream() {
    let store = make_store("create-or-verify-conflict").await;
    let first = request("candidate-1", "request-1", "binding-a");
    store.create_or_verify(&first).await.unwrap();
    let mut divergent = request("candidate-1", "request-2", "binding-b");
    divergent.key_rows = first.key_rows.clone();
    assert!(matches!(
        store.create_or_verify(&divergent).await.unwrap(),
        CreateOrVerifyStoreOutcome::Conflict { .. }
    ));
    assert_eq!(
        store
            .read_events(&first.persistence_id, 0)
            .await
            .unwrap()
            .len(),
        1
    );
}

#[tokio::test]
async fn idempotency_reuse_with_a_different_requested_identity_conflicts() {
    let store = make_store("create-or-verify-id-reuse").await;
    let first = request("candidate-1", "request-identity", "binding-a");
    store.create_or_verify(&first).await.unwrap();
    let reused = request("candidate-2", "request-identity", "binding-a");
    assert_eq!(
        store.create_or_verify(&reused).await.unwrap(),
        CreateOrVerifyStoreOutcome::Conflict {
            fields: vec!["Id".to_string()],
            truncated: false,
        }
    );
}
