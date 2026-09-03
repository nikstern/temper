use super::*;

fn create_or_verify_request(
    entity_id: &str,
    idempotency_key: &str,
    binding: &str,
) -> CreateOrVerifyRequest {
    let persistence_id = format!("default:Candidate:{entity_id}");
    let mut event = test_envelope(1, "Created");
    event.metadata.actor_id = persistence_id.clone();
    let contract = CreationContract {
        version: CREATION_CONTRACT_VERSION_V1,
        schema_digest: "schema".to_string(),
        fields: vec![
            CreationContractField {
                name: "Binding".to_string(),
                type_descriptor: "Edm.String".to_string(),
                value_source: "stored_field".to_string(),
                nullable: false,
                create_required: Some(true),
                default_digest: String::new(),
                value_digest: binding.to_string(),
            },
            CreationContractField {
                name: "Id".to_string(),
                type_descriptor: "Edm.String".to_string(),
                value_source: "entity_id".to_string(),
                nullable: false,
                create_required: Some(true),
                default_digest: String::new(),
                value_digest: entity_id.to_string(),
            },
        ],
        digest: format!("{entity_id}:{binding}"),
    };
    CreateOrVerifyRequest {
        module_name: "worker".to_string(),
        idempotency_key: idempotency_key.to_string(),
        first_event: FirstEventCommit {
            tenant: "default".to_string(),
            entity_type: "Candidate".to_string(),
            entity_id: entity_id.to_string(),
            persistence_id,
            event,
            contract,
            contract_revision: CREATION_CONTRACT_VERSION_V1,
            schema_identity: "schema".to_string(),
            declared_key_signature: "v1:BindingKey".to_string(),
            key_rows: vec![EntityKeyRow {
                key_name: "BindingKey".to_string(),
                key_hash: binding.to_string(),
            }],
            vector_rows: Vec::new(),
            reconcile_vectors: false,
            projection: Some(FirstEventProjection {
                status: "Ready".to_string(),
                fields: serde_json::json!({"Binding": binding}),
                state: serde_json::json!({"status": "Ready", "fields": {"Binding": binding}}),
                sequence_nr: 1,
            }),
        },
    }
}

#[tokio::test]
async fn backend_neutral_create_or_verify_conformance() {
    let store = SimEventStore::no_faults(82);
    temper_runtime::persistence::conformance::run(&store, "sim-conformance")
        .await
        .unwrap();
}

#[tokio::test]
async fn create_or_verify_replay_converges_after_response_loss() {
    let store = SimEventStore::no_faults(82);
    let request = create_or_verify_request("candidate-1", "request-1", "binding-a");
    assert!(matches!(
        store.create_or_verify(&request).await.unwrap(),
        CreateOrVerifyStoreOutcome::Created { sequence_nr: 1, .. }
    ));
    assert!(matches!(
        store.create_or_verify(&request).await.unwrap(),
        CreateOrVerifyStoreOutcome::AlreadyMatches { sequence_nr: 1, .. }
    ));
    assert_eq!(
        store
            .read_events(&request.persistence_id, 0)
            .await
            .unwrap()
            .len(),
        1
    );
    let projection = store
        .dump_first_event_projection(&request.persistence_id)
        .expect("projection must be co-committed with the first event");
    assert_eq!(projection.status, "Ready");
    assert_eq!(projection.fields["Binding"], "binding-a");
}

#[tokio::test]
async fn idempotency_reuse_with_a_different_requested_identity_conflicts() {
    let store = SimEventStore::no_faults(82);
    let first = create_or_verify_request("candidate-1", "request-identity", "binding-a");
    store.create_or_verify(&first).await.unwrap();
    let reused = create_or_verify_request("candidate-2", "request-identity", "binding-a");
    assert_eq!(
        store.create_or_verify(&reused).await.unwrap(),
        CreateOrVerifyStoreOutcome::Conflict {
            fields: vec!["Id".to_string()],
            truncated: false,
        }
    );
}

#[tokio::test]
async fn create_or_verify_committed_reply_loss_retries_as_already_matches() {
    let store = SimEventStore::new(
        82,
        SimFaultConfig {
            create_or_verify_reply_loss_prob: 1.0,
            ..SimFaultConfig::none()
        },
    );
    let request = create_or_verify_request("candidate-1", "request-1", "binding-a");
    assert!(store.create_or_verify(&request).await.is_err());
    store.disable_faults();
    assert!(matches!(
        store.create_or_verify(&request).await.unwrap(),
        CreateOrVerifyStoreOutcome::AlreadyMatches { sequence_nr: 1, .. }
    ));
    assert_eq!(
        store
            .read_events(&request.persistence_id, 0)
            .await
            .unwrap()
            .len(),
        1
    );
}

