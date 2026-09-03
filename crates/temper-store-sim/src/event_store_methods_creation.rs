macro_rules! impl_sim_creation_methods {
    () => {
        async fn reconcile_creation_metadata(
            &self,
            repair: &temper_runtime::persistence::CreationMetadataRepair,
        ) -> Result<(), PersistenceError> {
            repair.first_event.validate()?;
            let commit = &repair.first_event;
            let mut inner = self.inner.lock().expect("SimEventStore lock poisoned");
            let journal = inner.journals.get(&commit.persistence_id).ok_or_else(|| {
                PersistenceError::Storage("creation repair stream is absent".into())
            })?;
            if !journal.first().is_some_and(|event| {
                event.sequence_nr == 1 && event.metadata.event_id == commit.event.metadata.event_id
            }) {
                return Err(PersistenceError::Storage(
                    "creation repair first event does not match".into(),
                ));
            }
            let actual = journal.last().map_or(0, |event| event.sequence_nr);
            if actual != repair.source_sequence {
                return Err(PersistenceError::ConcurrencyViolation {
                    expected: repair.source_sequence,
                    actual,
                });
            }
            inner
                .creation_contracts
                .insert(commit.persistence_id.clone(), commit.contract.clone());
            inner.creation_metadata.insert(
                commit.persistence_id.clone(),
                (
                    FirstEventMetadata {
                        contract: commit.contract.clone(),
                        contract_revision: commit.contract_revision,
                        schema_identity: commit.schema_identity.clone(),
                        declared_key_signature: commit.declared_key_signature.clone(),
                    },
                    repair.source_sequence,
                ),
            );
            inner.key_index.retain(|(t, et, _, _), holder| {
                !(t == &commit.tenant && et == &commit.entity_type && holder == &commit.entity_id)
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
            Ok(())
        }

        async fn publish_creation_coverage(
            &self,
            publication: &temper_runtime::persistence::CreationCoveragePublication,
        ) -> Result<(), PersistenceError> {
            let mut inner = self.inner.lock().expect("SimEventStore lock poisoned");
            let actual = creation_source_write_version_locked(
                &inner,
                &publication.tenant,
                &publication.entity_type,
            )?;
            if actual != publication.source_write_version {
                return Err(PersistenceError::ConcurrencyViolation {
                    expected: publication.source_write_version,
                    actual,
                });
            }
            inner.creation_coverage.insert(
                (
                    publication.tenant.clone(),
                    publication.entity_type.clone(),
                    publication.metadata.schema_identity.clone(),
                    publication.metadata.contract_revision,
                    publication.metadata.declared_key_signature.clone(),
                ),
                publication.clone(),
            );
            Ok(())
        }

        async fn commit_first_event(
            &self,
            commit: &temper_runtime::persistence::FirstEventCommit,
        ) -> Result<u64, PersistenceError> {
            commit.validate()?;
            let mut inner = self.inner.lock().expect("SimEventStore lock poisoned");
            let current = inner
                .journals
                .get(&commit.persistence_id)
                .and_then(|events| events.last())
                .map_or(0, |event| event.sequence_nr);
            if current != 0 {
                return Err(PersistenceError::ConcurrencyViolation {
                    expected: 0,
                    actual: current,
                });
            }
            for row in &commit.key_rows {
                if inner
                    .key_index
                    .get(&(
                        commit.tenant.clone(),
                        commit.entity_type.clone(),
                        row.key_name.clone(),
                        row.key_hash.clone(),
                    ))
                    .is_some_and(|owner| owner != &commit.entity_id)
                {
                    return Err(PersistenceError::Storage(format!(
                        "duplicate declared key '{}'",
                        row.key_name
                    )));
                }
            }
            commit_first_event_locked(&mut inner, commit)?;
            Ok(1)
        }

        async fn create_or_verify(
            &self,
            request: &CreateOrVerifyRequest,
        ) -> Result<CreateOrVerifyStoreOutcome, PersistenceError> {
            create_or_verify::run(self, request).await
        }

        async fn acknowledge_create_or_verify_notification(
            &self,
            request: &CreateOrVerifyRequest,
        ) -> Result<(), PersistenceError> {
            create_or_verify::acknowledge(self, request).await
        }
    };
}
