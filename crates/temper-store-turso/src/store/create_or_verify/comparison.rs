use super::*;

pub(crate) async fn load_contract(
    tx: &libsql::Transaction,
    request: &CreateOrVerifyRequest,
    entity_id: &str,
) -> Result<Option<CreationContract>, PersistenceError> {
    let mut rows = tx
        .query(
            "SELECT contract_json FROM entity_creation_contracts
             WHERE tenant = ?1 AND entity_type = ?2 AND entity_id = ?3",
            params![
                request.tenant.as_str(),
                request.entity_type.as_str(),
                entity_id
            ],
        )
        .await
        .map_err(storage_error)?;
    let Some(row) = rows.next().await.map_err(storage_error)? else {
        return Ok(None);
    };
    let value = row.get::<String>(0).map_err(storage_error)?;
    serde_json::from_str(&value)
        .map(Some)
        .map_err(|error| PersistenceError::Serialization(error.to_string()))
}

pub(crate) async fn compare_existing(
    tx: &libsql::Transaction,
    request: &CreateOrVerifyRequest,
    entity_id: &str,
    stored: &CreationContract,
    alternate_owner: bool,
) -> Result<CreateOrVerifyStoreOutcome, PersistenceError> {
    let comparison = if alternate_owner {
        compare_creation_contracts_for_alternate_owner(stored, &request.contract)
    } else {
        compare_creation_contracts(stored, &request.contract)
    };
    match comparison {
        CreationContractComparison::Matches => {
            let mut rows = tx
                .query(
                    "SELECT 1 FROM events
                     WHERE tenant = ?1 AND entity_type = ?2 AND entity_id = ?3
                       AND sequence_nr = 1 LIMIT 1",
                    params![
                        request.tenant.as_str(),
                        request.entity_type.as_str(),
                        entity_id
                    ],
                )
                .await
                .map_err(storage_error)?;
            if rows.next().await.map_err(storage_error)?.is_none() {
                return Err(PersistenceError::Storage(
                    "creation contract has no sequence-one event".into(),
                ));
            }
            Ok(CreateOrVerifyStoreOutcome::AlreadyMatches {
                entity_id: entity_id.to_string(),
                sequence_nr: 1,
                notification_pending: false,
            })
        }
        CreationContractComparison::Conflict { fields, truncated } => {
            Ok(CreateOrVerifyStoreOutcome::Conflict { fields, truncated })
        }
        CreationContractComparison::MigrationRequired => {
            Ok(CreateOrVerifyStoreOutcome::CreationContractMigrationRequired)
        }
    }
}

pub(crate) async fn insert_idempotency(
    tx: &libsql::Transaction,
    request: &CreateOrVerifyRequest,
    entity_id: &str,
    notification_pending: bool,
) -> Result<(), PersistenceError> {
    tx.execute(
        "INSERT INTO entity_create_or_verify_idempotency
         (tenant, module_name, entity_type, idempotency_key, entity_id,
          requested_entity_id, requested_contract_json, contract_digest, notification_pending)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![
            request.tenant.as_str(),
            request.module_name.as_str(),
            request.entity_type.as_str(),
            request.idempotency_key.as_str(),
            entity_id,
            request.entity_id.as_str(),
            serde_json::to_string(&request.contract)
                .map_err(|error| PersistenceError::Serialization(error.to_string()))?,
            request.contract.digest.as_str(),
            i64::from(notification_pending)
        ],
    )
    .await
    .map_err(storage_error)?;
    Ok(())
}

pub(crate) async fn acknowledge_notification(
    store: &TursoEventStore,
    request: &CreateOrVerifyRequest,
) -> Result<(), PersistenceError> {
    let _write_permit = store
        .acquire_write_permit(
            "turso.acknowledge_create_or_verify_notification",
            super::super::write_gate::WritePriority::High,
        )
        .await?;
    let conn = store.configured_connection().await?;
    let changed = conn
        .execute(
            "UPDATE entity_create_or_verify_idempotency
             SET notification_pending = 0
             WHERE tenant = ?1 AND module_name = ?2 AND entity_type = ?3
               AND idempotency_key = ?4 AND requested_entity_id = ?5",
            params![
                request.tenant.as_str(),
                request.module_name.as_str(),
                request.entity_type.as_str(),
                request.idempotency_key.as_str(),
                request.entity_id.as_str()
            ],
        )
        .await
        .map_err(storage_error)?;
    if changed != 1 {
        return Err(PersistenceError::Storage(
            "create-or-verify notification acknowledgement lost its request".into(),
        ));
    }
    Ok(())
}

pub(crate) fn owner_conflict(
    owners: &BTreeMap<String, BTreeSet<String>>,
) -> CreateOrVerifyStoreOutcome {
    bounded_conflict(owners.values().flatten().cloned())
}

pub(crate) fn bounded_conflict(
    fields: impl IntoIterator<Item = String>,
) -> CreateOrVerifyStoreOutcome {
    let fields = fields.into_iter().collect::<BTreeSet<_>>();
    let truncated = fields.len() > CREATE_OR_VERIFY_CONFLICT_FIELD_BUDGET;
    CreateOrVerifyStoreOutcome::Conflict {
        fields: fields
            .into_iter()
            .take(CREATE_OR_VERIFY_CONFLICT_FIELD_BUDGET)
            .collect(),
        truncated,
    }
}
