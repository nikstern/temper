use super::*;

pub(crate) async fn load_contract(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    request: &CreateOrVerifyRequest,
    entity_id: &str,
) -> Result<Option<CreationContract>, PersistenceError> {
    let value: Option<serde_json::Value> = sqlx::query_scalar(
        "SELECT contract_json FROM entity_creation_contracts
         WHERE tenant = $1 AND entity_type = $2 AND entity_id = $3 FOR UPDATE",
    )
    .bind(&request.tenant)
    .bind(&request.entity_type)
    .bind(entity_id)
    .fetch_optional(&mut **tx)
    .await
    .map_err(storage)?;
    value
        .map(serde_json::from_value)
        .transpose()
        .map_err(|error| PersistenceError::Serialization(error.to_string()))
}

pub(crate) async fn compare_existing(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
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
            let creation_exists: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM events
                 WHERE tenant = $1 AND entity_type = $2 AND entity_id = $3 AND sequence_nr = 1)",
            )
            .bind(&request.tenant)
            .bind(&request.entity_type)
            .bind(entity_id)
            .fetch_one(&mut **tx)
            .await
            .map_err(storage)?;
            if !creation_exists {
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

pub(crate) fn owner_conflict(
    owners: &BTreeMap<String, BTreeSet<String>>,
) -> CreateOrVerifyStoreOutcome {
    let fields = owners
        .values()
        .flat_map(BTreeSet::iter)
        .cloned()
        .collect::<BTreeSet<_>>();
    let truncated =
        fields.len() > temper_runtime::persistence::CREATE_OR_VERIFY_CONFLICT_FIELD_BUDGET;
    CreateOrVerifyStoreOutcome::Conflict {
        fields: fields
            .into_iter()
            .take(temper_runtime::persistence::CREATE_OR_VERIFY_CONFLICT_FIELD_BUDGET)
            .collect(),
        truncated,
    }
}

pub(crate) async fn insert_idempotency(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    request: &CreateOrVerifyRequest,
    entity_id: &str,
    notification_pending: bool,
) -> Result<(), PersistenceError> {
    sqlx::query(
        "INSERT INTO entity_create_or_verify_idempotency
         (tenant, module_name, entity_type, idempotency_key, entity_id,
          requested_entity_id, requested_contract_json, contract_digest, notification_pending)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)",
    )
    .bind(&request.tenant)
    .bind(&request.module_name)
    .bind(&request.entity_type)
    .bind(&request.idempotency_key)
    .bind(entity_id)
    .bind(&request.entity_id)
    .bind(
        serde_json::to_value(&request.contract)
            .map_err(|error| PersistenceError::Serialization(error.to_string()))?,
    )
    .bind(&request.contract.digest)
    .bind(notification_pending)
    .execute(&mut **tx)
    .await
    .map_err(storage)?;
    Ok(())
}

pub(crate) async fn acknowledge_notification(
    store: &PostgresEventStore,
    request: &CreateOrVerifyRequest,
) -> Result<(), PersistenceError> {
    let result = sqlx::query(
        "UPDATE entity_create_or_verify_idempotency
         SET notification_pending = FALSE
         WHERE tenant = $1 AND module_name = $2 AND entity_type = $3
           AND idempotency_key = $4 AND requested_entity_id = $5",
    )
    .bind(&request.tenant)
    .bind(&request.module_name)
    .bind(&request.entity_type)
    .bind(&request.idempotency_key)
    .bind(&request.entity_id)
    .execute(store.pool())
    .await
    .map_err(storage)?;
    if result.rows_affected() != 1 {
        return Err(PersistenceError::Storage(
            "create-or-verify notification acknowledgement lost its request".into(),
        ));
    }
    Ok(())
}

pub(crate) fn storage(error: impl std::fmt::Display) -> PersistenceError {
    PersistenceError::Storage(error.to_string())
}
