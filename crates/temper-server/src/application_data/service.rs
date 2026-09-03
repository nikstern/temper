//! Shared transport-neutral entity mutation service.

mod mutations;

use std::collections::BTreeMap;

use temper_authz::{AuthzDenial, SecurityContext};
use temper_runtime::tenant::TenantId;

use crate::entity_actor::EntityResponse;
use crate::state::ServerState;
use crate::storage::{QueryFieldIndexOrder, QueryFieldIndexPage};

/// Structured read failure at the application-data service boundary.
#[derive(Debug, thiserror::Error)]
pub(super) enum ApplicationDataReadError {
    /// The authoritative entity does not exist.
    #[error("entity not found")]
    NotFound,
    /// The entity may exist, but its state could not be observed.
    #[error("application-data read failed: {0}")]
    Internal(String),
}

/// Typed reason for a write rejected before a commit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ApplicationDataRejection {
    /// An optimistic concurrency precondition failed.
    Conflict,
    /// Authorization rejected the operation.
    AuthorizationDenied,
    /// A bounded resource budget rejected the operation.
    BudgetExceeded,
    /// The requested entity or action is not governed by the active schema.
    SchemaMismatch,
    /// Another internal pre-commit check rejected the operation.
    Internal,
}

/// Commit-phase-preserving write failure for application-data callers.
#[derive(Debug, thiserror::Error)]
pub(super) enum ApplicationDataWriteError {
    /// Structural evidence proves that no commit occurred.
    #[error("application-data write was not applied: {diagnostic}")]
    NotApplied {
        /// Stable rejection class independent of the diagnostic.
        reason: ApplicationDataRejection,
        /// Diagnostic retained only for logs.
        diagnostic: String,
    },
    /// Structural evidence proves that the commit occurred.
    #[error("application-data write committed before failure: {0}")]
    Applied(String),
    /// Available evidence cannot prove whether the commit occurred.
    #[error("application-data write acknowledgement is unknown: {0}")]
    Unknown(String),
}

/// Governed entity data substrate shared by OData adapters and module invocations.
pub(crate) struct GovernedApplicationDataService<'a> {
    state: &'a ServerState,
}

impl<'a> GovernedApplicationDataService<'a> {
    /// Bind the service to server state for one adapter call.
    pub(crate) fn new(state: &'a ServerState) -> Self {
        Self { state }
    }

    /// Apply the adapter-neutral Cedar decision for a resolved resource.
    pub(crate) fn authorize(
        &self,
        tenant: &TenantId,
        security: &SecurityContext,
        action: &str,
        entity_type: &str,
        attributes: &BTreeMap<String, serde_json::Value>,
    ) -> Result<(), AuthzDenial> {
        self.state.authorize_with_context(
            security,
            action,
            entity_type,
            attributes,
            tenant.as_str(),
        )
    }

    /// Read one canonical actor state.
    #[tracing::instrument(skip_all, fields(otel.name = "application_data.service.get", entity_type))]
    pub(crate) async fn get(
        &self,
        tenant: &TenantId,
        entity_type: &str,
        entity_id: &str,
    ) -> Result<EntityResponse, String> {
        if !self.state.entity_exists(tenant, entity_type, entity_id)
            && !self
                .state
                .ensure_entity_loaded(tenant, entity_type, entity_id)
                .await
        {
            return Err("EntityNotFound".into());
        }
        self.state
            .get_tenant_entity_state(tenant, entity_type, entity_id)
            .await
    }

    /// Read one canonical actor state through an exact immutable scoped pin.
    #[allow(dead_code, reason = "retained for the legacy scoped-data adapter")]
    pub(crate) async fn get_scoped(
        &self,
        tenant: &TenantId,
        entity_type: &str,
        entity_id: &str,
        schema_pin: temper_runtime::persistence::schema_deployment::SchemaExecutionPin,
    ) -> Result<EntityResponse, String> {
        self.state
            .get_scoped_entity_state(tenant, entity_type, entity_id, schema_pin)
            .await
    }

    /// Read one entity with a typed missing-versus-internal distinction.
    pub(super) async fn get_typed(
        &self,
        tenant: &TenantId,
        entity_type: &str,
        entity_id: &str,
    ) -> Result<EntityResponse, ApplicationDataReadError> {
        if !self.state.entity_exists(tenant, entity_type, entity_id)
            && !self
                .state
                .ensure_entity_loaded(tenant, entity_type, entity_id)
                .await
        {
            return Err(ApplicationDataReadError::NotFound);
        }
        self.state
            .get_tenant_entity_state(tenant, entity_type, entity_id)
            .await
            .map_err(ApplicationDataReadError::Internal)
    }

