use super::*;

pub(crate) async fn insert_contract_and_keys(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    commit: &FirstEventCommit,
) -> Result<(), PersistenceError> {
    let contract_json = serde_json::to_value(&commit.contract)
        .map_err(|error| PersistenceError::Serialization(error.to_string()))?;
    sqlx::query(
        "INSERT INTO entity_creation_contracts
         (tenant, entity_type, entity_id, contract_json, contract_digest,
          contract_revision, schema_identity, declared_key_signature, source_write_version)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, 1)",
    )
    .bind(&commit.tenant)
    .bind(&commit.entity_type)
    .bind(&commit.entity_id)
    .bind(contract_json)
    .bind(&commit.contract.digest)
    .bind(i64::from(commit.contract_revision))
    .bind(&commit.schema_identity)
    .bind(&commit.declared_key_signature)
    .execute(&mut **tx)
    .await
    .map_err(storage)?;
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
    sqlx::query(
        "DELETE FROM entity_key_index WHERE tenant = $1 AND entity_type = $2 AND entity_id = $3",
    )
    .bind(&commit.tenant)
    .bind(&commit.entity_type)
    .bind(&commit.entity_id)
    .execute(&mut **tx)
    .await
    .map_err(storage)?;
    for key in &commit.key_rows {
        sqlx::query(
            "INSERT INTO entity_key_index
             (tenant, entity_type, key_name, key_hash, entity_id, sequence_nr)
             VALUES ($1, $2, $3, $4, $5, 1)",
        )
        .bind(&commit.tenant)
        .bind(&commit.entity_type)
        .bind(&key.key_name)
        .bind(&key.key_hash)
        .bind(&commit.entity_id)
        .execute(&mut **tx)
        .await
        .map_err(storage)?;
    }
    Ok(())
}

pub(crate) async fn coverage_is_current(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    request: &CreateOrVerifyRequest,
) -> Result<bool, PersistenceError> {
    let covered: Option<(i64, i64)> = sqlx::query_as(
        "SELECT source_write_version, covered_write_version
         FROM entity_creation_coverage
         WHERE tenant = $1 AND entity_type = $2 AND schema_identity = $3
           AND contract_revision = $4 AND declared_key_signature = $5
           AND completed_at IS NOT NULL",
    )
    .bind(&request.tenant)
    .bind(&request.entity_type)
    .bind(&request.schema_identity)
    .bind(i64::from(request.contract_revision))
    .bind(&request.declared_key_signature)
    .fetch_optional(&mut **tx)
    .await
    .map_err(storage)?;
    let stream_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(DISTINCT entity_id) FROM events WHERE tenant = $1 AND entity_type = $2",
    )
    .bind(&request.tenant)
    .bind(&request.entity_type)
    .fetch_one(&mut **tx)
    .await
    .map_err(storage)?;
    let total_contracts: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM entity_creation_contracts WHERE tenant=$1 AND entity_type=$2",
    )
    .bind(&request.tenant)
    .bind(&request.entity_type)
    .fetch_one(&mut **tx)
    .await
    .map_err(storage)?;
    let actual_write_version: i64 = sqlx::query_scalar(
        "SELECT COALESCE(SUM(sequence_nr), 0)::BIGINT FROM (
           SELECT MAX(sequence_nr) AS sequence_nr FROM events
           WHERE tenant=$1 AND entity_type=$2 GROUP BY entity_id
         ) streams",
    )
    .bind(&request.tenant)
    .bind(&request.entity_type)
    .fetch_one(&mut **tx)
    .await
    .map_err(storage)?;
    Ok(covered.is_some_and(|(source, complete)| {
        source == complete && complete == actual_write_version && stream_count == total_contracts
    }))
}

pub(crate) struct CoverageAdvance<'a> {
    pub(crate) tenant: &'a str,
    pub(crate) entity_type: &'a str,
    pub(crate) cursor: &'a str,
    pub(crate) created_sequence: u64,
    pub(crate) contract_revision: u32,
    pub(crate) schema_identity: &'a str,
    pub(crate) declared_key_signature: &'a str,
}

