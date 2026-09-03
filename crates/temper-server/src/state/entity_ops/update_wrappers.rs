use temper_runtime::persistence::schema_deployment::SchemaExecutionPin;
use temper_runtime::tenant::TenantId;

use crate::entity_actor::EntityResponse;

use super::{EntityMutationError, ServerState};

impl ServerState {
    /// Update fields on an existing entity.
    #[tracing::instrument(skip_all, fields(otel.name = "entity.update_tenant_entity_fields", tenant = %tenant, entity_type, entity_id))]
    pub async fn update_tenant_entity_fields(
        &self,
        tenant: &TenantId,
        entity_type: &str,
        entity_id: &str,
        fields: serde_json::Value,
        replace: bool,
    ) -> Result<EntityResponse, String> {
        self.update_tenant_entity_fields_if_sequence(
            tenant,
            entity_type,
            entity_id,
            fields,
            replace,
            None,
        )
        .await
    }

    /// Update fields only when the actor is still at `expected_sequence`.
    #[tracing::instrument(skip_all, fields(otel.name = "entity.update_tenant_entity_fields_if_sequence", tenant = %tenant, entity_type, entity_id))]
    pub async fn update_tenant_entity_fields_if_sequence(
        &self,
        tenant: &TenantId,
        entity_type: &str,
        entity_id: &str,
        fields: serde_json::Value,
        replace: bool,
        expected_sequence: Option<u64>,
    ) -> Result<EntityResponse, String> {
        self.update_entity_fields_with_schema_pin(
            tenant,
            entity_type,
            entity_id,
            fields,
            replace,
            expected_sequence,
            None,
            None,
        )
        .await
        .map_err(|error| error.to_string())
    }

    /// Update fields through one exact immutable scoped actor.
    #[expect(
        clippy::too_many_arguments,
        reason = "scoped update carries its immutable schema and sequence evidence"
    )]
    pub async fn update_scoped_entity_fields_if_sequence(
        &self,
        tenant: &TenantId,
        entity_type: &str,
        entity_id: &str,
        fields: serde_json::Value,
        replace: bool,
        expected_sequence: Option<u64>,
        schema_pin: SchemaExecutionPin,
    ) -> Result<EntityResponse, String> {
        self.update_entity_fields_with_schema_pin(
            tenant,
            entity_type,
            entity_id,
            fields,
            replace,
            expected_sequence,
            Some(schema_pin),
            None,
        )
        .await
        .map_err(|error| error.to_string())
    }

    /// Update fields only if the global actor still matches the state Cedar authorized.
    pub(crate) async fn update_tenant_entity_fields_if_current(
        &self,
        tenant: &TenantId,
        entity_type: &str,
        entity_id: &str,
        fields: serde_json::Value,
        replace: bool,
        expected_precondition: String,
    ) -> Result<EntityResponse, String> {
        self.update_entity_fields_with_schema_pin(
            tenant,
            entity_type,
            entity_id,
            fields,
            replace,
            None,
            None,
            Some(expected_precondition),
        )
        .await
        .map_err(|error| error.to_string())
    }

    /// Update fields only if an immutable scoped actor still matches the state Cedar authorized.
    #[expect(
        clippy::too_many_arguments,
        reason = "scoped update carries schema and authorization evidence"
    )]
    pub(crate) async fn update_scoped_entity_fields_if_current(
        &self,
        tenant: &TenantId,
        entity_type: &str,
        entity_id: &str,
        fields: serde_json::Value,
        replace: bool,
        schema_pin: SchemaExecutionPin,
        expected_precondition: String,
    ) -> Result<EntityResponse, String> {
        self.update_entity_fields_with_schema_pin(
            tenant,
            entity_type,
            entity_id,
            fields,
            replace,
            None,
            Some(schema_pin),
            Some(expected_precondition),
        )
        .await
        .map_err(|error| error.to_string())
    }

    /// Update global entity fields while preserving causal commit evidence.
    pub(crate) async fn update_tenant_entity_fields_typed(
        &self,
        tenant: &TenantId,
        entity_type: &str,
        entity_id: &str,
        fields: serde_json::Value,
        replace: bool,
        expected_sequence: Option<u64>,
    ) -> Result<EntityResponse, EntityMutationError> {
        self.update_entity_fields_with_schema_pin(
            tenant,
            entity_type,
            entity_id,
            fields,
            replace,
            expected_sequence,
            None,
            None,
        )
        .await
    }

    /// Update scoped entity fields while preserving causal commit evidence.
    #[expect(
        clippy::too_many_arguments,
        reason = "typed scoped update carries immutable schema and sequence evidence"
    )]
    pub(crate) async fn update_scoped_entity_fields_typed(
        &self,
        tenant: &TenantId,
        entity_type: &str,
        entity_id: &str,
        fields: serde_json::Value,
        replace: bool,
        expected_sequence: Option<u64>,
        schema_pin: SchemaExecutionPin,
    ) -> Result<EntityResponse, EntityMutationError> {
        self.update_entity_fields_with_schema_pin(
            tenant,
            entity_type,
            entity_id,
            fields,
            replace,
            expected_sequence,
            Some(schema_pin),
            None,
        )
        .await
    }
}
