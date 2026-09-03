use super::*;

pub(crate) async fn reconcile_creation_metadata(
    store: &PostgresEventStore,
    repair: &temper_runtime::persistence::CreationMetadataRepair,
) -> Result<(), PersistenceError> {
    repair.first_event.validate()?;
    let commit = &repair.first_event;
    let mut tx = store.pool().begin().await.map_err(storage)?;
    let row: Option<(i64, Option<String>)> = sqlx::query_as(
        "SELECT MAX(sequence_nr),
                MAX(CASE WHEN sequence_nr = 1 THEN metadata->>'event_id' END)
         FROM events WHERE tenant = $1 AND entity_type = $2 AND entity_id = $3",
    )
    .bind(&commit.tenant)
    .bind(&commit.entity_type)
    .bind(&commit.entity_id)
    .fetch_optional(&mut *tx)
    .await
    .map_err(storage)?;
    let (actual, event_id) = row.unwrap_or((0, None));
    if u64::try_from(actual).map_err(storage)? != repair.source_sequence
        || event_id.as_deref() != Some(commit.event.metadata.event_id.to_string().as_str())
    {
        return Err(PersistenceError::ConcurrencyViolation {
            expected: repair.source_sequence,
            actual: u64::try_from(actual).unwrap_or(0),
        });
    }
    let contract_json = serde_json::to_value(&commit.contract)
        .map_err(|error| PersistenceError::Serialization(error.to_string()))?;
    sqlx::query(
        "INSERT INTO entity_creation_contracts
         (tenant, entity_type, entity_id, contract_json, contract_digest,
          contract_revision, schema_identity, declared_key_signature, source_write_version)
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9)
         ON CONFLICT (tenant, entity_type, entity_id) DO NOTHING",
    )
    .bind(&commit.tenant)
    .bind(&commit.entity_type)
    .bind(&commit.entity_id)
    .bind(contract_json)
    .bind(&commit.contract.digest)
    .bind(i64::from(commit.contract_revision))
    .bind(&commit.schema_identity)
    .bind(&commit.declared_key_signature)
    .bind(i64::try_from(repair.source_sequence).map_err(storage)?)
    .execute(&mut *tx)
    .await
    .map_err(storage)?;
    let stored_value: serde_json::Value = sqlx::query_scalar(
        "SELECT contract_json FROM entity_creation_contracts
         WHERE tenant=$1 AND entity_type=$2 AND entity_id=$3",
    )
    .bind(&commit.tenant)
    .bind(&commit.entity_type)
    .bind(&commit.entity_id)
    .fetch_one(&mut *tx)
    .await
    .map_err(storage)?;
    let stored_contract: CreationContract = serde_json::from_value(stored_value)
        .map_err(|error| PersistenceError::Serialization(error.to_string()))?;
    if !matches!(
        temper_runtime::persistence::compare_creation_contracts(&stored_contract, &commit.contract),
        temper_runtime::persistence::CreationContractComparison::Matches
    ) {
        return Err(PersistenceError::Storage(
            "creation repair does not match the immutable creation contract".into(),
        ));
    }
    sqlx::query(
        "UPDATE entity_creation_contracts
         SET contract_revision=$4, schema_identity=$5, declared_key_signature=$6,
             source_write_version=$7
         WHERE tenant=$1 AND entity_type=$2 AND entity_id=$3",
    )
    .bind(&commit.tenant)
    .bind(&commit.entity_type)
    .bind(&commit.entity_id)
    .bind(i64::from(commit.contract_revision))
    .bind(&commit.schema_identity)
    .bind(&commit.declared_key_signature)
    .bind(i64::try_from(repair.source_sequence).map_err(storage)?)
    .execute(&mut *tx)
    .await
    .map_err(storage)?;
    sqlx::query("DELETE FROM entity_key_index WHERE tenant=$1 AND entity_type=$2 AND entity_id=$3")
        .bind(&commit.tenant)
        .bind(&commit.entity_type)
        .bind(&commit.entity_id)
        .execute(&mut *tx)
        .await
        .map_err(storage)?;
    for key in &commit.key_rows {
        sqlx::query(
            "INSERT INTO entity_key_index
             (tenant, entity_type, key_name, key_hash, entity_id, sequence_nr)
             VALUES ($1,$2,$3,$4,$5,$6)",
        )
        .bind(&commit.tenant)
        .bind(&commit.entity_type)
        .bind(&key.key_name)
        .bind(&key.key_hash)
        .bind(&commit.entity_id)
        .bind(i64::try_from(repair.source_sequence).map_err(storage)?)
        .execute(&mut *tx)
        .await
        .map_err(storage)?;
    }
    tx.commit().await.map_err(storage)
}

