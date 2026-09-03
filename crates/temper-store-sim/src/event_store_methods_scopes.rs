macro_rules! impl_sim_scopes_methods {
    () => {
    async fn list_unscoped_entity_ids_page(
        &self,
        tenant: &str,
        entity_type: &str,
        after_entity_id: Option<&str>,
        limit: usize,
    ) -> Result<Vec<String>, PersistenceError> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let inner = self.inner.lock().expect("SimEventStore lock poisoned"); // ci-ok: infallible lock
        Ok(inner
            .journals
            .keys()
            .filter_map(|persistence_id| {
                parse_persistence_id_parts(persistence_id)
                    .ok()
                    .filter(|(found_tenant, found_type, entity_id)| {
                        *found_tenant == tenant
                            && *found_type == entity_type
                            && split_scoped_journal_entity_id(entity_id).is_none()
                    })
                    .map(|(_, _, entity_id)| entity_id.to_string())
            })
            .filter(|entity_id| after_entity_id.is_none_or(|after| entity_id.as_str() > after))
            .take(limit)
            .collect())
    }

    async fn unscoped_entity_type_write_version(
        &self,
        tenant: &str,
        entity_type: &str,
    ) -> Result<u64, PersistenceError> {
        let inner = self.inner.lock().expect("SimEventStore lock poisoned"); // ci-ok: infallible lock
        inner
            .journals
            .iter()
            .filter_map(|(persistence_id, events)| {
                parse_persistence_id_parts(persistence_id)
                    .ok()
                    .filter(|(found_tenant, found_type, entity_id)| {
                        *found_tenant == tenant
                            && *found_type == entity_type
                            && split_scoped_journal_entity_id(entity_id).is_none()
                    })
                    .map(|_| events.len())
            })
            .try_fold(0_u64, |total, count| {
                u64::try_from(count)
                    .ok()
                    .and_then(|count| total.checked_add(count))
            })
            .ok_or_else(|| PersistenceError::Storage("global write version exhausted".into()))
    }

    async fn activate_unscoped_stream_publication_fence(
        &self,
        tenant: &str,
        fence: &temper_runtime::persistence::schema_deployment::StreamPublicationFence,
    ) -> Result<(), PersistenceError> {
        let temper_runtime::persistence::schema_deployment::StreamPublicationFence::InstalledApplication {
            application_id,
            semantic_digest,
            bindings,
        } = fence
        else {
            return Err(PersistenceError::Storage(
                "task fence cannot activate global stream publications".into(),
            ));
        };
        let mut inner = self.inner.lock().expect("SimEventStore lock poisoned"); // ci-ok: infallible lock
        for (entity_type, binding) in bindings {
            let version = inner
                .journals
                .iter()
                .filter_map(|(persistence_id, events)| {
                    parse_persistence_id_parts(persistence_id)
                        .ok()
                        .filter(|(found_tenant, found_type, entity_id)| {
                            *found_tenant == tenant
                                && *found_type == entity_type
                                && split_scoped_journal_entity_id(entity_id).is_none()
                        })
                        .map(|_| events.len())
                })
                .try_fold(0_u64, |total, count| {
                    u64::try_from(count)
                        .ok()
                        .and_then(|count| total.checked_add(count))
                })
                .ok_or_else(|| {
                    PersistenceError::Storage("global write version exhausted".into())
                })?;
            if version != binding.expected_write_version {
                return Err(PersistenceError::ConcurrencyViolation {
                    expected: binding.expected_write_version,
                    actual: version,
                });
            }
        }
        inner
            .unscoped_stream_fences
            .retain(|(found_tenant, _), (found_application, _, _, _)| {
                found_tenant != tenant || found_application != application_id
            });
        for (entity_type, binding) in bindings {
            inner.unscoped_stream_fences.insert(
                (tenant.to_string(), entity_type.clone()),
                (
                    application_id.clone(),
                    semantic_digest.clone(),
                    binding.publication_action.clone(),
                    binding.capability_digest.clone(),
                ),
            );
        }
        Ok(())
    }

    async fn deactivate_unscoped_stream_publication_fence(
        &self,
        tenant: &str,
        application_id: &str,
        semantic_digest: Option<&str>,
    ) -> Result<(), PersistenceError> {
        let mut inner = self.inner.lock().expect("SimEventStore lock poisoned"); // ci-ok: infallible lock
        inner.unscoped_stream_fences.retain(
            |(found_tenant, _), (found_application, found_digest, _, _)| {
                found_tenant != tenant
                    || found_application != application_id
                    || semantic_digest.is_some_and(|expected| found_digest != expected)
            },
        );
        Ok(())
    }

    async fn get_unscoped_stream_publication_fence(
        &self,
        tenant: &str,
        application_id: &str,
    ) -> Result<
        Option<temper_runtime::persistence::schema_deployment::StreamPublicationFence>,
        PersistenceError,
    > {
        let inner = self.inner.lock().expect("SimEventStore lock poisoned"); // ci-ok: infallible lock
        let mut semantic_digest = None;
        let mut bindings = BTreeMap::new();
        for (
            (found_tenant, entity_type),
            (found_application, found_digest, action, capability_digest),
        ) in &inner.unscoped_stream_fences
        {
            if found_tenant != tenant || found_application != application_id {
                continue;
            }
            if semantic_digest
                .as_ref()
                .is_some_and(|existing| existing != found_digest)
            {
                return Err(PersistenceError::Storage(
                    "installed application publication fence is inconsistent".into(),
                ));
            }
            semantic_digest = Some(found_digest.clone());
            let version = inner
                .journals
                .iter()
                .filter_map(|(persistence_id, events)| {
                    parse_persistence_id_parts(persistence_id)
                        .ok()
                        .filter(|(journal_tenant, journal_type, entity_id)| {
                            *journal_tenant == tenant
                                && *journal_type == entity_type
                                && split_scoped_journal_entity_id(entity_id).is_none()
                        })
                        .map(|_| events.len())
                })
                .try_fold(0_u64, |total, count| {
                    u64::try_from(count)
                        .ok()
                        .and_then(|count| total.checked_add(count))
                })
                .ok_or_else(|| {
                    PersistenceError::Storage("global write version exhausted".into())
                })?;
            bindings.insert(
                entity_type.clone(),
                temper_runtime::persistence::schema_deployment::UnscopedStreamPublicationBinding {
                    publication_action: action.clone(),
                    capability_digest: capability_digest.clone(),
                    expected_write_version: version,
                },
            );
        }
        Ok(semantic_digest.map(|semantic_digest| {
            temper_runtime::persistence::schema_deployment::StreamPublicationFence::InstalledApplication {
                application_id: application_id.into(),
                semantic_digest,
                bindings,
            }
        }))
    }

    async fn unscoped_stream_publication_fence_active(
        &self,
        tenant: &str,
        entity_type: &str,
        publication_action: &str,
        capability_digest: &str,
    ) -> Result<bool, PersistenceError> {
        let fault_key = format!("{tenant}:_TemperStreamPublicationFence:{entity_type}");
        let mut inner = self.inner.lock().expect("SimEventStore lock poisoned"); // ci-ok: infallible lock
        if let Some(remaining) = inner.pending_read_failures.get_mut(&fault_key) {
            *remaining -= 1;
            let cleared = *remaining == 0;
            if cleared {
                inner.pending_read_failures.remove(&fault_key);
            }
            return Err(PersistenceError::Storage(format!(
                "injected read failure for {fault_key}"
            )));
        }
        Ok(inner
            .unscoped_stream_fences
            .get(&(tenant.to_string(), entity_type.to_string()))
            .is_some_and(|(_, _, found_action, found_capability_digest)| {
                found_action == publication_action && found_capability_digest == capability_digest
            }))
    }

    async fn restore_unscoped_stream_publication_fence(
        &self,
        tenant: &str,
        expected_current_semantic_digest: &str,
        fence: &temper_runtime::persistence::schema_deployment::StreamPublicationFence,
    ) -> Result<(), PersistenceError> {
        let temper_runtime::persistence::schema_deployment::StreamPublicationFence::InstalledApplication {
            application_id,
            semantic_digest,
            bindings,
        } = fence
        else {
            return Err(PersistenceError::Storage(
                "task fence cannot restore global stream publications".into(),
            ));
        };
        let mut inner = self.inner.lock().expect("SimEventStore lock poisoned"); // ci-ok: infallible lock
        let current_matches = inner.unscoped_stream_fences.iter().any(
            |((found_tenant, _), (found_application, found_digest, _, _))| {
                found_tenant == tenant
                    && found_application == application_id
                    && found_digest == expected_current_semantic_digest
            },
        );
        if !current_matches {
            return Err(PersistenceError::Storage(
                "installed application publication fence changed concurrently".into(),
            ));
        }
        inner
            .unscoped_stream_fences
            .retain(|(found_tenant, _), (found_application, _, _, _)| {
                found_tenant != tenant || found_application != application_id
            });
        for (entity_type, binding) in bindings {
            inner.unscoped_stream_fences.insert(
                (tenant.into(), entity_type.clone()),
                (
                    application_id.clone(),
                    semantic_digest.clone(),
                    binding.publication_action.clone(),
                    binding.capability_digest.clone(),
                ),
            );
        }
        Ok(())
    }

    async fn list_scoped_entity_ids_page(
        &self,
        tenant: &str,
        entity_type: &str,
        scope: &SchemaScope,
        bundle_digest: &str,
        after_entity_id: Option<&str>,
        limit: usize,
    ) -> Result<Vec<String>, PersistenceError> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let suffix = scoped_journal_pin_suffix(&SchemaExecutionPin {
            scope: scope.clone(),
            bundle_digest: bundle_digest.to_string(),
        });
        let inner = self.inner.lock().expect("SimEventStore lock poisoned"); // ci-ok: infallible lock
        Ok(inner
            .journals
            .keys()
            .filter_map(|persistence_id| {
                parse_persistence_id_parts(persistence_id)
                    .ok()
                    .filter(|(found_tenant, found_type, _)| {
                        *found_tenant == tenant && *found_type == entity_type
                    })
                    .and_then(|(_, _, journal_entity_id)| journal_entity_id.strip_suffix(&suffix))
            })
            .filter(|entity_id| after_entity_id.is_none_or(|after| *entity_id > after))
            .take(limit)
            .map(str::to_string)
            .collect())
    }

    async fn scoped_entity_bundle_digests(
        &self,
        tenant: &str,
        entity_type: &str,
        entity_id: &str,
        scope: &SchemaScope,
        limit: usize,
    ) -> Result<Vec<String>, PersistenceError> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let inner = self.inner.lock().expect("SimEventStore lock poisoned"); // ci-ok: infallible lock
        Ok(inner
            .journals
            .keys()
            .filter_map(|persistence_id| {
                parse_persistence_id_parts(persistence_id)
                    .ok()
                    .filter(|(found_tenant, found_type, _)| {
                        *found_tenant == tenant && *found_type == entity_type
                    })
                    .and_then(|(_, _, journal_entity_id)| {
                        split_scoped_journal_entity_id(journal_entity_id)
                    })
                    .filter(|(found_entity_id, pin)| {
                        *found_entity_id == entity_id && &pin.scope == scope
                    })
                    .map(|(_, pin)| pin.bundle_digest)
            })
            .take(limit)
            .collect())
    }

    async fn scoped_bundle_write_version(
        &self,
        tenant: &str,
        scope: &SchemaScope,
        bundle_digest: &str,
    ) -> Result<u64, PersistenceError> {
        let suffix = scoped_journal_pin_suffix(&SchemaExecutionPin {
            scope: scope.clone(),
            bundle_digest: bundle_digest.to_string(),
        });
        let inner = self.inner.lock().expect("SimEventStore lock poisoned"); // ci-ok: infallible lock
        inner
            .journals
            .iter()
            .filter_map(|(persistence_id, events)| {
                parse_persistence_id_parts(persistence_id)
                    .ok()
                    .filter(|(found_tenant, _, entity_id)| {
                        *found_tenant == tenant && entity_id.ends_with(&suffix)
                    })
                    .map(|_| events.len())
            })
            .try_fold(0_u64, |version, count| {
                version
                    .checked_add(u64::try_from(count).map_err(|_| {
                        PersistenceError::Storage("schema write version exhausted".into())
                    })?)
                    .ok_or_else(|| {
                        PersistenceError::Storage("schema write version exhausted".into())
                    })
            })
    }
    };
}
