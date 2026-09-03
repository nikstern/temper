use super::*;

pub(crate) async fn reconcile_creation_metadata(
    store: &TursoEventStore,
    repair: &temper_runtime::persistence::CreationMetadataRepair,
) -> Result<(), PersistenceError> {
    repair.first_event.validate()?;
    let commit = &repair.first_event;
    let tx = store
        .connection()?
        .transaction()
        .await
        .map_err(storage_error)?;
    let mut rows = tx
        .query(
            "SELECT sequence_nr, metadata FROM events
             WHERE tenant=?1 AND entity_type=?2 AND entity_id=?3
             ORDER BY sequence_nr",
            params![
                commit.tenant.as_str(),
                commit.entity_type.as_str(),
                commit.entity_id.as_str()
            ],
        )
        .await
        .map_err(storage_error)?;
    let mut first_event_id = None;
    let mut actual = 0_u64;
    while let Some(row) = rows.next().await.map_err(storage_error)? {
        let sequence =
            u64::try_from(row.get::<i64>(0).map_err(storage_error)?).map_err(storage_error)?;
        actual = sequence;
        if sequence == 1 {
            let metadata: String = row.get(1).map_err(storage_error)?;
            let metadata: temper_runtime::persistence::EventMetadata =
                serde_json::from_str(&metadata)
                    .map_err(|error| PersistenceError::Serialization(error.to_string()))?;
            first_event_id = Some(metadata.event_id);
        }
    }
    drop(rows);
    if actual != repair.source_sequence || first_event_id != Some(commit.event.metadata.event_id) {
        return Err(PersistenceError::ConcurrencyViolation {
            expected: repair.source_sequence,
            actual,
        });
    }
    let contract_json = serde_json::to_string(&commit.contract)
        .map_err(|error| PersistenceError::Serialization(error.to_string()))?;
    tx.execute(
        "INSERT INTO entity_creation_contracts
         (tenant,entity_type,entity_id,contract_json,contract_digest,contract_revision,
          schema_identity,declared_key_signature,source_write_version)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9)
         ON CONFLICT(tenant,entity_type,entity_id) DO NOTHING",
        params![
            commit.tenant.as_str(),
            commit.entity_type.as_str(),
            commit.entity_id.as_str(),
            contract_json,
            commit.contract.digest.as_str(),
            i64::from(commit.contract_revision),
            commit.schema_identity.as_str(),
            commit.declared_key_signature.as_str(),
            i64::try_from(repair.source_sequence).map_err(storage_error)?
        ],
    )
    .await
    .map_err(storage_error)?;
    let mut stored = tx
        .query(
            "SELECT contract_json FROM entity_creation_contracts
             WHERE tenant=?1 AND entity_type=?2 AND entity_id=?3",
            params![
                commit.tenant.as_str(),
                commit.entity_type.as_str(),
                commit.entity_id.as_str()
            ],
        )
        .await
        .map_err(storage_error)?;
    let contract_json = stored
        .next()
        .await
        .map_err(storage_error)?
        .ok_or_else(|| PersistenceError::Storage("creation repair contract is absent".into()))?
        .get::<String>(0)
        .map_err(storage_error)?;
    drop(stored);
    let stored_contract: CreationContract = serde_json::from_str(&contract_json)
        .map_err(|error| PersistenceError::Serialization(error.to_string()))?;
    if !matches!(
        temper_runtime::persistence::compare_creation_contracts(&stored_contract, &commit.contract),
        temper_runtime::persistence::CreationContractComparison::Matches
    ) {
        return Err(PersistenceError::Storage(
            "creation repair does not match the immutable creation contract".into(),
        ));
    }
    tx.execute(
        "UPDATE entity_creation_contracts
         SET contract_revision=?4, schema_identity=?5, declared_key_signature=?6,
             source_write_version=?7
         WHERE tenant=?1 AND entity_type=?2 AND entity_id=?3",
        params![
            commit.tenant.as_str(),
            commit.entity_type.as_str(),
            commit.entity_id.as_str(),
            i64::from(commit.contract_revision),
            commit.schema_identity.as_str(),
            commit.declared_key_signature.as_str(),
            i64::try_from(repair.source_sequence).map_err(storage_error)?
        ],
    )
    .await
    .map_err(storage_error)?;
    tx.execute(
        "DELETE FROM entity_key_index WHERE tenant=?1 AND entity_type=?2 AND entity_id=?3",
        params![
            commit.tenant.as_str(),
            commit.entity_type.as_str(),
            commit.entity_id.as_str()
        ],
    )
    .await
    .map_err(storage_error)?;
    for key in &commit.key_rows {
        tx.execute(
            "INSERT INTO entity_key_index
             (tenant,entity_type,key_name,key_hash,entity_id,sequence_nr)
             VALUES (?1,?2,?3,?4,?5,?6)",
            params![
                commit.tenant.as_str(),
                commit.entity_type.as_str(),
                key.key_name.as_str(),
                key.key_hash.as_str(),
                commit.entity_id.as_str(),
                i64::try_from(repair.source_sequence).map_err(storage_error)?
            ],
        )
        .await
        .map_err(storage_error)?;
    }
    tx.commit().await.map_err(storage_error)
}