pub(crate) async fn publish_creation_coverage(
    store: &PostgresEventStore,
    publication: &temper_runtime::persistence::CreationCoveragePublication,
) -> Result<(), PersistenceError> {
    let mut tx = store.pool().begin().await.map_err(storage)?;
    let stream_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(DISTINCT entity_id) FROM events WHERE tenant=$1 AND entity_type=$2",
    )
    .bind(&publication.tenant)
    .bind(&publication.entity_type)
    .fetch_one(&mut *tx)
    .await
    .map_err(storage)?;
    let actual_write_version: i64 = sqlx::query_scalar(
        "SELECT COALESCE(SUM(sequence_nr), 0)::BIGINT FROM (
           SELECT MAX(sequence_nr) AS sequence_nr FROM events
           WHERE tenant=$1 AND entity_type=$2 GROUP BY entity_id
         ) streams",
    )
    .bind(&publication.tenant)
    .bind(&publication.entity_type)
    .fetch_one(&mut *tx)
    .await
    .map_err(storage)?;
    let (matching_contracts, reconciled_write_version): (i64, i64) = sqlx::query_as(
        "SELECT COUNT(*), COALESCE(SUM(source_write_version), 0)::BIGINT
         FROM entity_creation_contracts
         WHERE tenant=$1 AND entity_type=$2 AND schema_identity=$3
           AND contract_revision=$4 AND declared_key_signature=$5",
    )
    .bind(&publication.tenant)
    .bind(&publication.entity_type)
    .bind(&publication.metadata.schema_identity)
    .bind(i64::from(publication.metadata.contract_revision))
    .bind(&publication.metadata.declared_key_signature)
    .fetch_one(&mut *tx)
    .await
    .map_err(storage)?;
    let expected = i64::try_from(publication.source_write_version).map_err(storage)?;
    if stream_count != matching_contracts
        || actual_write_version != expected
        || reconciled_write_version != expected
    {
        return Err(PersistenceError::ConcurrencyViolation {
            expected: publication.source_write_version,
            actual: u64::try_from(actual_write_version).unwrap_or(0),
        });
    }
    sqlx::query(
        "INSERT INTO entity_creation_coverage
         (tenant,entity_type,schema_identity,contract_revision,declared_key_signature,
          cursor,source_write_version,covered_write_version,completed_at)
         VALUES ($1,$2,$3,$4,$5,$6,$7,$7,now())
         ON CONFLICT (tenant,entity_type,schema_identity,contract_revision,
                      declared_key_signature) DO UPDATE SET
          cursor=EXCLUDED.cursor,source_write_version=EXCLUDED.source_write_version,
          covered_write_version=EXCLUDED.covered_write_version,completed_at=EXCLUDED.completed_at",
    )
    .bind(&publication.tenant)
    .bind(&publication.entity_type)
    .bind(&publication.metadata.schema_identity)
    .bind(i64::from(publication.metadata.contract_revision))
    .bind(&publication.metadata.declared_key_signature)
    .bind(&publication.cursor)
    .bind(expected)
    .execute(&mut *tx)
    .await
    .map_err(storage)?;
    tx.commit().await.map_err(storage)
}