#[tokio::test]
async fn create_or_verify_seeded_concurrent_writers_converge_or_conflict() {
    for seed in 1..=128 {
        let store = SimEventStore::no_faults(seed);
        let first = create_or_verify_request("candidate-a", "request-a", "binding-a");
        let mut second = create_or_verify_request("candidate-b", "request-b", "binding-a");
        if seed % 2 == 0 {
            second.contract.fields[0].value_digest = "different".to_string();
            second.contract.digest = "different-contract".to_string();
        }
        let (left, right) = if seed % 3 == 0 {
            let (right, left) = tokio::join!(
                store.create_or_verify(&second),
                store.create_or_verify(&first)
            );
            (left.unwrap(), right.unwrap())
        } else {
            let (left, right) = tokio::join!(
                store.create_or_verify(&first),
                store.create_or_verify(&second)
            );
            (left.unwrap(), right.unwrap())
        };
        let outcomes = [left, right];
        assert_eq!(
            outcomes
                .iter()
                .filter(|outcome| matches!(outcome, CreateOrVerifyStoreOutcome::Created { .. }))
                .count(),
            1,
            "seed {seed}"
        );
        if seed % 2 == 0 {
            assert!(
                outcomes
                    .iter()
                    .any(|outcome| matches!(outcome, CreateOrVerifyStoreOutcome::Conflict { .. }))
            );
        } else {
            assert!(outcomes.iter().any(|outcome| matches!(
                outcome,
                CreateOrVerifyStoreOutcome::AlreadyMatches { .. }
            )));
        }
    }
}

