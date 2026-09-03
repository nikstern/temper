use std::collections::{BTreeMap, BTreeSet};

use libsql::{TransactionBehavior, Value, params, params_from_iter};
use temper_runtime::persistence::{
    CREATE_OR_VERIFY_CONFLICT_FIELD_BUDGET, CreateOrVerifyRequest, CreateOrVerifyStoreOutcome,
    CreationContract, CreationContractComparison, FirstEventCommit, PersistenceError,
    compare_creation_contracts, compare_creation_contracts_for_alternate_owner,
};

use super::TursoEventStore;

fn storage_error(error: impl std::fmt::Display) -> PersistenceError {
    PersistenceError::PreCommit(error.to_string())
}

fn acknowledgement_unknown(error: impl std::fmt::Display) -> PersistenceError {
    PersistenceError::AcknowledgementUnknown(error.to_string())
}

pub(super) async fn run(
    store: &TursoEventStore,
    request: &CreateOrVerifyRequest,
) -> Result<CreateOrVerifyStoreOutcome, PersistenceError> {
    request.first_event.validate()?;
    let _write_permit = store
        .acquire_write_permit(
            "turso.create_or_verify",
            super::write_gate::WritePriority::High,
        )
        .await?;
    let conn = store.configured_connection().await?;
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .await
        .map_err(storage_error)?;

    if !coverage_is_current(&tx, request).await? {
        let mut rows = tx
            .query(
                "SELECT 1 FROM events WHERE tenant = ?1 AND entity_type = ?2 LIMIT 1",
                params![request.tenant.as_str(), request.entity_type.as_str()],
            )
            .await
            .map_err(storage_error)?;
        if rows.next().await.map_err(storage_error)?.is_some() {
            drop(rows);
            tx.commit().await.map_err(acknowledgement_unknown)?;
            return Ok(CreateOrVerifyStoreOutcome::CreationContractMigrationRequired);
        }
    }

    let mut replay_rows = tx
        .query(
            "SELECT entity_id, requested_entity_id, requested_contract_json, notification_pending
             FROM entity_create_or_verify_idempotency
             WHERE tenant = ?1 AND module_name = ?2 AND entity_type = ?3
               AND idempotency_key = ?4",
            params![
                request.tenant.as_str(),
                request.module_name.as_str(),
                request.entity_type.as_str(),
                request.idempotency_key.as_str()
            ],
        )
        .await
        .map_err(storage_error)?;
    if let Some(row) = replay_rows.next().await.map_err(storage_error)? {
        let entity_id = row.get::<String>(0).map_err(storage_error)?;
        let requested_entity_id = row.get::<String>(1).map_err(storage_error)?;
        let original: CreationContract =
            serde_json::from_str(&row.get::<String>(2).map_err(storage_error)?)
                .map_err(|error| PersistenceError::Serialization(error.to_string()))?;
        let notification_pending = row.get::<i64>(3).map_err(storage_error)? != 0;
        drop(replay_rows);
        if requested_entity_id != request.entity_id {
            tx.commit().await.map_err(acknowledgement_unknown)?;
            return Ok(bounded_conflict(["Id".to_string()]));
        }
        match compare_creation_contracts(&original, &request.contract) {
            CreationContractComparison::Matches => {}
            CreationContractComparison::Conflict { fields, truncated } => {
                tx.commit().await.map_err(acknowledgement_unknown)?;
                return Ok(CreateOrVerifyStoreOutcome::Conflict { fields, truncated });
            }
            CreationContractComparison::MigrationRequired => {
                tx.commit().await.map_err(acknowledgement_unknown)?;
                return Ok(CreateOrVerifyStoreOutcome::CreationContractMigrationRequired);
            }
        }
        let Some(stored) = load_contract(&tx, request, &entity_id).await? else {
            tx.commit().await.map_err(acknowledgement_unknown)?;
            return Ok(CreateOrVerifyStoreOutcome::CreationContractMigrationRequired);
        };
        let mut outcome = compare_existing(
            &tx,
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
        tx.commit().await.map_err(acknowledgement_unknown)?;
        return Ok(outcome);
    }
    drop(replay_rows);

    let mut owners = BTreeMap::<String, BTreeSet<String>>::new();
    let mut identity_rows = tx
        .query(
            "SELECT 1 FROM events
             WHERE tenant = ?1 AND entity_type = ?2 AND entity_id = ?3 LIMIT 1",
            params![
                request.tenant.as_str(),
                request.entity_type.as_str(),
                request.entity_id.as_str()
            ],
        )
        .await
        .map_err(storage_error)?;
    if identity_rows.next().await.map_err(storage_error)?.is_some() {
        owners
            .entry(request.entity_id.clone())
            .or_default()
            .insert("Id".to_string());
    }
    drop(identity_rows);
    for key in &request.key_rows {
        let mut rows = tx
            .query(
                "SELECT entity_id FROM entity_key_index
                 WHERE tenant = ?1 AND entity_type = ?2 AND key_name = ?3 AND key_hash = ?4",
                params![
                    request.tenant.as_str(),
                    request.entity_type.as_str(),
                    key.key_name.as_str(),
                    key.key_hash.as_str()
                ],
            )
            .await
            .map_err(storage_error)?;
        if let Some(row) = rows.next().await.map_err(storage_error)? {
            let owner = row.get::<String>(0).map_err(storage_error)?;
            owners
                .entry(owner)
                .or_default()
                .insert(key.key_name.clone());
        }
    }

    if owners.len() > 1 {
        let outcome = owner_conflict(&owners);
        tx.commit().await.map_err(acknowledgement_unknown)?;
        return Ok(outcome);
    }
    if let Some((entity_id, _)) = owners.first_key_value() {
        let Some(stored) = load_contract(&tx, request, entity_id).await? else {
            tx.commit().await.map_err(acknowledgement_unknown)?;
            return Ok(CreateOrVerifyStoreOutcome::CreationContractMigrationRequired);
        };
        let alternate_owner = !owners
            .get(entity_id)
            .is_some_and(|fields| fields.contains("Id"));
        let outcome = compare_existing(&tx, request, entity_id, &stored, alternate_owner).await?;
        if matches!(outcome, CreateOrVerifyStoreOutcome::AlreadyMatches { .. }) {
            insert_idempotency(&tx, request, entity_id, false).await?;
        }
        tx.commit().await.map_err(acknowledgement_unknown)?;
        return Ok(outcome);
    }

    insert_first_event(&tx, &request.first_event).await?;
    insert_idempotency(&tx, request, &request.entity_id, true).await?;
    tx.commit().await.map_err(acknowledgement_unknown)?;
    Ok(CreateOrVerifyStoreOutcome::Created {
        entity_id: request.entity_id.clone(),
        sequence_nr: 1,
    })
}

pub(super) async fn commit_first_event(
    store: &TursoEventStore,
    commit: &FirstEventCommit,
) -> Result<u64, PersistenceError> {
    commit.validate()?;
    let _write_permit = store
        .acquire_write_permit(
            "turso.commit_first_event",
            super::write_gate::WritePriority::High,
        )
        .await?;
    let conn = store.configured_connection().await?;
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .await
        .map_err(storage_error)?;
    let mut rows = tx
        .query(
            "SELECT COALESCE(MAX(sequence_nr), 0) FROM events
             WHERE tenant = ?1 AND entity_type = ?2 AND entity_id = ?3",
            params![
                commit.tenant.as_str(),
                commit.entity_type.as_str(),
                commit.entity_id.as_str()
            ],
        )
        .await
        .map_err(storage_error)?;
    let current = rows
        .next()
        .await
        .map_err(storage_error)?
        .expect("aggregate query returns one row")
        .get::<i64>(0)
        .map_err(storage_error)?;
    drop(rows);
    if current != 0 {
        return Err(PersistenceError::ConcurrencyViolation {
            expected: 0,
            actual: u64::try_from(current).map_err(storage_error)?,
        });
    }
    insert_first_event(&tx, commit).await?;
    tx.commit().await.map_err(acknowledgement_unknown)?;
    Ok(1)
}

async fn insert_first_event(
    tx: &libsql::Transaction,
    commit: &FirstEventCommit,
) -> Result<(), PersistenceError> {
    super::event_store::assert_scoped_journal_write_fence(
        tx,
        &commit.tenant,
        &commit.entity_type,
        &commit.entity_id,
        std::slice::from_ref(&commit.event),
    )
    .await?;
    super::event_store::assert_unscoped_stream_publication_fence(
        tx,
        &commit.tenant,
        &commit.entity_type,
        &commit.entity_id,
        std::slice::from_ref(&commit.event),
    )
    .await?;
    let payload = serde_json::to_string(&commit.event.payload)
        .map_err(|error| PersistenceError::Serialization(error.to_string()))?;
    let metadata = serde_json::to_string(&commit.event.metadata)
        .map_err(|error| PersistenceError::Serialization(error.to_string()))?;
    tx.execute(
        "INSERT INTO events
         (tenant, entity_type, entity_id, sequence_nr, segment_index, event_type, payload, metadata)
         VALUES (?1, ?2, ?3, 1, 0, ?4, ?5, ?6)",
        params![
            commit.tenant.as_str(),
            commit.entity_type.as_str(),
            commit.entity_id.as_str(),
            commit.event.event_type.as_str(),
            payload,
            metadata
        ],
    )
    .await
    .map_err(storage_error)?;
    tx.execute(
        "INSERT INTO event_segments
         (tenant, entity_type, entity_id, segment_index, start_sequence_nr,
          end_sequence_nr, event_count)
         VALUES (?1, ?2, ?3, 0, 1, 1, 1)
         ON CONFLICT(tenant, entity_type, entity_id, segment_index) DO NOTHING",
        params![
            commit.tenant.as_str(),
            commit.entity_type.as_str(),
            commit.entity_id.as_str()
        ],
    )
    .await
    .map_err(storage_error)?;
    let contract = serde_json::to_string(&commit.contract)
        .map_err(|error| PersistenceError::Serialization(error.to_string()))?;
    tx.execute(
        "INSERT INTO entity_creation_contracts
         (tenant, entity_type, entity_id, contract_json, contract_digest,
          contract_revision, schema_identity, declared_key_signature, source_write_version)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 1)",
        params![
            commit.tenant.as_str(),
            commit.entity_type.as_str(),
            commit.entity_id.as_str(),
            contract,
            commit.contract.digest.as_str(),
            i64::from(commit.contract_revision),
            commit.schema_identity.as_str(),
            commit.declared_key_signature.as_str()
        ],
    )
    .await
    .map_err(storage_error)?;
    advance_coverage_if_complete(
        tx,
        CoverageAdvance {
            tenant: &commit.tenant,
            entity_type: &commit.entity_type,
            cursor: &commit.entity_id,
            created_sequence: 1,
            contract_revision: commit.contract_revision,
            schema_identity: &commit.schema_identity,
            declared_key_signature: &commit.declared_key_signature,
        },
    )
    .await?;
    replace_first_event_keys(tx, commit).await?;
    insert_projection(tx, commit).await?;
    Ok(())
}

mod comparison;
mod metadata;
mod repair;
pub(crate) use comparison::*;
pub(crate) use metadata::*;
pub(crate) use repair::*;
