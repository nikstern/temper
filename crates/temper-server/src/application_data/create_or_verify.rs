//! Governed atomic create-or-verify execution (ADR-0196).

mod notification;
mod reservation;

use temper_runtime::persistence::schema_deployment::scoped_journal_entity_id;
use temper_runtime::persistence::{
    CREATION_CONTRACT_VERSION_V1, CreateOrVerifyRequest, CreateOrVerifyStoreOutcome, EntityKeyRow,
    EventMetadata, FirstEventCommit, FirstEventProjection, PersistenceEnvelope,
};
use temper_runtime::scheduler::{sim_now, sim_uuid};
use temper_wasm_sdk::data::{
    CommitToken, CreateOrVerifyResultV1, DataResponseV1, DataResultV1, ModuleDataError,
    ModuleDataErrorKind,
};

use crate::entity_actor::{EntityActor, EntityEvent, SCHEMA_PIN_FIELD, schema_event_pin};

use super::{
    ApplicationDataInvocation, EntityWriteOperation, ModuleDataTarget, compile_creation_contract,
    data_error, extract_id, internal_error, short_type,
};

impl ApplicationDataInvocation {
    pub(super) async fn entity_create_or_verify(
        &self,
        entity_type: &str,
        idempotency_key: &str,
        mut value: serde_json::Map<String, serde_json::Value>,
    ) -> Result<DataResultV1, ModuleDataError> {
        self.require(
            temper_wasm_sdk::data::DataOperationKind::EntityCreateOrVerify,
            entity_type,
            None,
        )?;
        self.validate_entity_object(entity_type, &value, EntityWriteOperation::Create)?;
        let entity_id = extract_id(&value)?;
        value
            .entry("Id")
            .or_insert_with(|| entity_id.clone().into());
        let manifest = self
            .authority
            .binding
            .entities
            .iter()
            .find(|entity| entity.entity_type == entity_type)
            .expect("granted entity type must exist in the bound schema");
        super::materialize_creation_fields(manifest, &mut value);

        self.reserve_create_or_verify_response(entity_type)?;
        self.authorize_value(
            "create_or_verify",
            entity_type,
            Some(&entity_id),
            Some(&value),
        )?;

        let fields = serde_json::Value::Object(value.clone());
        let _guard = self
            .state
            .acquire_commons_write_guardrail_lock(&self.authority.tenant)
            .await;
        self.run_governed_write_prechecks(
            entity_type,
            &entity_id,
            "Create",
            "create_or_verify",
            &fields,
            false,
        )
        .await?;

        let schema_digest = match &self.authority.target {
            ModuleDataTarget::TenantGlobal => self.authority.binding.schema_digest.as_str(),
            ModuleDataTarget::Scoped(pin) => pin.bundle_digest.as_str(),
        };
        let contract = compile_creation_contract(manifest, schema_digest, &value)?;
        let runtime_type = short_type(entity_type);
        let declared_keys = self
            .state
            .declared_keys_for(&self.authority.tenant, runtime_type);
        let mut key_rows = declared_keys
            .iter()
            .filter_map(|key| {
                crate::key_index::canonical_key_hash(&key.name, &key.properties, &value).map(
                    |key_hash| EntityKeyRow {
                        key_name: key.name.clone(),
                        key_hash,
                    },
                )
            })
            .collect::<Vec<_>>();
        key_rows.sort_by(|left, right| {
            (&left.key_name, &left.key_hash).cmp(&(&right.key_name, &right.key_hash))
        });
        let declared_key_signature = super::declared_key_signature(&declared_keys, &contract);

        let journal_entity_id = match &self.authority.target {
            ModuleDataTarget::TenantGlobal => entity_id.clone(),
            ModuleDataTarget::Scoped(pin) => scoped_journal_entity_id(&entity_id, pin),
        };
        let persistence_id = format!(
            "{}:{runtime_type}:{journal_entity_id}",
            self.authority.tenant
        );
        let timestamp = sim_now();
        let created = EntityEvent {
            action: "Created".to_string(),
            from_status: String::new(),
            to_status: self.initial_status(runtime_type)?,
            timestamp,
            params: serde_json::Value::Object(value.clone()),
            idempotency_key: None,
        };
        let mut payload =
            serde_json::to_value(&created).map_err(|error| internal_error(error.to_string()))?;
        if let Some(pin) = self.authority.target.schema_pin() {
            payload
                .as_object_mut()
                .expect("serialized entity event must be an object")
                .insert(
                    SCHEMA_PIN_FIELD.to_string(),
                    serde_json::to_value(schema_event_pin(pin, runtime_type, "Created"))
                        .map_err(|error| internal_error(error.to_string()))?,
                );
        }
        let table = self.initial_table(runtime_type)?;
        let initial_fields = serde_json::Value::Object(value.clone());
        let mut projection_state =
            EntityActor::build_initial_state(runtime_type, &entity_id, &table, &initial_fields);
        projection_state
            .fields
            .as_object_mut()
            .expect("entity projection fields must be an object")
            .insert(
                "Id".to_string(),
                serde_json::Value::String(entity_id.clone()),
            );
        let Some((store, backend)) = self.state.event_journal() else {
            return Err(data_error(
                ModuleDataErrorKind::ConsistencyUnavailable,
                "AtomicCreateOrVerifyUnavailable",
                "atomic create-or-verify requires a durable event journal",
            ));
        };
        let blob_store = self
            .state
            .blob_store_for_tenant(&self.authority.tenant)
            .ok();
        let _overflow_blobs = crate::entity_actor::effects::sync_fields_with_metadata(
            &mut projection_state,
            &created.params,
            EntityActor::field_sync_mode_for_backend(backend.into(), blob_store.as_ref()),
            Some(&table.state_var_metadata),
        );
        let reaction_context = self
            .creation_reaction_context(runtime_type, &projection_state.fields)
            .await?;
        let mut intent_actor = EntityActor::new(
            runtime_type,
            &entity_id,
            std::sync::Arc::new(std::sync::RwLock::new((*table).clone())),
            initial_fields.clone(),
        )
        .with_tenant(self.authority.tenant.as_str());
        if let Some(pin) = self.authority.target.schema_pin() {
            intent_actor = intent_actor.with_schema_pin(pin.clone());
        }
        intent_actor
            .attach_durable_intents(
                &mut payload,
                &projection_state,
                &created,
                reaction_context.as_ref(),
                None,
            )
            .map_err(|error| internal_error(error.to_string()))?;
        let envelope = PersistenceEnvelope {
            sequence_nr: 1,
            event_type: "Created".to_string(),
            payload,
            metadata: EventMetadata {
                event_id: sim_uuid(),
                causation_id: sim_uuid(),
                correlation_id: sim_uuid(),
                timestamp,
                actor_id: persistence_id.clone(),
                kernel: None,
            },
        };
        projection_state.sequence_nr = 1;
        projection_state.record_committed_event(created, 1);
        let projection_fields = self.state.query_projection_fields(
            &self.authority.tenant,
            runtime_type,
            &projection_state.fields,
        );
        let projection = FirstEventProjection {
            status: projection_state.status.clone(),
            fields: projection_fields,
            state: self.state.query_projection_state(&projection_state),
            sequence_nr: 1,
        };
        let request = CreateOrVerifyRequest {
            module_name: self.authority.module_name.clone(),
            idempotency_key: idempotency_key.to_string(),
            first_event: FirstEventCommit {
                tenant: self.authority.tenant.to_string(),
                entity_type: runtime_type.to_string(),
                entity_id: journal_entity_id,
                persistence_id,
                event: envelope,
                contract,
                contract_revision: CREATION_CONTRACT_VERSION_V1,
                schema_identity: schema_digest.to_string(),
                declared_key_signature,
                key_rows,
                vector_rows: Vec::new(),
                reconcile_vectors: false,
                projection: Some(projection),
            },
        };
        request
            .first_event
            .validate()
            .map_err(|error| internal_error(error.to_string()))?;
        let outcome = store
            .create_or_verify(&request)
            .await
            .map_err(|error| internal_error(error.to_string()))?;

        match outcome {
            CreateOrVerifyStoreOutcome::Created {
                entity_id: winning_id,
                sequence_nr,
            } => {
                self.successful_create_or_verify(
                    entity_type,
                    &winning_id,
                    sequence_nr,
                    true,
                    true,
                    &request,
                )
                .await
            }
            CreateOrVerifyStoreOutcome::AlreadyMatches {
                entity_id: winning_id,
                sequence_nr,
                notification_pending,
            } => {
                self.successful_create_or_verify(
                    entity_type,
                    &winning_id,
                    sequence_nr,
                    false,
                    notification_pending,
                    &request,
                )
                .await
            }
            CreateOrVerifyStoreOutcome::Conflict { fields, truncated } => {
                if let Ok(response) = self
                    .get_durable_target_entity(entity_type, &request.entity_id)
                    .await
                    && response.state.status == "Deleted"
                {
                    return Ok(DataResultV1::CreateOrVerify {
                        outcome: CreateOrVerifyResultV1::Conflict {
                            fields: vec![self.lifecycle_field(entity_type)],
                            truncated: false,
                        },
                    });
                }
                Ok(DataResultV1::CreateOrVerify {
                    outcome: CreateOrVerifyResultV1::Conflict { fields, truncated },
                })
            }
            CreateOrVerifyStoreOutcome::CreationContractMigrationRequired => Err(data_error(
                ModuleDataErrorKind::SchemaMismatch,
                "CreationContractMigrationRequired",
                "the stored creation contract requires an explicit schema migration",
            )),
        }
    }