#[tokio::test]
async fn create_or_verify_alternate_key_owner_can_match_with_different_id() {
    let store = SimEventStore::no_faults(82);
    let first = create_or_verify_request("candidate-1", "request-1", "binding-a");
    store.create_or_verify(&first).await.unwrap();
    let alternate = create_or_verify_request("candidate-2", "request-2", "binding-a");
    assert_eq!(
        store.create_or_verify(&alternate).await.unwrap(),
        CreateOrVerifyStoreOutcome::AlreadyMatches {
            entity_id: "candidate-1".to_string(),
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
async fn ordinary_create_cannot_publish_a_new_signature_for_a_nonempty_type() {
    let store = SimEventStore::no_faults(82);
    let first = create_or_verify_request("candidate-1", "request-1", "binding-a");
    store.create_or_verify(&first).await.unwrap();

    let mut second = create_or_verify_request("candidate-2", "request-2", "binding-b");
    second.declared_key_signature = "v2:BindingKey".to_string();
    store.commit_first_event(&second.first_event).await.unwrap();

    assert_eq!(
        store.create_or_verify(&second).await.unwrap(),
        CreateOrVerifyStoreOutcome::CreationContractMigrationRequired
    );
}

#[tokio::test]
async fn idempotency_replay_survives_compatible_schema_publication() {
    let store = SimEventStore::no_faults(82);
    let original = create_or_verify_request("candidate-1", "request-1", "binding-a");
    store.create_or_verify(&original).await.unwrap();

    let mut published = original.clone();
    published.schema_identity = "schema-v2".to_string();
    published.contract.schema_digest = "schema-v2".to_string();
    published.contract.fields.push(CreationContractField {
        name: "OptionalLabel".to_string(),
        type_descriptor: "Edm.String".to_string(),
        value_source: "stored_field".to_string(),
        nullable: false,
        create_required: Some(false),
        default_digest: "label-default".to_string(),
        value_digest: "label-default".to_string(),
    });
    published.contract.digest = "schema-v2-contract".to_string();
    published.declared_key_signature = "v2:BindingKey".to_string();
    store
        .reconcile_creation_metadata(&CreationMetadataRepair {
            first_event: published.first_event.clone(),
            source_sequence: 1,
        })
        .await
        .unwrap();
    store
        .publish_creation_coverage(&CreationCoveragePublication {
            tenant: published.tenant.clone(),
            entity_type: published.entity_type.clone(),
            metadata: FirstEventMetadata {
                contract: published.contract.clone(),
                contract_revision: published.contract_revision,
                schema_identity: published.schema_identity.clone(),
                declared_key_signature: published.declared_key_signature.clone(),
            },
            cursor: published.entity_id.clone(),
            source_write_version: 1,
        })
        .await
        .unwrap();

    assert!(matches!(
        store.create_or_verify(&published).await.unwrap(),
        CreateOrVerifyStoreOutcome::AlreadyMatches { sequence_nr: 1, .. }
    ));
}

#[tokio::test]
async fn create_or_verify_divergent_owner_and_idempotency_reuse_conflict() {
    let store = SimEventStore::no_faults(82);
    let first = create_or_verify_request("candidate-1", "request-1", "binding-a");
    store.create_or_verify(&first).await.unwrap();

    let divergent_owner = create_or_verify_request("candidate-2", "request-2", "binding-b");
    let mut identity_conflict = divergent_owner.clone();
    identity_conflict.entity_id = "candidate-1".to_string();
    identity_conflict.persistence_id = "default:Candidate:candidate-1".to_string();
    identity_conflict.event.metadata.actor_id = identity_conflict.persistence_id.clone();
    assert!(matches!(
        store.create_or_verify(&identity_conflict).await.unwrap(),
        CreateOrVerifyStoreOutcome::Conflict { .. }
    ));

    let reused = create_or_verify_request("candidate-1", "request-1", "binding-b");
    assert!(matches!(
        store.create_or_verify(&reused).await.unwrap(),
        CreateOrVerifyStoreOutcome::Conflict { .. }
    ));
}

#[tokio::test]
async fn legacy_stream_fails_closed_until_stable_creation_reconciliation() {
    let store = SimEventStore::no_faults(82);
    let request = create_or_verify_request("candidate-1", "request-1", "binding-a");
    store
        .append(
            &request.persistence_id,
            0,
            std::slice::from_ref(&request.event),
        )
        .await
        .unwrap();
    assert_eq!(
        store.create_or_verify(&request).await.unwrap(),
        CreateOrVerifyStoreOutcome::CreationContractMigrationRequired
    );

    store
        .reconcile_creation_metadata(&CreationMetadataRepair {
            first_event: request.first_event.clone(),
            source_sequence: 1,
        })
        .await
        .unwrap();
    store
        .publish_creation_coverage(&CreationCoveragePublication {
            tenant: request.tenant.clone(),
            entity_type: request.entity_type.clone(),
            metadata: FirstEventMetadata {
                contract: request.contract.clone(),
                contract_revision: request.contract_revision,
                schema_identity: request.schema_identity.clone(),
                declared_key_signature: request.declared_key_signature.clone(),
            },
            cursor: request.entity_id.clone(),
            source_write_version: 1,
        })
        .await
        .unwrap();
    assert!(matches!(
        store.create_or_verify(&request).await.unwrap(),
        CreateOrVerifyStoreOutcome::AlreadyMatches { sequence_nr: 1, .. }
    ));
}

#[tokio::test]
async fn reconciliation_rejects_a_stale_source_write_version_atomically() {
    let store = SimEventStore::no_faults(82);
    let request = create_or_verify_request("candidate-1", "request-1", "binding-a");
    store
        .append(
            &request.persistence_id,
            0,
            std::slice::from_ref(&request.event),
        )
        .await
        .unwrap();
    store
        .append(
            &request.persistence_id,
            1,
            &[test_envelope(0, "BindingChanged")],
        )
        .await
        .unwrap();
    assert!(matches!(
        store
            .reconcile_creation_metadata(&CreationMetadataRepair {
                first_event: request.first_event.clone(),
                source_sequence: 1,
            })
            .await,
        Err(PersistenceError::ConcurrencyViolation {
            expected: 1,
            actual: 2
        })
    ));
    assert!(matches!(
        store.create_or_verify(&request).await.unwrap(),
        CreateOrVerifyStoreOutcome::CreationContractMigrationRequired
    ));

    store
        .reconcile_creation_metadata(&CreationMetadataRepair {
            first_event: request.first_event.clone(),
            source_sequence: 2,
        })
        .await
        .unwrap();
    let stale_publication = CreationCoveragePublication {
        tenant: request.tenant.clone(),
        entity_type: request.entity_type.clone(),
        metadata: FirstEventMetadata {
            contract: request.contract.clone(),
            contract_revision: request.contract_revision,
            schema_identity: request.schema_identity.clone(),
            declared_key_signature: request.declared_key_signature.clone(),
        },
        cursor: request.entity_id.clone(),
        source_write_version: 1,
    };
    assert!(matches!(
        store.publish_creation_coverage(&stale_publication).await,
        Err(PersistenceError::ConcurrencyViolation {
            expected: 1,
            actual: 2
        })
    ));
}
