//! Redis same-tenant atomic batch append.

use super::*;

impl RedisEventStore {
    pub(super) async fn append_batch_inner(
        &self,
        appends: &[PersistenceAppend],
    ) -> Result<Vec<PersistenceAppendResult>, PersistenceError> {
        if appends.is_empty() {
            return Ok(Vec::new());
        }
        if let [append] = appends
            && append.first_event.is_none()
        {
            let sequence_nr = self
                .append_with_index_rows(
                    &append.persistence_id,
                    append.expected_sequence,
                    &append.events,
                    &append.key_rows,
                    &append.vector_rows,
                    append.reconcile_vectors,
                )
                .await?;
            return Ok(vec![PersistenceAppendResult {
                persistence_id: append.persistence_id.clone(),
                sequence_nr,
            }]);
        }

        let mut seen = std::collections::BTreeSet::new();
        let mut parsed = Vec::with_capacity(appends.len());
        let mut tenant_name: Option<&str> = None;
        for append in appends {
            if !seen.insert(append.persistence_id.as_str()) {
                return Err(PersistenceError::Storage(format!(
                    "duplicate persistence_id '{}' in append_batch",
                    append.persistence_id
                )));
            }
            let (tenant, entity_type, entity_id) =
                parse_persistence_id_parts(&append.persistence_id)
                    .map_err(PersistenceError::Storage)?;
            if tenant_name.is_some_and(|expected| expected != tenant) {
                return Err(PersistenceError::Storage(
                    "append_batch cannot span Redis tenant indexes".to_string(),
                ));
            }
            tenant_name = Some(tenant);
            parsed.push((tenant, entity_type, entity_id));
        }
        let tenant = tenant_name.expect("non-empty appends set tenant");
        #[derive(serde::Serialize)]
        struct LuaKeyRow {
            owner_field: String,
        }
        let mut keys = Vec::with_capacity(appends.len() * 9 + 2);
        let mut args = Vec::new();
        args.push(appends.len().to_string());
        for (append, (_, entity_type, entity_id)) in appends.iter().zip(parsed.iter()) {
            keys.push(Self::seq_key(tenant, entity_type, entity_id));
            keys.push(Self::events_key(tenant, entity_type, entity_id));
            keys.push(Self::unscoped_journals_key(tenant, entity_type));
            keys.push(Self::unscoped_generation_key(tenant, entity_type));
            keys.push(Self::unscoped_fence_key(tenant, entity_type));
            keys.push(Self::create_or_verify_hash_key(
                tenant,
                entity_type,
                "owners",
            ));
            keys.push(Self::create_or_verify_hash_key(
                tenant,
                entity_type,
                "entity_keys",
            ));
            keys.push(Self::create_or_verify_hash_key(
                tenant,
                entity_type,
                "contracts",
            ));
            let contracts_key = Self::create_or_verify_hash_key(tenant, entity_type, "contracts");
            let stored_contract: Option<String> = self
                .client
                .hget(&contracts_key, *entity_id)
                .await
                .map_err(storage_error)?;
            let stored_metadata = stored_contract
                .as_deref()
                .map(serde_json::from_str::<serde_json::Value>)
                .transpose()
                .map_err(|error| PersistenceError::Serialization(error.to_string()))?;
            let coverage_identity = append
                .first_event
                .as_ref()
                .map(|metadata| {
                    (
                        metadata.schema_identity.clone(),
                        metadata.contract_revision,
                        metadata.declared_key_signature.clone(),
                    )
                })
                .or_else(|| {
                    let value = stored_metadata.as_ref()?;
                    Some((
                        value.get("schema_identity")?.as_str()?.to_string(),
                        u32::try_from(value.get("contract_revision")?.as_u64()?).ok()?,
                        value.get("declared_key_signature")?.as_str()?.to_string(),
                    ))
                });
            keys.push(coverage_identity.as_ref().map_or_else(
                || {
                    Self::create_or_verify_hash_key(
                        tenant,
                        &format!("{entity_type}:{entity_id}"),
                        "unused_coverage",
                    )
                },
                |(schema, revision, signature)| {
                    Self::creation_coverage_key(tenant, entity_type, schema, *revision, signature)
                },
            ));
            args.push(append.expected_sequence.to_string());
            let entity_ref = EntityRef {
                entity_type: (*entity_type).to_string(),
                entity_id: (*entity_id).to_string(),
            };
            args.push(
                serde_json::to_string(&entity_ref)
                    .map_err(|error| PersistenceError::Serialization(error.to_string()))?,
            );
            args.push(Self::journal_member(entity_type, entity_id));
            args.push(if split_scoped_journal_entity_id(entity_id).is_none() {
                encode_lex_component(entity_id)
            } else {
                String::new()
            });
            args.push(append.events.len().to_string());
            args.push((*entity_id).to_string());
            args.push(
                serde_json::to_string(
                    &append
                        .key_rows
                        .iter()
                        .map(|row| LuaKeyRow {
                            owner_field: format!("{}\0{}", row.key_name, row.key_hash),
                        })
                        .collect::<Vec<_>>(),
                )
                .map_err(|error| PersistenceError::Serialization(error.to_string()))?,
            );
            #[derive(serde::Serialize)]
            struct BatchFirstEvent<'a> {
                contract: &'a temper_runtime::persistence::CreationContract,
                contract_revision: u32,
                schema_identity: &'a str,
                declared_key_signature: &'a str,
                journal_lower: String,
                journal_upper: String,
            }
            let first_metadata = match &append.first_event {
                Some(first_event) => {
                    if append.expected_sequence != 0 || append.events.is_empty() {
                        return Err(PersistenceError::Storage(
                            "first-event metadata requires a non-empty sequence-0 append"
                                .to_string(),
                        ));
                    }
                    if first_event.contract_revision != first_event.contract.version
                        || first_event.schema_identity != first_event.contract.schema_digest
                    {
                        return Err(PersistenceError::Storage(
                            "invalid first-event metadata".to_string(),
                        ));
                    }
                    serde_json::to_string(&BatchFirstEvent {
                        contract: &first_event.contract,
                        contract_revision: first_event.contract_revision,
                        schema_identity: &first_event.schema_identity,
                        declared_key_signature: &first_event.declared_key_signature,
                        journal_lower: format!("[{}!", encode_lex_component(entity_type)),
                        journal_upper: format!("[{}!\u{10ffff}", encode_lex_component(entity_type)),
                    })
                    .map_err(|error| PersistenceError::Serialization(error.to_string()))?
                }
                None => String::new(),
            };
            args.push(first_metadata);
            #[derive(serde::Serialize)]
            struct BatchCoverageMetadata<'a> {
                schema_identity: &'a str,
                contract_revision: u32,
                declared_key_signature: &'a str,
                journal_lower: String,
                journal_upper: String,
            }
            args.push(match coverage_identity.as_ref() {
                Some((schema, revision, signature)) => {
                    serde_json::to_string(&BatchCoverageMetadata {
                        schema_identity: schema,
                        contract_revision: *revision,
                        declared_key_signature: signature,
                        journal_lower: format!("[{}!", encode_lex_component(entity_type)),
                        journal_upper: format!("[{}!\u{10ffff}", encode_lex_component(entity_type)),
                    })
                    .map_err(|error| PersistenceError::Serialization(error.to_string()))?
                }
                None => String::new(),
            });
            let mut sequence = append.expected_sequence;
            for event in &append.events {
                sequence += 1;
                let mut stored = event.clone();
                stored.sequence_nr = sequence;
                args.push(
                    serde_json::to_string(&stored)
                        .map_err(|error| PersistenceError::Serialization(error.to_string()))?,
                );
            }
        }
        keys.push(Self::tenant_entities_key(tenant));
        keys.push(Self::tenant_journals_key(tenant));
        let result: Vec<i64> = self
            .append_batch_script
            .evalsha_with_reload(&self.client, keys, args)
            .await
            .map_err(storage_error)?;
        if result.first() == Some(&-1) {
            return Err(PersistenceError::Storage(
                "stream descriptor publication fence".into(),
            ));
        }
        if result.first() == Some(&-2) {
            return Err(PersistenceError::Storage(
                "duplicate declared key in append_batch".to_string(),
            ));
        }
        if result.first() == Some(&-3) {
            return Err(PersistenceError::Storage(
                "invalid first-event metadata in append_batch".to_string(),
            ));
        }
        if result.first() == Some(&0) {
            let [_, append_index, actual] = result.as_slice() else {
                return Err(PersistenceError::Storage(format!(
                    "unexpected Redis append_batch conflict result: {result:?}"
                )));
            };
            let index = usize::try_from(*append_index)
                .ok()
                .and_then(|index| index.checked_sub(1))
                .filter(|index| *index < appends.len())
                .ok_or_else(|| {
                    PersistenceError::Storage(
                        "Redis append_batch returned an invalid conflict index".to_string(),
                    )
                })?;
            return Err(PersistenceError::ConcurrencyViolation {
                expected: appends[index].expected_sequence,
                actual: *actual as u64,
            });
        }
        if result.len() != appends.len() + 1 || result.first() != Some(&1) {
            return Err(PersistenceError::Storage(format!(
                "unexpected Redis append_batch result: {result:?}"
            )));
        }

        let mut results = Vec::with_capacity(appends.len());
        for (((append, (tenant, entity_type, entity_id)), new_sequence), result_index) in appends
            .iter()
            .zip(parsed.iter())
            .zip(result.iter().skip(1))
            .zip(0..appends.len())
        {
            let new_sequence = u64::try_from(*new_sequence).map_err(|_| {
                PersistenceError::Storage(format!(
                    "Redis append_batch returned a negative sequence at index {result_index}"
                ))
            })?;
            self.update_segment_after_append(
                tenant,
                entity_type,
                entity_id,
                append.expected_sequence,
                new_sequence,
            )
            .await?;
            results.push(PersistenceAppendResult {
                persistence_id: append.persistence_id.clone(),
                sequence_nr: new_sequence,
            });
        }
        Ok(results)
    }
}
