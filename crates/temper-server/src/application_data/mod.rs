//! Governed, transport-neutral application-data service for WASM modules.

mod action;
mod authority;
mod batch;
mod create_or_verify;
mod creation_contract;
mod helpers;
mod invocation;
mod protocol;
mod query;
mod schema;
mod schema_deployment;
mod service;
mod streams;
mod target;
mod telemetry;

#[cfg(feature = "test-helpers")]
/// Canonicalize committed entity state with the production module-data response path.
pub use schema::canonicalize_entity_for_test;

pub(crate) use schema::EntityWriteOperation;
pub(crate) use schema::canonical_manifest_entity_value_from_parts;
pub(crate) use schema::{validate_manifest_action_params, validate_manifest_entity_object};
pub(crate) use service::GovernedApplicationDataService;

use temper_wasm_sdk::data::{
    DATA_ABI_VERSION_V1, DATA_ABI_VERSION_V2, DataOperationKind, DataOperationV1, DataRequestV1,
    DataRequestV2, DataResponseV1, DataResultV1, ModuleDataError, ModuleDataErrorKind,
};

pub(crate) use creation_contract::{
    compile_creation_contract, declared_key_signature, materialize_actor_creation_fields,
    materialize_creation_fields,
};
use helpers::{
    applied_internal_error, commit, compact_committed_results, error_with_outcome, extract_id,
    mark_applied, not_applied_error, not_applied_internal_error, read_service_error, short_type,
    unknown_internal_error, validate_value_budget, write_result, write_service_error,
};
pub(crate) use invocation::{
    ApplicationDataInvocation, ModuleDataTarget, ModuleInvocationAuthority,
};
use protocol::{abi_mismatch, decode_request, encode_response, response_outcome};
use telemetry::{record_operation_fields, result_kind};

#[cfg(test)]
mod canonical_defaults_tests;
#[cfg(all(test, feature = "sim"))]
mod create_or_verify_dst_tests;
#[cfg(test)]
mod create_or_verify_fault_tests;
#[cfg(all(test, feature = "sim"))]
mod create_or_verify_tests;
#[cfg(test)]
mod create_reconciliation_fault_tests;
#[cfg(test)]
mod delete_fault_tests;
#[cfg(test)]
mod entity_action_result_tests;
#[cfg(test)]
mod lifecycle_provenance_tests;
#[cfg(test)]
mod parity_tests;
#[cfg(all(test, feature = "sim"))]
mod schema_deployment_tests;
#[cfg(all(test, feature = "sim"))]
mod scoped_tests;
#[cfg(test)]
mod telemetry_span_tests;
#[cfg(test)]
mod tests;

