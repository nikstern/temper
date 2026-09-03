use std::collections::{BTreeMap, BTreeSet};

use sqlx::{Acquire, Row};
use temper_runtime::persistence::{
    CreateOrVerifyRequest, CreateOrVerifyStoreOutcome, CreationContract,
    CreationContractComparison, FirstEventCommit, PersistenceError, compare_creation_contracts,
    compare_creation_contracts_for_alternate_owner,
};

use crate::PostgresEventStore;

pub(crate) async fn run(
    store: &PostgresEventStore,
    request: &CreateOrVerifyRequest,
) -> Result<CreateOrVerifyStoreOutcome, PersistenceError> {
    request.first_event.validate()?;
    let mut connection = store.pool().acquire().await.map_err(storage)?;
    let mut tx = connection.begin().await.map_err(storage)?;

    acquire_creation_locks(&mut tx, &request.first_event, Some(request)).await?;

    if !coverage_is_current(&mut tx, request).await? {
        let type_has_events: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM events WHERE tenant = $1 AND entity_type = $2)",
        )
        .bind(&request.tenant)
        .bind(&request.entity_type)
        .fetch_one(&mut *tx)
        .await
        .map_err(storage)?;
        if type_has_events {
            tx.commit().await.map_err(storage)?;
            return Ok(CreateOrVerifyStoreOutcome::CreationContractMigrationRequired);
        }
    }

    let replay = sqlx::query(
        "SELECT entity_id, requested_entity_id, requested_contract_json, notification_pending
         FROM entity_create_or_verify_idempotency
         WHERE tenant = $1 AND module_name = $2 AND entity_type = $3 AND idempotency_key = $4
         FOR UPDATE",
    )
    .bind(&request.tenant)
    .bind(&request.module_name)
    .bind(&request.entity_type)
    .bind(&request.idempotency_key)
    .fetch_optional(&mut *tx)
    .await
    .map_err(storage)?;
    if let Some(row) = replay {
        let entity_id: String = row.try_get("entity_id").map_err(storage)?;
        let requested_entity_id: String = row.try_get("requested_entity_id").map_err(storage)?;
        let original: CreationContract =
            serde_json::from_value(row.try_get("requested_contract_json").map_err(storage)?)
                .map_err(|error| PersistenceError::Serialization(error.to_string()))?;
        let notification_pending: bool = row.try_get("notification_pending").map_err(storage)?;
        if requested_entity_id != request.entity_id {
            tx.commit().await.map_err(storage)?;
            return Ok(CreateOrVerifyStoreOutcome::Conflict {
                fields: vec!["Id".to_string()],
                truncated: false,
            });
        }
        match compare_creation_contracts(&original, &request.contract) {
            CreationContractComparison::Matches => {}
            CreationContractComparison::Conflict { fields, truncated } => {
                tx.commit().await.map_err(storage)?;
                return Ok(CreateOrVerifyStoreOutcome::Conflict { fields, truncated });
            }
            CreationContractComparison::MigrationRequired => {
                tx.commit().await.map_err(storage)?;
                return Ok(CreateOrVerifyStoreOutcome::CreationContractMigrationRequired);
            }
        }
        let Some(stored) = load_contract(&mut tx, request, &entity_id).await? else {
            tx.commit().await.map_err(storage)?;
            return Ok(CreateOrVerifyStoreOutcome::CreationContractMigrationRequired);
        };
        let mut outcome = compare_existing(
            &mut tx,
            request,
            &entity_id,
            &stored,
            requested_entity_id != entity_id,
        )
        .await?;
        if let CreateOrVerifyStoreOutcome::AlreadyMatches {
            notification_pending: pending,
            ..
        } = &mut outcome
        {
            *pending = notification_pending;
        }
        tx.commit().await.map_err(storage)?;
        return Ok(outcome);
    }

    let mut owners = BTreeMap::<String, BTreeSet<String>>::new();
    let requested_exists: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM events WHERE tenant = $1 AND entity_type = $2 AND entity_id = $3)",
    )
    .bind(&request.tenant)
    .bind(&request.entity_type)
    .bind(&request.entity_id)
    .fetch_one(&mut *tx)
    .await
    .map_err(storage)?;
    if requested_exists {
        owners
            .entry(request.entity_id.clone())
            .or_default()
            .insert("Id".to_string());
    }
    for key in &request.key_rows {
        if let Some(owner) = sqlx::query_scalar::<_, String>(
            "SELECT entity_id FROM entity_key_index
             WHERE tenant = $1 AND entity_type = $2 AND key_name = $3 AND key_hash = $4
             FOR UPDATE",
        )
        .bind(&request.tenant)
        .bind(&request.entity_type)
        .bind(&key.key_name)
        .bind(&key.key_hash)
        .fetch_optional(&mut *tx)
        .await
        .map_err(storage)?
        {
            owners
                .entry(owner)
                .or_default()
                .insert(key.key_name.clone());
        }
    }
    if owners.len() > 1 {
        let outcome = owner_conflict(&owners);
        tx.commit().await.map_err(storage)?;
        return Ok(outcome);
    }
    if let Some((entity_id, _)) = owners.first_key_value() {
        let Some(stored) = load_contract(&mut tx, request, entity_id).await? else {
            tx.commit().await.map_err(storage)?;
            return Ok(CreateOrVerifyStoreOutcome::CreationContractMigrationRequired);
        };
        let alternate_owner = !owners
            .get(entity_id)
            .is_some_and(|fields| fields.contains("Id"));
        let outcome =
            compare_existing(&mut tx, request, entity_id, &stored, alternate_owner).await?;
        if matches!(outcome, CreateOrVerifyStoreOutcome::AlreadyMatches { .. }) {
            insert_idempotency(&mut tx, request, entity_id, false).await?;
        }
        tx.commit().await.map_err(storage)?;
        return Ok(outcome);
    }

    insert_first_event(&mut tx, &request.first_event).await?;
    insert_idempotency(&mut tx, request, &request.entity_id, true).await?;
    tx.commit().await.map_err(storage)?;
    Ok(CreateOrVerifyStoreOutcome::Created {
        entity_id: request.entity_id.clone(),
        sequence_nr: 1,
    })
}

