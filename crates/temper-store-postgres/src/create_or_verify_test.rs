use sqlx::PgPool;
use temper_runtime::persistence::{
    CREATION_CONTRACT_VERSION_V1, CreateOrVerifyRequest, CreateOrVerifyStoreOutcome,
    CreationContract, CreationContractField, EntityKeyRow, EventMetadata, EventStore,
    FirstEventCommit, FirstEventProjection, PersistenceEnvelope,
};

use super::PostgresEventStore;
use crate::migration::run_migrations;

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

#[test]
fn create_replay_and_alternate_owner_match_atomically() {
    let Ok(database_url) = std::env::var("DATABASE_URL") else {
        return;
    };
    sqlx::test_block_on(async {
        let pool = PgPool::connect(&database_url).await.unwrap();
        run_migrations(&pool).await.unwrap();
        let store = PostgresEventStore::new(pool.clone());
        let tenant = format!("create-or-verify-{}", uuid::Uuid::new_v4());
        let first = request(&tenant, "candidate-1", "request-1", "binding-a");
        assert!(matches!(
            store.create_or_verify(&first).await.unwrap(),
            CreateOrVerifyStoreOutcome::Created { sequence_nr: 1, .. }
        ));
        let projected: (String, serde_json::Value, i64) = sqlx::query_as(
            "SELECT status, fields, sequence_nr FROM entity_catalog
             WHERE tenant=$1 AND entity_type='Candidate' AND entity_id='candidate-1'",
        )
        .bind(&tenant)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(projected.0, "Ready");
        assert_eq!(projected.1["Binding"], "binding-a");
        assert_eq!(projected.2, 1);
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

        sqlx::query("DELETE FROM entity_create_or_verify_idempotency WHERE tenant=$1")
            .bind(&tenant)
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("DELETE FROM entity_creation_coverage WHERE tenant=$1")
            .bind(&tenant)
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("DELETE FROM entity_creation_contracts WHERE tenant=$1")
            .bind(&tenant)
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("DELETE FROM entity_key_index WHERE tenant=$1")
            .bind(&tenant)
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("DELETE FROM entity_field_index WHERE tenant=$1")
            .bind(&tenant)
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("DELETE FROM entity_catalog WHERE tenant=$1")
            .bind(&tenant)
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("DELETE FROM events WHERE tenant=$1")
            .bind(&tenant)
            .execute(&pool)
            .await
            .unwrap();
    });
}

#[test]
fn backend_neutral_create_or_verify_conformance() {
    let Ok(database_url) = std::env::var("DATABASE_URL") else {
        return;
    };
    sqlx::test_block_on(async {
        let pool = PgPool::connect(&database_url).await.unwrap();
        run_migrations(&pool).await.unwrap();
        let store = PostgresEventStore::new(pool);
        let tenant = format!("pg-conformance-{}", uuid::Uuid::new_v4());
        temper_runtime::persistence::conformance::run(&store, &tenant)
            .await
            .unwrap();
    });
}