    /// Read one scoped entity with a typed missing-versus-internal distinction.
    pub(super) async fn get_scoped_typed(
        &self,
        tenant: &TenantId,
        entity_type: &str,
        entity_id: &str,
        schema_pin: temper_runtime::persistence::schema_deployment::SchemaExecutionPin,
    ) -> Result<EntityResponse, ApplicationDataReadError> {
        if !self
            .state
            .scoped_entity_exists(tenant, entity_type, entity_id, &schema_pin)
            .await
            .map_err(ApplicationDataReadError::Internal)?
        {
            return Err(ApplicationDataReadError::NotFound);
        }
        self.state
            .get_scoped_entity_state(tenant, entity_type, entity_id, schema_pin)
            .await
            .map_err(ApplicationDataReadError::Internal)
    }

    /// Whether an exact scoped entity already exists under the supplied pin.
    pub(crate) async fn exists_scoped(
        &self,
        tenant: &TenantId,
        entity_type: &str,
        entity_id: &str,
        schema_pin: &temper_runtime::persistence::schema_deployment::SchemaExecutionPin,
    ) -> Result<bool, String> {
        self.state
            .scoped_entity_exists(tenant, entity_type, entity_id, schema_pin)
            .await
    }

    /// Read a stable ordered page from the query plane when available.
    pub(crate) async fn query_candidates(
        &self,
        tenant: &TenantId,
        entity_type: &str,
        order: &[QueryFieldIndexOrder],
        offset: usize,
        limit: usize,
    ) -> Result<Option<Vec<String>>, String> {
        self.query_index_page(
            tenant,
            entity_type,
            "TRUE",
            Vec::new(),
            order,
            offset,
            limit,
            false,
        )
        .await
        .map(|page| page.map(|page| page.entity_ids))
    }

    /// Execute one bounded query-plane page for either external adapter.
    #[expect(
        clippy::too_many_arguments,
        reason = "the query-plane boundary preserves its storage contract explicitly"
    )]
    pub(crate) async fn query_index_page(
        &self,
        tenant: &TenantId,
        entity_type: &str,
        where_clause: &str,
        params: Vec<String>,
        order: &[QueryFieldIndexOrder],
        offset: usize,
        limit: usize,
        include_count: bool,
    ) -> Result<Option<QueryFieldIndexPage>, String> {
        let Some(store) = self.state.query_plane_store() else {
            return Ok(None);
        };
        store
            .query_field_index_page(
                tenant.as_str(),
                entity_type,
                where_clause,
                params,
                order,
                offset,
                limit,
                include_count,
            )
            .await
            .map_err(|error| error.to_string())
    }

    /// Enumerate the authoritative actor index for a bounded query fallback.
    pub(crate) async fn fallback_candidates(
        &self,
        tenant: &TenantId,
        entity_type: &str,
    ) -> Vec<String> {
        let mut ids = self.state.list_entity_ids_lazy(tenant, entity_type).await;
        ids.sort();
        ids
    }

    /// Enumerate only when authoritative completeness is already bounded.
    pub(crate) fn bounded_fallback_candidates(
        &self,
        tenant: &TenantId,
        entity_type: &str,
        budget: usize,
    ) -> Result<Vec<String>, String> {
        self.state
            .list_entity_ids_bounded(tenant, entity_type, budget)
            .ok_or_else(|| "BoundedAuthoritativeFallbackUnavailable".into())
    }

    /// Enumerate an exact scoped journal set within a caller-owned work budget.
    pub(crate) async fn bounded_scoped_candidates(
        &self,
        tenant: &TenantId,
        entity_type: &str,
        schema_pin: &temper_runtime::persistence::schema_deployment::SchemaExecutionPin,
        budget: usize,
    ) -> Result<Vec<String>, String> {
        self.state
            .list_scoped_entity_ids_bounded(tenant, entity_type, schema_pin, budget)
            .await
    }

    /// Read the currently indexed IDs for bounded projection-coverage repair.
    pub(crate) fn indexed_candidates(&self, tenant: &TenantId, entity_type: &str) -> Vec<String> {
        self.state.list_entity_ids(tenant, entity_type)
    }
}