    async fn successful_create_or_verify(
        &self,
        entity_type: &str,
        winning_journal_id: &str,
        sequence_nr: u64,
        created: bool,
        notification_pending: bool,
        request: &CreateOrVerifyRequest,
    ) -> Result<DataResultV1, ModuleDataError> {
        let entity_id = logical_entity_id(winning_journal_id);
        let response = self
            .get_durable_target_entity(entity_type, winning_journal_id)
            .await?;
        if response.state.sequence_nr < sequence_nr {
            return Err(data_error(
                ModuleDataErrorKind::ConsistencyUnavailable,
                "ConsistencyUnavailable",
                "authoritative actor state has not reached the committed sequence",
            ));
        }
        if response.state.status == "Deleted" {
            return Ok(DataResultV1::CreateOrVerify {
                outcome: CreateOrVerifyResultV1::Conflict {
                    fields: vec![self.lifecycle_field(entity_type)],
                    truncated: false,
                },
            });
        }
        let Some((store, backend)) = self.state.event_journal() else {
            return Err(data_error(
                ModuleDataErrorKind::ConsistencyUnavailable,
                "AtomicCreateOrVerifyUnavailable",
                "atomic create-or-verify requires a durable event journal",
            ));
        };
        if notification_pending {
            self.recover_creation_notification(entity_type, winning_journal_id, &entity_id, &store)
                .await?;
            // The per-entity SSE replay path is journal-backed, so the
            // sequence-one event is externally recoverable even if this
            // process exits after clearing the delivery reservation.
            store
                .acknowledge_create_or_verify_notification(request)
                .await
                .map_err(|error| internal_error(error.to_string()))?;
        }
        match store
            .create_or_verify(request)
            .await
            .map_err(|error| internal_error(error.to_string()))?
        {
            CreateOrVerifyStoreOutcome::AlreadyMatches {
                entity_id: verified_id,
                sequence_nr: verified_sequence,
                ..
            } if verified_id == winning_journal_id
                && verified_sequence <= response.state.sequence_nr => {}
            _ => {
                return Err(data_error(
                    ModuleDataErrorKind::ConsistencyUnavailable,
                    "CreationContractRevalidationFailed",
                    "authoritative state no longer agrees with its immutable creation contract",
                ));
            }
        }
        let table = self.initial_table(short_type(entity_type))?;
        let creation_event: EntityEvent =
            serde_json::from_value(request.first_event.event.payload.clone())
                .map_err(|error| internal_error(error.to_string()))?;
        let mut blob_repair_state = EntityActor::build_initial_state(
            short_type(entity_type),
            &entity_id,
            &table,
            &creation_event.params,
        );
        let blob_store = self
            .state
            .blob_store_for_tenant(&self.authority.tenant)
            .ok();
        let overflow_blobs = crate::entity_actor::effects::sync_fields_with_metadata(
            &mut blob_repair_state,
            &creation_event.params,
            EntityActor::field_sync_mode_for_backend(backend.into(), blob_store.as_ref()),
            Some(&table.state_var_metadata),
        );
        if !overflow_blobs.is_empty() {
            EntityActor::persist_overflow_blobs(blob_store.as_ref(), &overflow_blobs)
                .await
                .map_err(internal_error)?;
        }
        let mut bounded_state = response.state.clone();
        crate::blobs::hydrate_blob_refs_for_tenant(
            &self.state,
            &self.authority.tenant,
            &mut bounded_state.fields,
        )
        .await;
        let value = self.canonical_entity_value(entity_type, &bounded_state)?;
        let commit = CommitToken {
            entity_type: entity_type.to_string(),
            entity_id: entity_id.to_string(),
            sequence: response.state.sequence_nr,
        };
        let outcome = if created {
            CreateOrVerifyResultV1::Created { commit, value }
        } else {
            CreateOrVerifyResultV1::AlreadyMatches { commit, value }
        };
        let result = DataResultV1::CreateOrVerify { outcome };
        let encoded = serde_json::to_vec(&DataResponseV1::ok(result.clone()))
            .map_err(|error| internal_error(error.to_string()))?;
        if encoded.len() > self.authority.binding.grant.budgets.max_response_bytes as usize {
            return Err(data_error(
                ModuleDataErrorKind::Internal,
                "CreateOrVerifyResponseReservationInvariant",
                "reserved create-or-verify response exceeded its pre-commit bound",
            ));
        }
        Ok(result)
    }

