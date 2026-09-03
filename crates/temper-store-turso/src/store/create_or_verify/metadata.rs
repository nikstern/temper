use super::*;

pub(crate) async fn coverage_is_current(
    tx: &libsql::Transaction,
    request: &CreateOrVerifyRequest,
) -> Result<bool, PersistenceError> {
    let mut rows = tx
        .query(
            "SELECT source_write_version, covered_write_version
             FROM entity_creation_coverage
             WHERE tenant = ?1 AND entity_type = ?2 AND schema_identity = ?3
               AND contract_revision = ?4 AND declared_key_signature = ?5
               AND completed_at IS NOT NULL",
            params![
                request.tenant.as_str(),
                request.entity_type.as_str(),
                request.schema_identity.as_str(),
                i64::from(request.contract_revision),
                request.declared_key_signature.as_str()
            ],
        )
        .await
        .map_err(storage_error)?;
    let Some(row) = rows.next().await.map_err(storage_error)? else {
        return Ok(false);
    };
    let source = row.get::<i64>(0).map_err(storage_error)?;
    let complete = row.get::<i64>(1).map_err(storage_error)?;
    drop(rows);
    let mut stream_rows = tx
        .query(
            "SELECT COUNT(DISTINCT entity_id) FROM events WHERE tenant = ?1 AND entity_type = ?2",
            params![request.tenant.as_str(), request.entity_type.as_str()],
        )
        .await
        .map_err(storage_error)?;
    let stream_count = stream_rows
        .next()
        .await
        .map_err(storage_error)?
        .expect("aggregate query returns one row")
        .get::<i64>(0)
        .map_err(storage_error)?;
    drop(stream_rows);
    let total_contracts = aggregate_count(
        tx,
        "SELECT COUNT(*) FROM entity_creation_contracts WHERE tenant=?1 AND entity_type=?2",
        params![request.tenant.as_str(), request.entity_type.as_str()],
    )
    .await?;
    let actual_write_version = aggregate_count(
        tx,
        "SELECT COALESCE(SUM(sequence_nr), 0) FROM (
           SELECT MAX(sequence_nr) AS sequence_nr FROM events
           WHERE tenant=?1 AND entity_type=?2 GROUP BY entity_id
         )",
        params![request.tenant.as_str(), request.entity_type.as_str()],
    )
    .await?;
    Ok(source == complete && complete == actual_write_version && stream_count == total_contracts)
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
    tx: &libsql::Transaction,
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
    let mut covered_rows = tx
        .query(
            "SELECT COALESCE(SUM(sequence_nr), 0) FROM (
               SELECT MAX(sequence_nr) AS sequence_nr FROM events
               WHERE tenant=?1 AND entity_type=?2 GROUP BY entity_id
             )",
            params![tenant, entity_type],
        )
        .await
        .map_err(storage_error)?;
    let write_version = covered_rows
        .next()
        .await
        .map_err(storage_error)?
        .expect("aggregate query returns one row")
        .get::<i64>(0)
        .map_err(storage_error)?;
    drop(covered_rows);
    let prior_write_version = write_version
        .checked_sub(i64::try_from(created_sequence).map_err(storage_error)?)
        .ok_or_else(|| {
            PersistenceError::Storage("creation coverage write version underflow".to_string())
        })?;
    let mut stream_rows = tx
        .query(
            "SELECT COUNT(DISTINCT entity_id) FROM events WHERE tenant=?1 AND entity_type=?2",
            params![tenant, entity_type],
        )
        .await
        .map_err(storage_error)?;
    let stream_count = stream_rows
        .next()
        .await
        .map_err(storage_error)?
        .expect("aggregate query returns one row")
        .get::<i64>(0)
        .map_err(storage_error)?;
    drop(stream_rows);
    let mut matching_rows = tx
        .query(
            "SELECT COUNT(*), COALESCE(SUM(source_write_version), 0)
             FROM entity_creation_contracts
             WHERE tenant=?1 AND entity_type=?2 AND schema_identity=?3
               AND contract_revision=?4 AND declared_key_signature=?5",
            params![
                tenant,
                entity_type,
                schema_identity,
                i64::from(contract_revision),
                declared_key_signature
            ],
        )
        .await
        .map_err(storage_error)?;
    let matching_row = matching_rows
        .next()
        .await
        .map_err(storage_error)?
        .expect("aggregate query returns one row");
    let matching_contracts = matching_row.get::<i64>(0).map_err(storage_error)?;
    let reconciled_write_version = matching_row.get::<i64>(1).map_err(storage_error)?;
    drop(matching_rows);
    if matching_contracts != stream_count || reconciled_write_version != write_version {
        return Ok(());
    }
    if prior_write_version == 0 {
        tx.execute(
            "INSERT INTO entity_creation_coverage
             (tenant, entity_type, schema_identity, contract_revision, declared_key_signature,
              cursor, source_write_version, covered_write_version, completed_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7, datetime('now'))
             ON CONFLICT DO NOTHING",
            params![
                tenant,
                entity_type,
                schema_identity,
                i64::from(contract_revision),
                declared_key_signature,
                cursor,
                write_version
            ],
        )
        .await
        .map_err(storage_error)?;
        return Ok(());
    }
    tx.execute(
        "UPDATE entity_creation_coverage
         SET cursor=?6, source_write_version=?7, covered_write_version=?7,
             completed_at=datetime('now')
         WHERE tenant=?1 AND entity_type=?2 AND schema_identity=?3
           AND contract_revision=?4 AND declared_key_signature=?5
           AND source_write_version=?8 AND covered_write_version=?8
           AND completed_at IS NOT NULL",
        params![
            tenant,
            entity_type,
            schema_identity,
            i64::from(contract_revision),
            declared_key_signature,
            cursor,
            write_version,
            prior_write_version
        ],
    )
    .await
    .map_err(storage_error)?;
    Ok(())
}

