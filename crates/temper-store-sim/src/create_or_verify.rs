use super::*;

pub(super) fn commit_first_event_locked(
    inner: &mut SimEventStoreInner,
    commit: &temper_runtime::persistence::FirstEventCommit,
) -> Result<(), PersistenceError> {
    let prior_source_write_version =
        creation_source_write_version_locked(inner, &commit.tenant, &commit.entity_type)?;
    let coverage_key = (
        commit.tenant.clone(),
        commit.entity_type.clone(),
        commit.schema_identity.clone(),
        commit.contract_revision,
        commit.declared_key_signature.clone(),
    );
    let can_advance_coverage = inner
        .creation_coverage
        .get(&coverage_key)
        .is_some_and(|coverage| coverage.source_write_version == prior_source_write_version);
    let mut event = commit.event.clone();
    event.sequence_nr = 1;
    inner
        .journals
        .insert(commit.persistence_id.clone(), vec![event]);
    inner
        .creation_contracts
        .insert(commit.persistence_id.clone(), commit.contract.clone());
    if let Some(projection) = &commit.projection {
        inner
            .query_projections
            .insert(commit.persistence_id.clone(), projection.clone());
    }
    let metadata = temper_runtime::persistence::FirstEventMetadata {
        contract: commit.contract.clone(),
        contract_revision: commit.contract_revision,
        schema_identity: commit.schema_identity.clone(),
        declared_key_signature: commit.declared_key_signature.clone(),
    };
    inner
        .creation_metadata
        .insert(commit.persistence_id.clone(), (metadata.clone(), 1));
    let source_write_version =
        creation_source_write_version_locked(inner, &commit.tenant, &commit.entity_type)?;
    if prior_source_write_version == 0 {
        inner.creation_coverage.insert(
            coverage_key.clone(),
            temper_runtime::persistence::CreationCoveragePublication {
                tenant: commit.tenant.clone(),
                entity_type: commit.entity_type.clone(),
                metadata,
                cursor: commit.entity_id.clone(),
                source_write_version,
            },
        );
    } else if can_advance_coverage
        && let Some(coverage) = inner.creation_coverage.get_mut(&coverage_key)
    {
        coverage.cursor = commit.entity_id.clone();
        coverage.source_write_version = source_write_version;
    }
    inner
        .key_index
        .retain(|(tenant, entity_type, _, _), holder| {
            !(tenant == &commit.tenant
                && entity_type == &commit.entity_type
                && holder == &commit.entity_id)
        });
    for row in &commit.key_rows {
        inner.key_index.insert(
            (
                commit.tenant.clone(),
                commit.entity_type.clone(),
                row.key_name.clone(),
                row.key_hash.clone(),
            ),
            commit.entity_id.clone(),
        );
    }
    if commit.reconcile_vectors {
        for row in &commit.vector_rows {
            inner.vector_index.insert(
                (
                    commit.tenant.clone(),
                    commit.entity_type.clone(),
                    row.decl_name.clone(),
                    row.model_tag.clone(),
                    commit.entity_id.clone(),
                ),
                row.vector.clone(),
            );
        }
    }
    Ok(())
}