    async fn creation_reaction_context(
        &self,
        entity_type: &str,
        fields: &serde_json::Value,
    ) -> Result<Option<crate::trigger::delivery::ReactionCommitContext>, ModuleDataError> {
        let dispatcher = self
            .state
            .reaction_dispatcher
            .read()
            .map_err(|_| internal_error("reaction dispatcher lock poisoned".to_string()))?
            .clone();
        let Some(dispatcher) = dispatcher else {
            return Ok(None);
        };
        let rules = match &self.authority.target {
            ModuleDataTarget::TenantGlobal => {
                dispatcher.candidate_rules(&self.authority.tenant, entity_type, "Created")
            }
            ModuleDataTarget::Scoped(pin) => self
                .state
                .registry
                .read()
                .map_err(|_| internal_error("schema registry lock poisoned".to_string()))?
                .scoped_reaction_candidates_at_digest(
                    &self.authority.tenant,
                    &pin.scope,
                    &pin.bundle_digest,
                    entity_type,
                    "Created",
                ),
        };
        let resolved_guards = crate::trigger::dispatcher::resolve_rule_guard_inputs(
            &self.state,
            &self.authority.tenant,
            &rules,
            fields,
            self.authority.target.schema_pin(),
        )
        .await;
        Ok(Some(crate::trigger::delivery::ReactionCommitContext {
            rules,
            authority: serde_json::to_value(&self.authority.security)
                .map_err(|error| internal_error(error.to_string()))?,
            depth: 0,
            root_delivery_id: None,
            expected_source_sequence: 0,
            resolved_guards,
            receipt: None,
        }))
    }
}

fn logical_entity_id(entity_id: &str) -> String {
    temper_runtime::persistence::schema_deployment::split_scoped_journal_entity_id(entity_id)
        .map_or_else(|| entity_id.to_string(), |(logical, _)| logical.to_string())
}