impl ApplicationDataInvocation {
    #[tracing::instrument(
        skip_all,
        fields(
            otel.name = "wasm.application_data.call",
            tenant = %self.authority.tenant,
            module = %self.authority.module_name,
            artifact = %self.authority.artifact_digest,
            grant = %self.authority.grant_digest,
            request_bytes = bytes.len(),
            abi_version = tracing::field::Empty,
            adapter = "module_sdk",
            operation_kind = tracing::field::Empty,
            entity_type = tracing::field::Empty,
            action = tracing::field::Empty,
            result_kind = tracing::field::Empty,
            consistency_path = tracing::field::Empty,
            batch_count = tracing::field::Empty,
            response_bytes = tracing::field::Empty,
            outcome = tracing::field::Empty
        )
    )]
    pub(super) async fn call_encoded(&self, bytes: &[u8]) -> Result<Vec<u8>, String> {
        if let Ok(request) = serde_json::from_slice::<
            temper_wasm_sdk::schema_deployment::SchemaDeploymentRequestV1,
        >(bytes)
        {
            return self
                .call_schema_deployment_encoded(bytes.len(), request)
                .await;
        }
        let abi = self.authority.binding.abi;
        tracing::Span::current().record("abi_version", abi);
        let mut response = match self.admit_call(bytes) {
            Ok(operation) => {
                record_operation_fields(&operation);
                self.execute(operation).await
            }
            Err(error) => DataResponseV1::error(error),
        };
        let mut encoded = encode_response(abi, &response)?;
        if encoded.len() > self.authority.binding.grant.budgets.max_response_bytes as usize {
            compact_committed_results(&mut response);
            encoded = encode_response(abi, &response)?;
        }
        if encoded.len() > self.authority.binding.grant.budgets.max_response_bytes as usize {
            let error = error_with_outcome(
                ModuleDataErrorKind::BudgetExceeded,
                "ResponseBudgetExceeded",
                "application-data response exceeded the invocation budget",
                response_outcome(&response),
            );
            return encode_response(abi, &DataResponseV1::error(error));
        }
        let span = tracing::Span::current();
        span.record("result_kind", result_kind(&response));
        span.record("response_bytes", encoded.len());
        span.record(
            "outcome",
            if matches!(
                response.outcome,
                temper_wasm_sdk::data::DataOutcomeV1::Ok { .. }
            ) {
                "ok"
            } else {
                "error"
            },
        );
        Ok(encoded)
    }

    fn admit_call(&self, bytes: &[u8]) -> Result<DataOperationV1, ModuleDataError> {
        if bytes.len() > self.authority.binding.grant.budgets.max_request_bytes as usize {
            return Err(not_applied_error(
                ModuleDataErrorKind::BudgetExceeded,
                "RequestBudgetExceeded",
                "application-data request exceeded the invocation budget",
            ));
        }
        let mut calls = self.calls.lock().map_err(|_| {
            not_applied_error(
                ModuleDataErrorKind::Internal,
                "InvocationStatePoisoned",
                "application-data invocation state is unavailable",
            )
        })?;
        *calls = calls.saturating_add(1);
        if *calls > self.authority.binding.grant.budgets.max_calls {
            return Err(not_applied_error(
                ModuleDataErrorKind::BudgetExceeded,
                "CallBudgetExceeded",
                "application-data call budget exhausted",
            ));
        }
        let operation = match self.authority.binding.abi {
            DATA_ABI_VERSION_V1 => {
                let request: DataRequestV1 = decode_request(bytes)?;
                if request.abi != DATA_ABI_VERSION_V1 {
                    return Err(abi_mismatch());
                }
                request.operation
            }
            DATA_ABI_VERSION_V2 => {
                let request: DataRequestV2 = decode_request(bytes)?;
                if request.abi != DATA_ABI_VERSION_V2 {
                    return Err(abi_mismatch());
                }
                request.operation
            }
            _ => return Err(abi_mismatch()),
        };
        let operation_value = serde_json::to_value(&operation).map_err(|_| {
            not_applied_error(
                ModuleDataErrorKind::InvalidRequest,
                "InvalidRequest",
                "request payload could not be validated",
            )
        })?;
        let mut nodes = 0;
        validate_value_budget(
            &operation_value,
            0,
            &mut nodes,
            self.authority.binding.grant.budgets.max_request_bytes as usize,
        )?;
        helpers::validate_operation_identifiers(&operation)?;
        helpers::reserve_compact_response(
            &operation,
            self.authority.binding.grant.budgets.max_response_bytes as usize,
        )?;
        Ok(operation)
    }

    async fn execute(&self, operation: DataOperationV1) -> DataResponseV1 {
        match self.execute_inner(operation).await {
            Ok(result) => DataResponseV1::ok(result),
            Err(error) => DataResponseV1::error(error),
        }
    }

    async fn execute_inner(
        &self,
        operation: DataOperationV1,
    ) -> Result<DataResultV1, ModuleDataError> {
        match operation {
            DataOperationV1::EntityGet {
                entity_type,
                entity_id,
                at_least_sequence,
            } => {
                self.entity_get(&entity_type, &entity_id, at_least_sequence)
                    .await
            }
            DataOperationV1::EntityQuery {
                entity_type,
                filter,
                order_by,
                page,
            } => {
                self.entity_query(&entity_type, filter.as_ref(), &order_by, &page)
                    .await
            }
            DataOperationV1::EntityCreate { entity_type, value } => {
                self.entity_create(&entity_type, value).await
            }
            DataOperationV1::EntityCreateOrVerify {
                entity_type,
                idempotency_key,
                value,
            } => {
                self.entity_create_or_verify(&entity_type, &idempotency_key, value)
                    .await
            }
            DataOperationV1::EntityPatch {
                entity_type,
                entity_id,
                expected_sequence,
                value,
            } => {
                self.entity_patch(&entity_type, &entity_id, expected_sequence, value)
                    .await
            }
            DataOperationV1::ActionInvoke {
                entity_type,
                entity_id,
                action,
                expected_sequence,
                params,
            } => {
                self.action_invoke(
                    DataOperationKind::ActionInvoke,
                    &entity_type,
                    &entity_id,
                    &action,
                    expected_sequence,
                    params,
                )
                .await
            }
            DataOperationV1::CompositeInvoke {
                entity_type,
                entity_id,
                action,
                expected_sequence,
                params,
            } => {
                self.action_invoke(
                    DataOperationKind::CompositeInvoke,
                    &entity_type,
                    &entity_id,
                    &action,
                    expected_sequence,
                    params,
                )
                .await
            }
            DataOperationV1::Batch { items } => self.batch(items).await,
            DataOperationV1::FileReadOpen {
                file_id,
                version_id,
            } => self.file_read_open(file_id, version_id).await,
            DataOperationV1::FileWriteOpen {
                file_id,
                expected_sequence,
                content_length,
                content_hash,
            } => {
                self.file_write_open(file_id, expected_sequence, content_length, content_hash)
                    .await
            }
            DataOperationV1::FileWriteCommit { stream_handle } => {
                self.file_write_commit(stream_handle).await
            }
            DataOperationV1::FileStreamAbort { stream_handle } => self.file_abort(stream_handle),
        }
    }

    async fn entity_get(
        &self,
        entity_type: &str,
        entity_id: &str,
        minimum: Option<u64>,
    ) -> Result<DataResultV1, ModuleDataError> {
        self.require(DataOperationKind::EntityGet, entity_type, None)?;
        tracing::Span::current().record("consistency_path", "authoritative");
        let response = self.get_target_entity(entity_type, entity_id).await?;
        let value = self.canonical_entity_value(entity_type, &response.state)?;
        self.authorize_value("read", entity_type, Some(entity_id), Some(&value))?;
        if minimum.is_some_and(|minimum| response.state.sequence_nr < minimum) {
            return Err(not_applied_error(
                ModuleDataErrorKind::ConsistencyUnavailable,
                "ConsistencyUnavailable",
                "requested commit is not visible",
            ));
        }
        Ok(DataResultV1::Entity {
            value,
            sequence: response.state.sequence_nr,
        })
    }

    async fn entity_create(
        &self,
        entity_type: &str,
        mut value: serde_json::Map<String, serde_json::Value>,
    ) -> Result<DataResultV1, ModuleDataError> {
        self.require(DataOperationKind::EntityCreate, entity_type, None)?;
        self.validate_entity_object(entity_type, &value, EntityWriteOperation::Create)?;
        let entity_id = extract_id(&value)?;
        self.authorize_value("create", entity_type, Some(&entity_id), Some(&value))?;
        if self.target_entity_exists(entity_type, &entity_id).await? {
            return Err(not_applied_error(
                ModuleDataErrorKind::AlreadyExists,
                "EntityAlreadyExists",
                "entity already exists",
            ));
        }
        value
            .entry("Id")
            .or_insert_with(|| entity_id.clone().into());
        let fields = serde_json::Value::Object(value.clone());
        let _guard = self
            .state
            .acquire_commons_write_guardrail_lock(&self.authority.tenant)
            .await;
        self.run_governed_write_prechecks(
            entity_type,
            &entity_id,
            "Create",
            "create",
            &fields,
            true,
        )
        .await?;
        let service = GovernedApplicationDataService::new(&self.state);
        let response = match &self.authority.target {
            ModuleDataTarget::TenantGlobal => {
                service
                    .create_typed(
                        &self.authority.tenant,
                        short_type(entity_type),
                        &entity_id,
                        value.clone().into(),
                    )
                    .await
            }
            ModuleDataTarget::Scoped(pin) => {
                service
                    .create_scoped_typed(
                        &self.authority.tenant,
                        short_type(entity_type),
                        &entity_id,
                        value.clone().into(),
                        pin.clone(),
                    )
                    .await
            }
        }
        .map_err(write_service_error)?;
        let canonical = self
            .canonical_entity_value(entity_type, &response.state)
            .map_err(|error| applied_internal_error(error.to_string()))?;
        Ok(write_result(
            entity_type,
            &entity_id,
            response.state.sequence_nr,
            serde_json::Value::Object(canonical),
        ))
    }

    async fn entity_patch(
        &self,
        entity_type: &str,
        entity_id: &str,
        expected: Option<u64>,
        value: serde_json::Map<String, serde_json::Value>,
    ) -> Result<DataResultV1, ModuleDataError> {
        self.require(DataOperationKind::EntityPatch, entity_type, None)?;
        self.validate_entity_object(entity_type, &value, EntityWriteOperation::Patch)?;
        let current = self.get_target_entity(entity_type, entity_id).await?;
        if expected.is_some_and(|sequence| sequence != current.state.sequence_nr) {
            return Err(ModuleDataError::new(
                ModuleDataErrorKind::Conflict,
                "SequenceConflict",
                "entity sequence does not match expected_sequence",
                temper_wasm_sdk::FailureRetryability::AfterRefresh,
                temper_wasm_sdk::FailureOutcome::NotApplied,
            )
            .expect("static sequence-conflict contract must be valid"));
        }
        let current_value = current
            .state
            .fields
            .as_object()
            .cloned()
            .unwrap_or_default();
        self.authorize_value("update", entity_type, Some(entity_id), Some(&current_value))?;
        let mut prospective = current.state.fields.clone();
        if let Some(object) = prospective.as_object_mut() {
            for (name, field_value) in &value {
                object.insert(name.clone(), field_value.clone());
            }
        }
        let _guard = self
            .state
            .acquire_commons_write_guardrail_lock(&self.authority.tenant)
            .await;
        self.run_governed_write_prechecks(
            entity_type,
            entity_id,
            "Patch",
            "patch",
            &prospective,
            true,
        )
        .await?;
        let service = GovernedApplicationDataService::new(&self.state);
        let response = match &self.authority.target {
            ModuleDataTarget::TenantGlobal => {
                service
                    .patch_typed(
                        &self.authority.tenant,
                        short_type(entity_type),
                        entity_id,
                        value.into(),
                        expected,
                        current.state.sequence_nr,
                    )
                    .await
            }
            ModuleDataTarget::Scoped(pin) => {
                service
                    .patch_scoped_typed(
                        &self.authority.tenant,
                        short_type(entity_type),
                        entity_id,
                        value.into(),
                        expected,
                        pin.clone(),
                        current.state.sequence_nr,
                    )
                    .await
            }
        }
        .map_err(write_service_error)?;
        if !response.success {
            return Err(not_applied_internal_error(
                response
                    .error
                    .unwrap_or_else(|| "FieldUpdateFailed".to_string()),
            ));
        }
        let canonical = self
            .canonical_entity_value(entity_type, &response.state)
            .map_err(|error| applied_internal_error(error.to_string()))?;
        Ok(write_result(
            entity_type,
            entity_id,
            response.state.sequence_nr,
            serde_json::Value::Object(canonical),
        ))
    }
}