fn comparison_outcome(
    inner: &SimEventStoreInner,
    persistence_id: &str,
    stored: &CreationContract,
    requested: &CreationContract,
    alternate_owner: bool,
) -> Result<CreateOrVerifyStoreOutcome, PersistenceError> {
    let comparison = if alternate_owner {
        compare_creation_contracts_for_alternate_owner(stored, requested)
    } else {
        compare_creation_contracts(stored, requested)
    };
    match comparison {
        CreationContractComparison::Matches => {
            let (_, _, entity_id) =
                parse_persistence_id_parts(persistence_id).map_err(PersistenceError::Storage)?;
            let creation_exists = inner
                .journals
                .get(persistence_id)
                .is_some_and(|journal| journal.iter().any(|event| event.sequence_nr == 1));
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

pub(super) async fn run(
    store: &SimEventStore,
    request: &CreateOrVerifyRequest,
) -> Result<CreateOrVerifyStoreOutcome, PersistenceError> {
    request.first_event.validate()?;
    let mut inner = store.inner.lock().expect("SimEventStore lock poisoned"); // ci-ok: infallible lock
    let type_has_events = inner.journals.iter().any(|(persistence_id, events)| {
        !events.is_empty()
            && parse_persistence_id_parts(persistence_id).is_ok_and(|(tenant, entity_type, _)| {
                tenant == request.tenant && entity_type == request.entity_type
            })
    });
    let source_write_version =
        creation_source_write_version_locked(&inner, &request.tenant, &request.entity_type)?;
    let covered = inner
        .creation_coverage
        .get(&(
            request.tenant.clone(),
            request.entity_type.clone(),
            request.schema_identity.clone(),
            request.contract_revision,
            request.declared_key_signature.clone(),
        ))
        .is_some_and(|coverage| {
            coverage.metadata.contract_revision == request.contract_revision
                && coverage.metadata.schema_identity == request.schema_identity
                && coverage.metadata.declared_key_signature == request.declared_key_signature
                && coverage.source_write_version == source_write_version
        });
    let covered_sources = inner
        .creation_metadata
        .iter()
        .filter(|(persistence_id, (metadata, sequence))| {
            parse_persistence_id_parts(persistence_id).is_ok_and(|(tenant, entity_type, _)| {
                tenant == request.tenant
                    && entity_type == request.entity_type
                    && metadata.schema_identity == request.schema_identity
                    && metadata.contract_revision == request.contract_revision
                    && metadata.declared_key_signature == request.declared_key_signature
                    && *sequence > 0
            })
        })
        .count();
    let stream_count = inner
        .journals
        .iter()
        .filter(|(persistence_id, events)| {
            !events.is_empty()
                && parse_persistence_id_parts(persistence_id).is_ok_and(
                    |(tenant, entity_type, _)| {
                        tenant == request.tenant && entity_type == request.entity_type
                    },
                )
        })
        .count();
    if type_has_events && (!covered || covered_sources != stream_count) {
        return Ok(CreateOrVerifyStoreOutcome::CreationContractMigrationRequired);
    }
    let request_key = (
        request.tenant.clone(),
        request.module_name.clone(),
        request.entity_type.clone(),
        request.idempotency_key.clone(),
    );

    if let Some((persistence_id, requested_persistence_id, original_contract, pending)) =
        inner.create_or_verify_idempotency.get(&request_key)
    {
        if requested_persistence_id != &request.persistence_id {
            return Ok(CreateOrVerifyStoreOutcome::Conflict {
                fields: vec!["Id".to_string()],
                truncated: false,
            });
        }
        match temper_runtime::persistence::compare_creation_contracts(
            original_contract,
            &request.contract,
        ) {
            CreationContractComparison::Matches => {}
            CreationContractComparison::Conflict { fields, truncated } => {
                return Ok(CreateOrVerifyStoreOutcome::Conflict { fields, truncated });
            }
            CreationContractComparison::MigrationRequired => {
                return Ok(CreateOrVerifyStoreOutcome::CreationContractMigrationRequired);
            }
        }
        let Some(stored) = inner.creation_contracts.get(persistence_id) else {
            return Err(PersistenceError::Storage(
                "create-or-verify idempotency record has no creation contract".into(),
            ));
        };
        let mut outcome = comparison_outcome(
            &inner,
            persistence_id,
            stored,
            &request.contract,
            persistence_id != requested_persistence_id,
        )?;
        if let CreateOrVerifyStoreOutcome::AlreadyMatches {
            notification_pending,
            ..
        } = &mut outcome
        {
            *notification_pending = *pending;
        }
        return Ok(outcome);
    }

    let mut owners = BTreeMap::<String, BTreeSet<String>>::new();
    if inner
        .journals
        .get(&request.persistence_id)
        .is_some_and(|events| !events.is_empty())
    {
        owners
            .entry(request.persistence_id.clone())
            .or_default()
            .insert("Id".to_string());
    }
    for row in &request.key_rows {
        if let Some(owner_id) = inner.key_index.get(&(
            request.tenant.clone(),
            request.entity_type.clone(),
            row.key_name.clone(),
            row.key_hash.clone(),
        )) {
            let persistence_id = format!("{}:{}:{}", request.tenant, request.entity_type, owner_id);
            owners
                .entry(persistence_id)
                .or_default()
                .insert(row.key_name.clone());
        }
    }

    if owners.len() > 1 {
        let fields = owners
            .values()
            .flat_map(BTreeSet::iter)
            .cloned()
            .collect::<BTreeSet<_>>();
        let truncated =
            fields.len() > temper_runtime::persistence::CREATE_OR_VERIFY_CONFLICT_FIELD_BUDGET;
        return Ok(CreateOrVerifyStoreOutcome::Conflict {
            fields: fields
                .into_iter()
                .take(temper_runtime::persistence::CREATE_OR_VERIFY_CONFLICT_FIELD_BUDGET)
                .collect(),
            truncated,
        });
    }

    if let Some((persistence_id, _)) = owners.first_key_value() {
        let Some(stored) = inner.creation_contracts.get(persistence_id).cloned() else {
            return Ok(CreateOrVerifyStoreOutcome::CreationContractMigrationRequired);
        };
        let outcome = comparison_outcome(
            &inner,
            persistence_id,
            &stored,
            &request.contract,
            !owners
                .get(persistence_id)
                .is_some_and(|fields| fields.contains("Id")),
        )?;
        if matches!(outcome, CreateOrVerifyStoreOutcome::AlreadyMatches { .. }) {
            inner.create_or_verify_idempotency.insert(
                request_key,
                (
                    persistence_id.clone(),
                    request.persistence_id.clone(),
                    request.contract.clone(),
                    false,
                ),
            );
        }
        return Ok(outcome);
    }

    let write_failure_probability = inner.faults.write_failure_prob;
    if inner.rng.chance(write_failure_probability) {
        return Err(PersistenceError::Storage(
            "SimEventStore: injected create-or-verify write failure".into(),
        ));
    }
    commit_first_event_locked(&mut inner, &request.first_event)?;
    inner.create_or_verify_idempotency.insert(
        request_key,
        (
            request.persistence_id.clone(),
            request.persistence_id.clone(),
            request.contract.clone(),
            true,
        ),
    );
    let reply_loss_probability = inner.faults.create_or_verify_reply_loss_prob;
    if inner.rng.chance(reply_loss_probability) {
        return Err(PersistenceError::Storage(
            "SimEventStore: injected create-or-verify reply loss after commit".into(),
        ));
    }
    Ok(CreateOrVerifyStoreOutcome::Created {
        entity_id: request.entity_id.clone(),
        sequence_nr: 1,
    })
}

pub(super) async fn acknowledge(
    store: &SimEventStore,
    request: &CreateOrVerifyRequest,
) -> Result<(), PersistenceError> {
    let key = (
        request.tenant.clone(),
        request.module_name.clone(),
        request.entity_type.clone(),
        request.idempotency_key.clone(),
    );
    let mut inner = store.inner.lock().expect("SimEventStore lock poisoned"); // ci-ok: infallible lock
    let Some((_, requested_id, _, pending)) = inner.create_or_verify_idempotency.get_mut(&key)
    else {
        return Err(PersistenceError::Storage(
            "create-or-verify notification acknowledgement lost its request".into(),
        ));
    };
    if requested_id != &request.persistence_id {
        return Err(PersistenceError::Storage(
            "create-or-verify notification acknowledgement request mismatch".into(),
        ));
    }
    *pending = false;
    Ok(())
}
