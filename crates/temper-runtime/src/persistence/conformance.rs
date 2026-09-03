//! Backend-neutral create-or-verify conformance oracle.

use super::*;

/// Build the canonical conformance request for a backend test namespace.
pub fn request(
    tenant: &str,
    entity_id: &str,
    idempotency_key: &str,
    binding: &str,
) -> CreateOrVerifyRequest {
    let persistence_id = format!("{tenant}:ConformanceCandidate:{entity_id}");
    CreateOrVerifyRequest {
        module_name: "conformance".into(),
        idempotency_key: idempotency_key.into(),
        first_event: FirstEventCommit {
            tenant: tenant.into(),
            entity_type: "ConformanceCandidate".into(),
            entity_id: entity_id.into(),
            persistence_id: persistence_id.clone(),
            event: PersistenceEnvelope {
                sequence_nr: 1,
                event_type: "Created".into(),
                payload: serde_json::json!({"Binding": binding}),
                metadata: EventMetadata {
                    event_id: uuid::Uuid::nil(),
                    causation_id: uuid::Uuid::nil(),
                    correlation_id: uuid::Uuid::nil(),
                    timestamp: chrono::DateTime::UNIX_EPOCH,
                    actor_id: persistence_id,
                    kernel: None,
                },
            },
            contract: CreationContract {
                version: CREATION_CONTRACT_VERSION_V1,
                schema_digest: "conformance-schema".into(),
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
            },
            contract_revision: CREATION_CONTRACT_VERSION_V1,
            schema_identity: "conformance-schema".into(),
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
                state: serde_json::json!({"status":"Ready","fields":{"Binding":binding}}),
                sequence_nr: 1,
            }),
        },
    }
}

fn request_for_type(
    tenant: &str,
    entity_type: &str,
    entity_id: &str,
    idempotency_key: &str,
    binding: &str,
) -> CreateOrVerifyRequest {
    let mut request = request(tenant, entity_id, idempotency_key, binding);
    request.entity_type = entity_type.to_string();
    request.persistence_id = format!("{tenant}:{entity_type}:{entity_id}");
    request.event.metadata.actor_id = request.persistence_id.clone();
    request
}