pub(crate) async fn advance_coverage_if_complete(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    advance: CoverageAdvance<'_>,
) -> Result<(), PersistenceError> {
    let CoverageAdvance {
        tenant,
        entity_type,
        cursor,
        created_sequence,
        contract_revision,
        schema_identity,
        declared_key_signature,
    } = advance;
    let write_version: i64 = sqlx::query_scalar(
        "SELECT COALESCE(SUM(sequence_nr), 0)::BIGINT FROM (
           SELECT MAX(sequence_nr) AS sequence_nr FROM events
           WHERE tenant=$1 AND entity_type=$2 GROUP BY entity_id
         ) streams",
    )
    .bind(tenant)
    .bind(entity_type)
    .fetch_one(&mut **tx)
    .await
    .map_err(storage)?;
    let prior_write_version = write_version
        .checked_sub(i64::try_from(created_sequence).map_err(storage)?)
        .ok_or_else(|| {
            PersistenceError::Storage("creation coverage write version underflow".to_string())
        })?;
    let stream_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(DISTINCT entity_id) FROM events WHERE tenant=$1 AND entity_type=$2",
    )
    .bind(tenant)
    .bind(entity_type)
    .fetch_one(&mut **tx)
    .await
    .map_err(storage)?;
    let (matching_contracts, reconciled_write_version): (i64, i64) = sqlx::query_as(
        "SELECT COUNT(*), COALESCE(SUM(source_write_version), 0)::BIGINT
         FROM entity_creation_contracts
         WHERE tenant=$1 AND entity_type=$2 AND schema_identity=$3
           AND contract_revision=$4 AND declared_key_signature=$5",
    )
    .bind(tenant)
    .bind(entity_type)
    .bind(schema_identity)
    .bind(i64::from(contract_revision))
    .bind(declared_key_signature)
    .fetch_one(&mut **tx)
    .await
    .map_err(storage)?;
    if matching_contracts != stream_count || reconciled_write_version != write_version {
        return Ok(());
    }
    if prior_write_version == 0 {
        sqlx::query(
            "INSERT INTO entity_creation_coverage
             (tenant, entity_type, schema_identity, contract_revision, declared_key_signature,
              cursor, source_write_version, covered_write_version, completed_at)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $7, now())
             ON CONFLICT DO NOTHING",
        )
        .bind(tenant)
        .bind(entity_type)
        .bind(schema_identity)
        .bind(i64::from(contract_revision))
        .bind(declared_key_signature)
        .bind(cursor)
        .bind(write_version)
        .execute(&mut **tx)
        .await
        .map_err(storage)?;
        return Ok(());
    }
    sqlx::query(
        "UPDATE entity_creation_coverage
         SET cursor=$6, source_write_version=$7, covered_write_version=$7, completed_at=now()
         WHERE tenant=$1 AND entity_type=$2 AND schema_identity=$3
           AND contract_revision=$4 AND declared_key_signature=$5
           AND source_write_version=$8 AND covered_write_version=$8
           AND completed_at IS NOT NULL",
    )
    .bind(tenant)
    .bind(entity_type)
    .bind(schema_identity)
    .bind(i64::from(contract_revision))
    .bind(declared_key_signature)
    .bind(cursor)
    .bind(write_version)
    .bind(prior_write_version)
    .execute(&mut **tx)
    .await
    .map_err(storage)?;
    Ok(())
}

pub(crate) async fn touch_creation_contract(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant: &str,
    entity_type: &str,
    entity_id: &str,
    source_write_version: u64,
) -> Result<Option<(u32, String, String)>, PersistenceError> {
    let row: Option<(i64, String, String)> = sqlx::query_as(
        "UPDATE entity_creation_contracts SET source_write_version=$4
         WHERE tenant=$1 AND entity_type=$2 AND entity_id=$3
         RETURNING contract_revision, schema_identity, declared_key_signature",
    )
    .bind(tenant)
    .bind(entity_type)
    .bind(entity_id)
    .bind(i64::try_from(source_write_version).map_err(storage)?)
    .fetch_optional(&mut **tx)
    .await
    .map_err(storage)?;
    row.map(|(revision, schema, signature)| {
        Ok((u32::try_from(revision).map_err(storage)?, schema, signature))
    })
    .transpose()
}

pub(crate) async fn insert_projection(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    commit: &FirstEventCommit,
) -> Result<(), PersistenceError> {
    let Some(projection) = &commit.projection else {
        return Ok(());
    };
    let status =
        crate::platform::canonical_projection_status(&projection.status, &projection.state);
    let projection_hash = crate::platform::json_hash(&projection.fields);
    sqlx::query(
        "INSERT INTO entity_catalog
         (tenant, entity_type, entity_id, status, fields, state, sequence_nr,
          projection_version, projection_hash, updated_at)
         VALUES ($1,$2,$3,$4,$5,$6,1,2,$7,now())",
    )
    .bind(&commit.tenant)
    .bind(&commit.entity_type)
    .bind(&commit.entity_id)
    .bind(status)
    .bind(&projection.fields)
    .bind(&projection.state)
    .bind(projection_hash)
    .execute(&mut **tx)
    .await
    .map_err(storage)?;
    let (indexed, _, _) = crate::platform::scalar_index_fields(&projection.fields);
    for (field_name, field_value) in indexed {
        sqlx::query(
            "INSERT INTO entity_field_index
             (tenant, entity_type, entity_id, field_name, field_value, status)
             VALUES ($1,$2,$3,$4,$5,$6)",
        )
        .bind(&commit.tenant)
        .bind(&commit.entity_type)
        .bind(&commit.entity_id)
        .bind(field_name)
        .bind(field_value)
        .bind(status)
        .execute(&mut **tx)
        .await
        .map_err(storage)?;
    }
    Ok(())
}