pub(crate) async fn commit_first_event(
    store: &PostgresEventStore,
    commit: &FirstEventCommit,
) -> Result<u64, PersistenceError> {
    commit.validate()?;
    let mut connection = store.pool().acquire().await.map_err(storage)?;
    let mut tx = connection.begin().await.map_err(storage)?;
    acquire_creation_locks(&mut tx, commit, None).await?;
    let current: i64 = sqlx::query_scalar(
        "SELECT COALESCE(MAX(sequence_nr), 0) FROM events
         WHERE tenant = $1 AND entity_type = $2 AND entity_id = $3",
    )
    .bind(&commit.tenant)
    .bind(&commit.entity_type)
    .bind(&commit.entity_id)
    .fetch_one(&mut *tx)
    .await
    .map_err(storage)?;
    if current != 0 {
        return Err(PersistenceError::ConcurrencyViolation {
            expected: 0,
            actual: u64::try_from(current).map_err(storage)?,
        });
    }
    insert_first_event(&mut tx, commit).await?;
    tx.commit().await.map_err(storage)?;
    Ok(1)
}

async fn acquire_creation_locks(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    commit: &FirstEventCommit,
    idempotency: Option<&CreateOrVerifyRequest>,
) -> Result<(), PersistenceError> {
    let mut lock_names = commit
        .key_rows
        .iter()
        .map(|row| {
            serde_json::to_string(&(
                "key",
                &commit.tenant,
                &commit.entity_type,
                &row.key_name,
                &row.key_hash,
            ))
            .expect("creation lock tuple must serialize")
        })
        .collect::<Vec<_>>();
    lock_names.push(
        serde_json::to_string(&("id", &commit.tenant, &commit.entity_type, &commit.entity_id))
            .expect("creation identity lock tuple must serialize"),
    );
    if let Some(request) = idempotency {
        lock_names.push(
            serde_json::to_string(&(
                "idempotency",
                &request.tenant,
                &request.entity_type,
                &request.module_name,
                &request.idempotency_key,
            ))
            .expect("creation idempotency lock tuple must serialize"),
        );
    }
    lock_names.sort();
    lock_names.dedup();
    for lock_name in lock_names {
        sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
            .bind(lock_name)
            .execute(&mut **tx)
            .await
            .map_err(storage)?;
    }
    Ok(())
}

async fn insert_first_event(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    commit: &FirstEventCommit,
) -> Result<(), PersistenceError> {
    crate::store::assert_scoped_journal_write_fence(
        tx,
        &commit.tenant,
        &commit.entity_type,
        &commit.entity_id,
        std::slice::from_ref(&commit.event),
    )
    .await?;
    crate::store::assert_unscoped_stream_publication_fence(
        tx,
        &commit.tenant,
        &commit.entity_type,
        &commit.entity_id,
        std::slice::from_ref(&commit.event),
    )
    .await?;
    let segment_index = crate::segments::open_segment_for_append(
        tx,
        &commit.tenant,
        &commit.entity_type,
        &commit.entity_id,
        0,
    )
    .await?;
    let metadata = serde_json::to_value(&commit.event.metadata)
        .map_err(|error| PersistenceError::Serialization(error.to_string()))?;
    sqlx::query(
        "INSERT INTO events
         (tenant, entity_type, entity_id, sequence_nr, segment_index, event_type, payload, metadata)
         VALUES ($1, $2, $3, 1, $4, $5, $6, $7)",
    )
    .bind(&commit.tenant)
    .bind(&commit.entity_type)
    .bind(&commit.entity_id)
    .bind(segment_index)
    .bind(&commit.event.event_type)
    .bind(&commit.event.payload)
    .bind(metadata)
    .execute(&mut **tx)
    .await
    .map_err(storage)?;
    crate::segments::update_segment_after_append(
        tx,
        &commit.tenant,
        &commit.entity_type,
        &commit.entity_id,
        segment_index,
        1,
    )
    .await?;
    insert_contract_and_keys(tx, commit).await?;
    insert_projection(tx, commit).await
}

mod comparison;
mod metadata;
mod repair;
pub(crate) use comparison::*;
pub(crate) use metadata::*;
pub(crate) use repair::*;