pub(crate) async fn touch_creation_contract(
    tx: &libsql::Transaction,
    tenant: &str,
    entity_type: &str,
    entity_id: &str,
    source_write_version: u64,
) -> Result<Option<(u32, String, String)>, PersistenceError> {
    let mut rows = tx
        .query(
            "UPDATE entity_creation_contracts SET source_write_version=?4
             WHERE tenant=?1 AND entity_type=?2 AND entity_id=?3
             RETURNING contract_revision, schema_identity, declared_key_signature",
            params![
                tenant,
                entity_type,
                entity_id,
                i64::try_from(source_write_version).map_err(storage_error)?
            ],
        )
        .await
        .map_err(storage_error)?;
    let result = match rows.next().await.map_err(storage_error)? {
        Some(row) => Some((
            u32::try_from(row.get::<i64>(0).map_err(storage_error)?).map_err(storage_error)?,
            row.get::<String>(1).map_err(storage_error)?,
            row.get::<String>(2).map_err(storage_error)?,
        )),
        None => None,
    };
    drop(rows);
    Ok(result)
}

pub(crate) async fn insert_projection(
    tx: &libsql::Transaction,
    commit: &FirstEventCommit,
) -> Result<(), PersistenceError> {
    let Some(projection) = &commit.projection else {
        return Ok(());
    };
    let status = super::super::field_index::canonical_projection_status(
        &projection.status,
        &projection.state,
    );
    let fields = serde_json::to_string(&projection.fields)
        .map_err(|error| PersistenceError::Serialization(error.to_string()))?;
    let state = serde_json::to_string(&projection.state)
        .map_err(|error| PersistenceError::Serialization(error.to_string()))?;
    let hash = super::super::field_index::projection_hash(status, &projection.fields);
    let updated_at = temper_runtime::scheduler::sim_now().to_rfc3339();
    tx.execute(
        "INSERT INTO entity_catalog
         (tenant, entity_type, entity_id, status, fields, state, updated_at,
          sequence_nr, projection_version, projection_hash)
         VALUES (?1,?2,?3,?4,?5,?6,?7,1,2,?8)",
        params![
            commit.tenant.as_str(),
            commit.entity_type.as_str(),
            commit.entity_id.as_str(),
            status,
            fields,
            state,
            updated_at,
            hash
        ],
    )
    .await
    .map_err(storage_error)?;
    let indexed = super::super::field_index::indexed_projection_fields(status, &projection.fields);
    if !indexed.is_empty() {
        let mut sql = String::from(
            "INSERT INTO entity_field_index
             (tenant, entity_type, entity_id, field_name, field_value, status) VALUES ",
        );
        let mut values = Vec::with_capacity(indexed.len() * 6);
        for (index, (field_name, field_value)) in indexed.into_iter().enumerate() {
            if index > 0 {
                sql.push_str(", ");
            }
            sql.push_str("(?, ?, ?, ?, ?, ?)");
            values.push(Value::from(commit.tenant.clone()));
            values.push(Value::from(commit.entity_type.clone()));
            values.push(Value::from(commit.entity_id.clone()));
            values.push(Value::from(field_name));
            values.push(field_value.map_or(Value::Null, Value::from));
            values.push(Value::from(status.to_string()));
        }
        tx.execute(&sql, params_from_iter(values))
            .await
            .map_err(storage_error)?;
    }
    Ok(())
}
