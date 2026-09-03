macro_rules! impl_sim_batch_methods {
    () => {
        async fn append_batch(
            &self,
            appends: &[PersistenceAppend],
        ) -> Result<Vec<PersistenceAppendResult>, PersistenceError> {
            if appends.is_empty() {
                return Ok(Vec::new());
            }

            let mut inner = self.inner.lock().expect("SimEventStore lock poisoned"); // ci-ok: infallible lock

            let mut seen = std::collections::BTreeSet::new();
            let mut batch_key_claims = std::collections::BTreeMap::new();
            for append in appends {
                if !seen.insert(append.persistence_id.as_str()) {
                    return Err(PersistenceError::Storage(format!(
                        "SimEventStore: duplicate persistence_id '{}' in append_batch",
                        append.persistence_id
                    )));
                }
            }

            for append in appends {
                if let Ok((tenant, entity_type, entity_id)) =
                    parse_persistence_id_parts(&append.persistence_id)
                    && let Some((_, pin)) = split_scoped_journal_entity_id(entity_id)
                {
                    if inner.schema_deployments.migrated_source_is_fenced(
                        tenant,
                        &pin.scope,
                        &pin.bundle_digest,
                    ) {
                        return Err(PersistenceError::Storage(
                            "migrated scoped schema write fence".into(),
                        ));
                    }
                    if let Some(publication_action) =
                        inner.schema_deployments.scoped_stream_publication_action(
                            tenant,
                            &pin.scope,
                            &pin.bundle_digest,
                            entity_type,
                        )
                        && append.events.iter().any(|event| {
                            event.event_type == publication_action
                                && event.metadata.kernel.is_none()
                        })
                    {
                        return Err(PersistenceError::Storage(
                            "stream descriptor source publication fence".into(),
                        ));
                    }
                    if !inner.journals.contains_key(&append.persistence_id)
                        && !inner.schema_deployments.permits_scoped_journal_write(
                            tenant,
                            &pin.scope,
                            &pin.bundle_digest,
                        )
                    {
                        return Err(PersistenceError::Storage(
                            "stale scoped schema write fence".into(),
                        ));
                    }
                }
                if let Ok((tenant, entity_type, entity_id)) =
                    parse_persistence_id_parts(&append.persistence_id)
                    && split_scoped_journal_entity_id(entity_id).is_none()
                    && let Some((_, _, publication_action, _)) = inner
                        .unscoped_stream_fences
                        .get(&(tenant.to_string(), entity_type.to_string()))
                    && append.events.iter().any(|event| {
                        event.event_type == *publication_action && event.metadata.kernel.is_none()
                    })
                {
                    return Err(PersistenceError::Storage(
                        "stream descriptor publication fence".into(),
                    ));
                }
                let pending_cv = inner
                    .pending_concurrency_violations
                    .get(&append.persistence_id)
                    .copied()
                    .unwrap_or(0);
                if pending_cv > 0 {
                    if pending_cv == 1 {
                        inner
                            .pending_concurrency_violations
                            .remove(&append.persistence_id);
                    } else {
                        inner
                            .pending_concurrency_violations
                            .insert(append.persistence_id.clone(), pending_cv - 1);
                    }
                    return Err(PersistenceError::ConcurrencyViolation {
                        expected: append.expected_sequence,
                        actual: append.expected_sequence,
                    });
                }
            }

            // Fault injection happens before mutation so a batch either writes
            // every stream or no stream.
            let cv_prob = inner.faults.concurrency_violation_prob;
            if inner.rng.chance(cv_prob) {
                let first = &appends[0];
                return Err(PersistenceError::ConcurrencyViolation {
                    expected: first.expected_sequence,
                    actual: first.expected_sequence.wrapping_add(1),
                });
            }
            let wf_prob = inner.faults.write_failure_prob;
            if inner.rng.chance(wf_prob) {
                return Err(PersistenceError::Storage(
                    "SimEventStore: injected batch write failure".into(),
                ));
            }

            for append in appends {
                let current_seq = inner
                    .journals
                    .get(&append.persistence_id)
                    .and_then(|journal| journal.last())
                    .map(|event| event.sequence_nr)
                    .unwrap_or(0);
                if current_seq != append.expected_sequence {
                    return Err(PersistenceError::ConcurrencyViolation {
                        expected: append.expected_sequence,
                        actual: current_seq,
                    });
                }
                if let Some(first) = &append.first_event {
                    if append.expected_sequence != 0 || append.events.is_empty() {
                        return Err(PersistenceError::Storage(
                            "first-event metadata requires a non-empty sequence-0 append".into(),
                        ));
                    }
                    if first.contract_revision != first.contract.version
                        || first.schema_identity != first.contract.schema_digest
                    {
                        return Err(PersistenceError::Storage(
                            "invalid first-event metadata".into(),
                        ));
                    }
                }
                let (tenant, entity_type, entity_id) =
                    parse_persistence_id_parts(&append.persistence_id)
                        .map_err(PersistenceError::Storage)?;
                for row in &append.key_rows {
                    let key = (
                        tenant.to_string(),
                        entity_type.to_string(),
                        row.key_name.clone(),
                        row.key_hash.clone(),
                    );
                    let existing = batch_key_claims
                        .get(&key)
                        .or_else(|| inner.key_index.get(&key));
                    if existing.is_some_and(|existing| existing != entity_id) {
                        return Err(PersistenceError::Storage(format!(
                            "duplicate declared key '{}' for {entity_type}: held by {existing}",
                            row.key_name,
                            existing = existing.expect("checked as present")
                        )));
                    }
                    batch_key_claims.insert(key, entity_id.to_string());
                }
            }

            let mut prior_type_versions = BTreeMap::new();
            for append in appends {
                let (tenant, entity_type, _) = parse_persistence_id_parts(&append.persistence_id)
                    .map_err(PersistenceError::Storage)?;
                prior_type_versions
                    .entry((tenant.to_string(), entity_type.to_string()))
                    .or_insert(creation_source_write_version_locked(
                        &inner,
                        tenant,
                        entity_type,
                    )?);
            }
            let mut results = Vec::with_capacity(appends.len());
            for append in appends {
                let journal = inner
                    .journals
                    .entry(append.persistence_id.clone())
                    .or_default();
                let mut new_seq = append.expected_sequence;
                for event in &append.events {
                    new_seq += 1;
                    let mut stored = event.clone();
                    stored.sequence_nr = new_seq;
                    journal.push(stored);
                }
                results.push(PersistenceAppendResult {
                    persistence_id: append.persistence_id.clone(),
                    sequence_nr: new_seq,
                });
                if let Some(first) = &append.first_event {
                    inner
                        .creation_contracts
                        .insert(append.persistence_id.clone(), first.contract.clone());
                    inner
                        .creation_metadata
                        .insert(append.persistence_id.clone(), (first.clone(), new_seq));
                } else if let Some((_, source_sequence)) =
                    inner.creation_metadata.get_mut(&append.persistence_id)
                {
                    *source_sequence = new_seq;
                }
            }
            for append in appends {
                let (tenant, entity_type, entity_id) =
                    parse_persistence_id_parts(&append.persistence_id)
                        .map_err(PersistenceError::Storage)?;
                for row in &append.key_rows {
                    inner.key_index.retain(|(t, et, name, _), holder| {
                        !(t.as_str() == tenant
                            && et == entity_type
                            && name == &row.key_name
                            && holder == entity_id)
                    });
                    inner.key_index.insert(
                        (
                            tenant.to_string(),
                            entity_type.to_string(),
                            row.key_name.clone(),
                            row.key_hash.clone(),
                        ),
                        entity_id.to_string(),
                    );
                }
                if append.reconcile_vectors {
                    inner.vector_index.retain(|(t, et, _, _, id), _| {
                        !(t.as_str() == tenant && et == entity_type && id == entity_id)
                    });
                    for row in &append.vector_rows {
                        inner.vector_index.insert(
                            (
                                tenant.to_string(),
                                entity_type.to_string(),
                                row.decl_name.clone(),
                                row.model_tag.clone(),
                                entity_id.to_string(),
                            ),
                            row.vector.clone(),
                        );
                    }
                }
            }

            for ((tenant, entity_type), prior) in prior_type_versions {
                if let Some(append) = appends.iter().find(|append| {
                    parse_persistence_id_parts(&append.persistence_id).is_ok_and(
                        |(candidate_tenant, candidate_type, _)| {
                            candidate_tenant == tenant && candidate_type == entity_type
                        },
                    )
                }) {
                    let (_, _, entity_id) = parse_persistence_id_parts(&append.persistence_id)
                        .map_err(PersistenceError::Storage)?;
                    let sequence = inner
                        .journals
                        .get(&append.persistence_id)
                        .and_then(|events| events.last())
                        .map_or(0, |event| event.sequence_nr);
                    advance_creation_coverage_after_append_locked(
                        &mut inner,
                        &append.persistence_id,
                        &tenant,
                        &entity_type,
                        entity_id,
                        prior,
                        sequence,
                    )?;
                }
            }

            Ok(results)
        }
    };
}
