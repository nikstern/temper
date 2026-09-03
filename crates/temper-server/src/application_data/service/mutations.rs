//! Commit-phase-preserving entity mutation operations.

use temper_runtime::tenant::TenantId;

use crate::entity_actor::EntityResponse;
use crate::request_context::AgentContext;
use crate::state::{DispatchError, DispatchExtOptions, EntityMutationError};

use super::{ApplicationDataRejection, ApplicationDataWriteError, GovernedApplicationDataService};

impl GovernedApplicationDataService<'_> {
    /// Create through the common durable actor/data-only path.
    #[tracing::instrument(skip_all, fields(otel.name = "application_data.service.create", entity_type))]
    pub(crate) async fn create(
        &self,
        tenant: &TenantId,
        entity_type: &str,
        entity_id: &str,
        fields: serde_json::Value,
    ) -> Result<EntityResponse, String> {
        if let Some(response) = self
            .state
            .try_create_data_only_tenant_entity(tenant, entity_type, entity_id, fields.clone())
            .await?
        {
            return Ok(response);
        }
        self.state
            .get_or_create_tenant_entity(tenant, entity_type, entity_id, fields)
            .await
    }

    /// Create while preserving structural commit evidence for module data.
    pub(in crate::application_data) async fn create_typed(
        &self,
        tenant: &TenantId,
        entity_type: &str,
        entity_id: &str,
        fields: serde_json::Value,
    ) -> Result<EntityResponse, ApplicationDataWriteError> {
        let result = self
            .state
            .get_or_create_tenant_entity_typed(tenant, entity_type, entity_id, fields)
            .await;
        match result {
            Ok(response) => Ok(response),
            Err(EntityMutationError::Unknown(diagnostic)) => Err(self
                .reconcile_unknown_create_failure(tenant, entity_type, entity_id, None, diagnostic)
                .await),
            Err(error) => {
                Err(self.map_entity_mutation_error(error, ApplicationDataRejection::Internal))
            }
        }
    }

    /// Create through the immutable task-scoped actor path.
    pub(crate) async fn create_scoped(
        &self,
        tenant: &TenantId,
        entity_type: &str,
        entity_id: &str,
        fields: serde_json::Value,
        schema_pin: temper_runtime::persistence::schema_deployment::SchemaExecutionPin,
    ) -> Result<EntityResponse, String> {
        self.state
            .get_or_create_scoped_entity(tenant, entity_type, entity_id, fields, schema_pin)
            .await
    }

    /// Create through a scoped actor while preserving structural commit evidence.
    pub(in crate::application_data) async fn create_scoped_typed(
        &self,
        tenant: &TenantId,
        entity_type: &str,
        entity_id: &str,
        fields: serde_json::Value,
        schema_pin: temper_runtime::persistence::schema_deployment::SchemaExecutionPin,
    ) -> Result<EntityResponse, ApplicationDataWriteError> {
        let result = self
            .state
            .get_or_create_scoped_entity_typed(
                tenant,
                entity_type,
                entity_id,
                fields,
                schema_pin.clone(),
            )
            .await;
        match result {
            Ok(response) => Ok(response),
            Err(EntityMutationError::Unknown(diagnostic)) => Err(self
                .reconcile_unknown_create_failure(
                    tenant,
                    entity_type,
                    entity_id,
                    Some(schema_pin),
                    diagnostic,
                )
                .await),
            Err(error) => {
                Err(self.map_entity_mutation_error(error, ApplicationDataRejection::Internal))
            }
        }
    }

    /// Patch through the common exact-sequence actor path.
    #[tracing::instrument(skip_all, fields(otel.name = "application_data.service.patch", entity_type))]
    #[expect(dead_code, reason = "retained for the legacy data adapter")]
    pub(crate) async fn patch(
        &self,
        tenant: &TenantId,
        entity_type: &str,
        entity_id: &str,
        fields: serde_json::Value,
        expected_sequence: Option<u64>,
    ) -> Result<EntityResponse, String> {
        self.write_fields(
            tenant,
            entity_type,
            entity_id,
            fields,
            false,
            expected_sequence,
        )
        .await
    }

    /// Patch while preserving structural commit evidence for module data.
    pub(in crate::application_data) async fn patch_typed(
        &self,
        tenant: &TenantId,
        entity_type: &str,
        entity_id: &str,
        fields: serde_json::Value,
        expected_sequence: Option<u64>,
        _baseline_sequence: u64,
    ) -> Result<EntityResponse, ApplicationDataWriteError> {
        let result = self
            .state
            .update_tenant_entity_fields_typed(
                tenant,
                entity_type,
                entity_id,
                fields,
                false,
                expected_sequence,
            )
            .await;
        match result {
            Ok(response) => Ok(response),
            Err(error) => {
                Err(self.map_entity_mutation_error(error, ApplicationDataRejection::Internal))
            }
        }
    }

    /// Patch through one immutable task-scoped actor.
    #[expect(dead_code, reason = "retained for the legacy scoped-data adapter")]
    pub(crate) async fn patch_scoped(
        &self,
        tenant: &TenantId,
        entity_type: &str,
        entity_id: &str,
        fields: serde_json::Value,
        expected_sequence: Option<u64>,
        schema_pin: temper_runtime::persistence::schema_deployment::SchemaExecutionPin,
    ) -> Result<EntityResponse, String> {
        self.state
            .update_scoped_entity_fields_if_sequence(
                tenant,
                entity_type,
                entity_id,
                fields,
                false,
                expected_sequence,
                schema_pin,
            )
            .await
    }

    /// Patch through a scoped actor while preserving structural commit evidence.
    #[expect(
        clippy::too_many_arguments,
        reason = "the typed scoped patch boundary carries the observed sequence and schema pin"
    )]
    pub(in crate::application_data) async fn patch_scoped_typed(
        &self,
        tenant: &TenantId,
        entity_type: &str,
        entity_id: &str,
        fields: serde_json::Value,
        expected_sequence: Option<u64>,
        schema_pin: temper_runtime::persistence::schema_deployment::SchemaExecutionPin,
        _baseline_sequence: u64,
    ) -> Result<EntityResponse, ApplicationDataWriteError> {
        let result = self
            .state
            .update_scoped_entity_fields_typed(
                tenant,
                entity_type,
                entity_id,
                fields,
                false,
                expected_sequence,
                schema_pin.clone(),
            )
            .await;
        match result {
            Ok(response) => Ok(response),
            Err(error) => {
                Err(self.map_entity_mutation_error(error, ApplicationDataRejection::Internal))
            }
        }
    }

    /// Replace through the common actor path used by the OData adapter.
    #[expect(dead_code, reason = "reserved for the typed module-data adapter")]
    pub(crate) async fn replace(
        &self,
        tenant: &TenantId,
        entity_type: &str,
        entity_id: &str,
        fields: serde_json::Value,
    ) -> Result<EntityResponse, String> {
        self.write_fields(tenant, entity_type, entity_id, fields, true, None)
            .await
    }

    /// Replace through one immutable task-scoped actor.
    #[expect(dead_code, reason = "reserved for the typed scoped-data adapter")]
    pub(crate) async fn replace_scoped(
        &self,
        tenant: &TenantId,
        entity_type: &str,
        entity_id: &str,
        fields: serde_json::Value,
        schema_pin: temper_runtime::persistence::schema_deployment::SchemaExecutionPin,
    ) -> Result<EntityResponse, String> {
        self.state
            .update_scoped_entity_fields_if_sequence(
                tenant,
                entity_type,
                entity_id,
                fields,
                true,
                None,
                schema_pin,
            )
            .await
    }

    async fn write_fields(
        &self,
        tenant: &TenantId,
        entity_type: &str,
        entity_id: &str,
        fields: serde_json::Value,
        replace: bool,
        expected_sequence: Option<u64>,
    ) -> Result<EntityResponse, String> {
        self.state
            .update_tenant_entity_fields_if_sequence(
                tenant,
                entity_type,
                entity_id,
                fields,
                replace,
                expected_sequence,
            )
            .await
    }

    fn map_entity_mutation_error(
        &self,
        error: EntityMutationError,
        rejection: ApplicationDataRejection,
    ) -> ApplicationDataWriteError {
        match error {
            EntityMutationError::NotApplied(diagnostic) => ApplicationDataWriteError::NotApplied {
                reason: rejection,
                diagnostic,
            },
            EntityMutationError::Applied(diagnostic) => {
                ApplicationDataWriteError::Applied(diagnostic)
            }
            EntityMutationError::Unknown(diagnostic) => {
                ApplicationDataWriteError::Unknown(diagnostic)
            }
        }
    }

    /// Reconcile only causally proven absence; retain unknown on every read failure.
    pub(in crate::application_data) async fn reconcile_unknown_create_failure(
        &self,
        tenant: &TenantId,
        entity_type: &str,
        entity_id: &str,
        schema_pin: Option<temper_runtime::persistence::schema_deployment::SchemaExecutionPin>,
        diagnostic: String,
    ) -> ApplicationDataWriteError {
        let observed = match schema_pin {
            Some(pin) => {
                self.state
                    .scoped_entity_exists(tenant, entity_type, entity_id, &pin)
                    .await
            }
            None => {
                self.state
                    .durable_global_entity_exists(tenant, entity_type, entity_id)
                    .await
            }
        };
        match observed {
            Ok(false) => ApplicationDataWriteError::NotApplied {
                reason: ApplicationDataRejection::Internal,
                diagnostic,
            },
            _ => ApplicationDataWriteError::Unknown(diagnostic),
        }
    }

    /// Invoke an action through the common actor dispatch path.
    #[tracing::instrument(skip_all, fields(otel.name = "application_data.service.action", entity_type, action))]
    #[allow(dead_code, reason = "retained for the legacy action adapter")]
    pub(crate) async fn action(
        &self,
        tenant: &TenantId,
        entity_type: &str,
        entity_id: &str,
        action: &str,
        params: serde_json::Value,
        agent: &AgentContext,
    ) -> Result<EntityResponse, String> {
        if agent.idempotency_key.as_deref().is_some_and(|key| {
            key.starts_with(crate::entity_actor::SCHEMA_BOOTSTRAP_ACTION_IDEMPOTENCY_PREFIX)
        }) {
            return Err("reserved schema-bootstrap idempotency identity".into());
        }
        self.action_with_options(tenant, entity_type, entity_id, action, params, agent, true)
            .await
            .map_err(|error| error.to_string())
    }

    /// Invoke an action while preserving typed rejection and commit evidence.
    #[tracing::instrument(skip_all, fields(otel.name = "application_data.service.action_typed", entity_type, action))]
    #[expect(
        clippy::too_many_arguments,
        reason = "the typed action boundary carries the observed pre-dispatch sequence"
    )]
    pub(in crate::application_data) async fn action_typed(
        &self,
        tenant: &TenantId,
        entity_type: &str,
        entity_id: &str,
        action: &str,
        params: serde_json::Value,
        agent: &AgentContext,
        _baseline_sequence: u64,
    ) -> Result<EntityResponse, ApplicationDataWriteError> {
        if agent.idempotency_key.as_deref().is_some_and(|key| {
            key.starts_with(crate::entity_actor::SCHEMA_BOOTSTRAP_ACTION_IDEMPOTENCY_PREFIX)
        }) {
            return Err(ApplicationDataWriteError::NotApplied {
                reason: ApplicationDataRejection::Internal,
                diagnostic: "reserved schema-bootstrap idempotency identity".into(),
            });
        }
        match self
            .action_with_options(tenant, entity_type, entity_id, action, params, agent, true)
            .await
        {
            Ok(response) => Ok(response),
            Err(error) => {
                let rejection = rejection_for_dispatch_error(&error);
                if let Some(reason) = rejection {
                    return Err(ApplicationDataWriteError::NotApplied {
                        reason,
                        diagnostic: error.to_string(),
                    });
                }
                Err(ApplicationDataWriteError::Unknown(error.to_string()))
            }
        }
    }

    /// Invoke the initial action through the host-only schema-bootstrap path.
    pub(crate) async fn action_for_schema_bootstrap(
        &self,
        tenant: &TenantId,
        entity_type: &str,
        entity_id: &str,
        action: &str,
        params: serde_json::Value,
        agent: &AgentContext,
    ) -> Result<EntityResponse, String> {
        debug_assert!(agent.idempotency_key.as_deref().is_some_and(|key| {
            key.starts_with(crate::entity_actor::SCHEMA_BOOTSTRAP_ACTION_IDEMPOTENCY_PREFIX)
        }));
        self.action_with_options(tenant, entity_type, entity_id, action, params, agent, true)
            .await
            .map_err(|error| error.to_string())
    }

    /// Invoke an action with the adapter's integration-wait preference.
    #[expect(
        clippy::too_many_arguments,
        reason = "the action boundary preserves the actor dispatch contract explicitly"
    )]
    pub(crate) async fn action_with_options(
        &self,
        tenant: &TenantId,
        entity_type: &str,
        entity_id: &str,
        action: &str,
        params: serde_json::Value,
        agent: &AgentContext,
        await_integration: bool,
    ) -> Result<EntityResponse, DispatchError> {
        self.state
            .dispatch_tenant_action_ext_typed(
                tenant,
                entity_type,
                entity_id,
                action,
                params,
                DispatchExtOptions {
                    agent_ctx: agent,
                    await_integration,
                    await_reactions: true,
                },
            )
            .await
    }
}

#[allow(deprecated)]
fn rejection_for_dispatch_error(error: &DispatchError) -> Option<ApplicationDataRejection> {
    match error {
        DispatchError::AuthzDenied(_) => Some(ApplicationDataRejection::AuthorizationDenied),
        DispatchError::QuotaExceeded(_) => Some(ApplicationDataRejection::BudgetExceeded),
        DispatchError::Conflict(_) | DispatchError::CollectionWorkflowConflict(_) => {
            Some(ApplicationDataRejection::Conflict)
        }
        DispatchError::Ungoverned(_) => Some(ApplicationDataRejection::SchemaMismatch),
        DispatchError::Deferred { .. } => Some(ApplicationDataRejection::Internal),
        DispatchError::Transient { .. }
        | DispatchError::Permanent { .. }
        | DispatchError::ActorFailed(_)
        | DispatchError::WasmFailed(_)
        | DispatchError::Internal(_) => None,
    }
}