pub(crate) async fn publish_creation_coverage(
    store: &TursoEventStore,
    publication: &temper_runtime::persistence::CreationCoveragePublication,
) -> Result<(), PersistenceError> {
    let tx = store
        .connection()?
        .transaction()
        .await
        .map_err(storage_error)?;
    let stream_count = aggregate_count(
        &tx,
        "SELECT COUNT(DISTINCT entity_id) FROM events WHERE tenant=?1 AND entity_type=?2",
        params![
            publication.tenant.as_str(),
            publication.entity_type.as_str()
        ],
    )
    .await?;
    let actual_write_version = aggregate_count(
        &tx,
        "SELECT COALESCE(SUM(sequence_nr), 0) FROM (
           SELECT MAX(sequence_nr) AS sequence_nr FROM events
           WHERE tenant=?1 AND entity_type=?2 GROUP BY entity_id
         )",
        params![
            publication.tenant.as_str(),
            publication.entity_type.as_str()
        ],
    )
    .await?;
    let matching_contracts = aggregate_count(
        &tx,
        "SELECT COUNT(*) FROM entity_creation_contracts
         WHERE tenant=?1 AND entity_type=?2 AND schema_identity=?3
           AND contract_revision=?4 AND declared_key_signature=?5",
        params![
            publication.tenant.as_str(),
            publication.entity_type.as_str(),
            publication.metadata.schema_identity.as_str(),
            i64::from(publication.metadata.contract_revision),
            publication.metadata.declared_key_signature.as_str()
        ],
    )
    .await?;
    let reconciled_write_version = aggregate_count(
        &tx,
        "SELECT COALESCE(SUM(source_write_version), 0) FROM entity_creation_contracts
         WHERE tenant=?1 AND entity_type=?2 AND schema_identity=?3
           AND contract_revision=?4 AND declared_key_signature=?5",
        params![
            publication.tenant.as_str(),
            publication.entity_type.as_str(),
            publication.metadata.schema_identity.as_str(),
            i64::from(publication.metadata.contract_revision),
            publication.metadata.declared_key_signature.as_str()
        ],
    )
    .await?;
    let expected = i64::try_from(publication.source_write_version).map_err(storage_error)?;
    if stream_count != matching_contracts
        || actual_write_version != expected
        || reconciled_write_version != expected
    {
        return Err(PersistenceError::ConcurrencyViolation {
            expected: publication.source_write_version,
            actual: u64::try_from(actual_write_version).unwrap_or(0),
        });
    }
    tx.execute(
        "INSERT INTO entity_creation_coverage
         (tenant,entity_type,schema_identity,contract_revision,declared_key_signature,
          cursor,source_write_version,covered_write_version,completed_at)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?7,datetime('now'))
         ON CONFLICT(tenant,entity_type,schema_identity,contract_revision,
                     declared_key_signature) DO UPDATE SET
          cursor=excluded.cursor,source_write_version=excluded.source_write_version,
          covered_write_version=excluded.covered_write_version,completed_at=excluded.completed_at",
        params![
            publication.tenant.as_str(),
            publication.entity_type.as_str(),
            publication.metadata.schema_identity.as_str(),
            i64::from(publication.metadata.contract_revision),
            publication.metadata.declared_key_signature.as_str(),
            publication.cursor.as_str(),
            expected
        ],
    )
    .await
    .map_err(storage_error)?;
    tx.commit().await.map_err(storage_error)
}

pub(crate) async fn aggregate_count(
    tx: &libsql::Transaction,
    sql: &str,
    params: impl libsql::params::IntoParams,
) -> Result<i64, PersistenceError> {
    let mut rows = tx.query(sql, params).await.map_err(storage_error)?;
    rows.next()
        .await
        .map_err(storage_error)?
        .expect("aggregate query returns one row")
        .get(0)
        .map_err(storage_error)
}

pub(crate) async fn replace_first_event_keys(
    tx: &libsql::Transaction,
    commit: &FirstEventCommit,
) -> Result<(), PersistenceError> {
    tx.execute(
        "DELETE FROM entity_key_index
         WHERE tenant = ?1 AND entity_type = ?2 AND entity_id = ?3",
        params![
            commit.tenant.as_str(),
            commit.entity_type.as_str(),
            commit.entity_id.as_str()
        ],
    )
    .await
    .map_err(storage_error)?;
    for key in &commit.key_rows {
        tx.execute(
            "INSERT INTO entity_key_index
             (tenant, entity_type, key_name, key_hash, entity_id, sequence_nr)
             VALUES (?1, ?2, ?3, ?4, ?5, 1)",
            params![
                commit.tenant.as_str(),
                commit.entity_type.as_str(),
                key.key_name.as_str(),
                key.key_hash.as_str(),
                commit.entity_id.as_str()
            ],
        )
        .await
        .map_err(storage_error)?;
    }
    Ok(())
}
