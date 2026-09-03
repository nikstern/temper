use temper_runtime::persistence::{
    CREATION_CONTRACT_VERSION_V1, CreateOrVerifyRequest, CreateOrVerifyStoreOutcome,
    CreationContract, CreationContractField, EntityKeyRow, EventMetadata, EventStore,
    FirstEventCommit, FirstEventMetadata, FirstEventProjection, PersistenceAppend,
    PersistenceEnvelope,
};

use super::make_store;

fn request(tenant: &str, entity_id: &str, key: &str, binding: &str) -> CreateOrVerifyRequest {
    let persistence_id = format!("{tenant}:Candidate:{entity_id}");
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
        idempotency_key: key.into(),
        first_event: FirstEventCommit {
            tenant: tenant.into(),
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
    let Some(store) = make_store().await else {
        return;
    };
    let tenant = format!("redis-conformance-{}", uuid::Uuid::new_v4());
    temper_runtime::persistence::conformance::run(&store, &tenant)
        .await
        .unwrap();
}

#[tokio::test]
async fn batch_first_event_uses_the_shared_sequence_one_mutation() {
    let Some(store) = make_store().await else {
        return;
    };
    let tenant = format!("batch-first-{}", uuid::Uuid::new_v4());
    let first = request(&tenant, "candidate-1", "unused", "binding-a");
    let result = store
        .append_batch(&[PersistenceAppend {
            persistence_id: first.persistence_id.clone(),
            expected_sequence: 0,
            events: vec![first.event.clone()],
            key_rows: first.key_rows.clone(),
            vector_rows: Vec::new(),
            reconcile_vectors: false,
            first_event: Some(FirstEventMetadata {
                contract: first.contract.clone(),
                contract_revision: first.contract_revision,
                schema_identity: first.schema_identity.clone(),
                declared_key_signature: first.declared_key_signature.clone(),
            }),
        }])
        .await
        .unwrap();
    assert_eq!(result[0].sequence_nr, 1);
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
async fn multi_entity_batch_update_preserves_creation_key_ownership() {
    let Some(store) = make_store().await else {
        return;
    };
    let tenant = format!("batch-update-{}", uuid::Uuid::new_v4());
    let first = request(&tenant, "candidate-1", "unused", "binding-a");
    store
        .append_batch(&[PersistenceAppend {
            persistence_id: first.persistence_id.clone(),
            expected_sequence: 0,
            events: vec![first.event.clone()],
            key_rows: first.key_rows.clone(),
            vector_rows: Vec::new(),
            reconcile_vectors: false,
            first_event: Some(FirstEventMetadata {
                contract: first.contract.clone(),
                contract_revision: first.contract_revision,
                schema_identity: first.schema_identity.clone(),
                declared_key_signature: first.declared_key_signature.clone(),
            }),
        }])
        .await
        .unwrap();

    let unrelated = request(&tenant, "unrelated", "unused-2", "binding-b");
    store
        .append_batch(&[PersistenceAppend {
            persistence_id: unrelated.persistence_id.clone(),
            expected_sequence: 0,
            events: vec![unrelated.event.clone()],
            key_rows: unrelated.key_rows.clone(),
            vector_rows: Vec::new(),
            reconcile_vectors: false,
            first_event: Some(FirstEventMetadata {
                contract: unrelated.contract.clone(),
                contract_revision: unrelated.contract_revision,
                schema_identity: unrelated.schema_identity.clone(),
                declared_key_signature: unrelated.declared_key_signature.clone(),
            }),
        }])
        .await
        .unwrap();

    let mut update = first.event.clone();
    update.event_type = "Updated".into();
    let mut unrelated_update = unrelated.event.clone();
    unrelated_update.event_type = "Updated".into();
    store
        .append_batch(&[
            PersistenceAppend {
                persistence_id: first.persistence_id.clone(),
                expected_sequence: 1,
                events: vec![update],
                key_rows: Vec::new(),
                vector_rows: Vec::new(),
                reconcile_vectors: false,
                first_event: None,
            },
            PersistenceAppend {
                persistence_id: unrelated.persistence_id.clone(),
                expected_sequence: 1,
                events: vec![unrelated_update],
                key_rows: Vec::new(),
                vector_rows: Vec::new(),
                reconcile_vectors: false,
                first_event: None,
            },
        ])
        .await
        .unwrap();

    let alternate = request(&tenant, "candidate-2", "request-2", "binding-a");
    assert_eq!(
        store.create_or_verify(&alternate).await.unwrap(),
        CreateOrVerifyStoreOutcome::AlreadyMatches {
            entity_id: "candidate-1".into(),
            sequence_nr: 1,
            notification_pending: false,
        }
    );
}

#[tokio::test]
async fn create_replay_and_alternate_owner_match_atomically() {
    let Some(store) = make_store().await else {
        eprintln!("REDIS_URL not set, skipping test");
        return;
    };
    let tenant = format!("create-or-verify-{}", uuid::Uuid::new_v4());
    let first = request(&tenant, "candidate-1", "request-1", "binding-a");
    assert!(matches!(
        store.create_or_verify(&first).await.unwrap(),
        CreateOrVerifyStoreOutcome::Created { sequence_nr: 1, .. }
    ));
    let projected = store
        .load_query_projections(&tenant, "Candidate", &["candidate-1".to_string()])
        .await
        .unwrap();
    assert_eq!(projected.len(), 1);
    assert_eq!(projected[0].1.fields["Binding"], "binding-a");
    assert!(matches!(
        store.create_or_verify(&first).await.unwrap(),
        CreateOrVerifyStoreOutcome::AlreadyMatches { sequence_nr: 1, .. }
    ));
    let alternate = request(&tenant, "candidate-2", "request-2", "binding-a");
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
async fn idempotency_reuse_with_a_different_requested_identity_conflicts() {
    let Some(store) = make_store().await else {
        eprintln!("REDIS_URL not set, skipping test");
        return;
    };
    let tenant = format!("create-or-verify-{}", uuid::Uuid::new_v4());
    let first = request(&tenant, "candidate-1", "request-identity", "binding-a");
    store.create_or_verify(&first).await.unwrap();
    let reused = request(&tenant, "candidate-2", "request-identity", "binding-a");
    assert_eq!(
        store.create_or_verify(&reused).await.unwrap(),
        CreateOrVerifyStoreOutcome::Conflict {
            fields: vec!["Id".to_string()],
            truncated: false,
        }
    );
}