fn composite_request(
    tenant: &str,
    entity_id: &str,
    idempotency_key: &str,
    region: &str,
    alias: &str,
) -> CreateOrVerifyRequest {
    let mut request = request_for_type(
        tenant,
        "ConformanceComposite",
        entity_id,
        idempotency_key,
        "composite",
    );
    request.event.payload = serde_json::json!({"Region": region, "Alias": alias});
    request.contract.fields = vec![
        CreationContractField {
            name: "Alias".into(),
            type_descriptor: "Edm.String".into(),
            value_source: "stored_field".into(),
            nullable: false,
            create_required: Some(true),
            default_digest: String::new(),
            value_digest: alias.into(),
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
        CreationContractField {
            name: "Region".into(),
            type_descriptor: "Edm.String".into(),
            value_source: "stored_field".into(),
            nullable: false,
            create_required: Some(true),
            default_digest: String::new(),
            value_digest: region.into(),
        },
    ];
    request.contract.digest = format!("{entity_id}:{region}:{alias}");
    request.declared_key_signature = "v1:AliasKey,RegionKey".into();
    request.key_rows = vec![
        EntityKeyRow {
            key_name: "AliasKey".into(),
            key_hash: alias.into(),
        },
        EntityKeyRow {
            key_name: "RegionKey".into(),
            key_hash: region.into(),
        },
    ];
    request
}

/// Run the same creation, replay, mutation, ownership, and acknowledgement
/// oracle against any event-store backend.
pub async fn run(store: &impl EventStore, tenant: &str) -> Result<(), String> {
    let first = request(tenant, "candidate-1", "request-1", "binding-a");
    let created = store
        .create_or_verify(&first)
        .await
        .map_err(|e| e.to_string())?;
    assert!(matches!(
        created,
        CreateOrVerifyStoreOutcome::Created { sequence_nr: 1, .. }
    ));
    let replay = store
        .create_or_verify(&first)
        .await
        .map_err(|e| e.to_string())?;
    assert!(matches!(
        replay,
        CreateOrVerifyStoreOutcome::AlreadyMatches {
            sequence_nr: 1,
            notification_pending: true,
            ..
        }
    ));
    store
        .acknowledge_create_or_verify_notification(&first)
        .await
        .map_err(|e| e.to_string())?;
    assert!(matches!(
        store
            .create_or_verify(&first)
            .await
            .map_err(|e| e.to_string())?,
        CreateOrVerifyStoreOutcome::AlreadyMatches {
            sequence_nr: 1,
            notification_pending: false,
            ..
        }
    ));

    let alternate = request(tenant, "candidate-2", "request-2", "binding-a");
    assert_eq!(
        store
            .create_or_verify(&alternate)
            .await
            .map_err(|e| e.to_string())?,
        CreateOrVerifyStoreOutcome::AlreadyMatches {
            entity_id: "candidate-1".into(),
            sequence_nr: 1,
            notification_pending: false
        }
    );
    let divergent = request(tenant, "candidate-3", "request-3", "binding-b");
    assert!(matches!(
        store
            .create_or_verify(&divergent)
            .await
            .map_err(|e| e.to_string())?,
        CreateOrVerifyStoreOutcome::Created { .. }
    ));
    let reused = request(tenant, "candidate-4", "request-3", "binding-b");
    assert!(matches!(
        store
            .create_or_verify(&reused)
            .await
            .map_err(|e| e.to_string())?,
        CreateOrVerifyStoreOutcome::Conflict { .. }
    ));

    let mut update = first.event.clone();
    update.sequence_nr = 2;
    update.event_type = "Updated".into();
    store
        .append_with_index_rows(
            &first.persistence_id,
            1,
            &[update],
            &first.key_rows,
            &[],
            false,
        )
        .await
        .map_err(|e| e.to_string())?;
    assert!(matches!(
        store
            .create_or_verify(&first)
            .await
            .map_err(|e| e.to_string())?,
        CreateOrVerifyStoreOutcome::AlreadyMatches { sequence_nr: 1, .. }
    ));

    let ordinary = request(tenant, "ordinary", "ordinary-request", "ordinary-binding");
    assert_eq!(
        store
            .commit_first_event(&ordinary.first_event)
            .await
            .map_err(|e| e.to_string())?,
        1
    );
    assert!(matches!(
        store
            .create_or_verify(&ordinary)
            .await
            .map_err(|e| e.to_string())?,
        CreateOrVerifyStoreOutcome::AlreadyMatches { sequence_nr: 1, .. }
    ));

    let concurrent_a = request(
        tenant,
        "concurrent-a",
        "concurrent-request-a",
        "binding-race",
    );
    let concurrent_b = request(
        tenant,
        "concurrent-b",
        "concurrent-request-b",
        "binding-race",
    );
    let (left, right) = tokio::join!(
        store.create_or_verify(&concurrent_a),
        store.create_or_verify(&concurrent_b)
    );
    let outcomes = [
        left.map_err(|e| e.to_string())?,
        right.map_err(|e| e.to_string())?,
    ];
    assert_eq!(
        outcomes
            .iter()
            .filter(|outcome| matches!(outcome, CreateOrVerifyStoreOutcome::Created { .. }))
            .count(),
        1
    );
    assert!(
        outcomes
            .iter()
            .any(|outcome| matches!(outcome, CreateOrVerifyStoreOutcome::AlreadyMatches { .. }))
    );

    let released_owner = request(
        tenant,
        "released-owner",
        "released-owner-request",
        "released",
    );
    store
        .create_or_verify(&released_owner)
        .await
        .map_err(|e| e.to_string())?;
    let mut release_event = released_owner.event.clone();
    release_event.sequence_nr = 2;
    release_event.event_type = "Deleted".into();
    store
        .append_with_index_rows(
            &released_owner.persistence_id,
            1,
            &[release_event],
            &[EntityKeyRow {
                key_name: "BindingKey".into(),
                key_hash: "released-away".into(),
            }],
            &[],
            false,
        )
        .await
        .map_err(|e| e.to_string())?;
    let replacement = request(tenant, "replacement", "replacement-request", "released");
    assert!(matches!(
        store
            .create_or_verify(&replacement)
            .await
            .map_err(|e| e.to_string())?,
        CreateOrVerifyStoreOutcome::Created { .. }
    ));
    let tombstoned_identity = request(
        tenant,
        "released-owner",
        "tombstoned-identity-request",
        "different-binding",
    );
    assert!(matches!(
        store
            .create_or_verify(&tombstoned_identity)
            .await
            .map_err(|e| e.to_string())?,
        CreateOrVerifyStoreOutcome::Conflict { .. }
    ));

    let mut incompatible = first.clone();
    incompatible.idempotency_key = "incompatible-request".into();
    incompatible.contract.schema_digest = "incompatible-schema".into();
    incompatible.first_event.schema_identity = "incompatible-schema".into();
    incompatible.contract.fields[0].type_descriptor = "Edm.Int64".into();
    incompatible.contract.digest = "incompatible-contract".into();
    assert!(matches!(
        store
            .create_or_verify(&incompatible)
            .await
            .map_err(|e| e.to_string())?,
        CreateOrVerifyStoreOutcome::CreationContractMigrationRequired
            | CreateOrVerifyStoreOutcome::Conflict { .. }
    ));

    let composite_a = composite_request(
        tenant,
        "composite-a",
        "composite-request-a",
        "west",
        "alpha",
    );
    let composite_b =
        composite_request(tenant, "composite-b", "composite-request-b", "east", "beta");
    assert!(matches!(
        store
            .create_or_verify(&composite_a)
            .await
            .map_err(|e| e.to_string())?,
        CreateOrVerifyStoreOutcome::Created { .. }
    ));
    assert!(matches!(
        store
            .create_or_verify(&composite_b)
            .await
            .map_err(|e| e.to_string())?,
        CreateOrVerifyStoreOutcome::Created { .. }
    ));
    let split_owner =
        composite_request(tenant, "composite-c", "composite-request-c", "west", "beta");
    assert!(matches!(
        store
            .create_or_verify(&split_owner)
            .await
            .map_err(|e| e.to_string())?,
        CreateOrVerifyStoreOutcome::Conflict { .. }
    ));

    let legacy = request_for_type(
        tenant,
        "ConformanceLegacy",
        "legacy-a",
        "legacy-request",
        "legacy-binding",
    );
    store
        .append(
            &legacy.persistence_id,
            0,
            std::slice::from_ref(&legacy.event),
        )
        .await
        .map_err(|e| e.to_string())?;
    assert_eq!(
        store
            .create_or_verify(&legacy)
            .await
            .map_err(|e| e.to_string())?,
        CreateOrVerifyStoreOutcome::CreationContractMigrationRequired
    );
    store
        .reconcile_creation_metadata(&CreationMetadataRepair {
            first_event: legacy.first_event.clone(),
            source_sequence: 1,
        })
        .await
        .map_err(|e| e.to_string())?;
    store
        .publish_creation_coverage(&CreationCoveragePublication {
            tenant: legacy.tenant.clone(),
            entity_type: legacy.entity_type.clone(),
            metadata: FirstEventMetadata {
                contract: legacy.contract.clone(),
                contract_revision: legacy.contract_revision,
                schema_identity: legacy.schema_identity.clone(),
                declared_key_signature: legacy.declared_key_signature.clone(),
            },
            cursor: legacy.entity_id.clone(),
            source_write_version: 1,
        })
        .await
        .map_err(|e| e.to_string())?;
    assert!(matches!(
        store
            .create_or_verify(&legacy)
            .await
            .map_err(|e| e.to_string())?,
        CreateOrVerifyStoreOutcome::AlreadyMatches { sequence_nr: 1, .. }
    ));
    Ok(())
}
