//! Host-owned routing for tenant-global and immutable scoped data targets.

use temper_wasm_sdk::data::ModuleDataError;

use crate::request_context::AgentContext;

use super::{
    ApplicationDataInvocation, GovernedApplicationDataService, ModuleDataTarget,
    not_applied_internal_error, read_service_error, short_type,
};

impl ApplicationDataInvocation {
    /// Load the authoritative actor selected by a durable journal identity.
    ///
    /// Unlike `get_target_entity`, this follows the winning stream's immutable
    /// pin instead of assuming it has the same target as the request.
    pub(super) async fn get_durable_target_entity(
        &self,
        entity_type: &str,
        journal_entity_id: &str,
    ) -> Result<crate::entity_actor::EntityResponse, ModuleDataError> {
        let service = GovernedApplicationDataService::new(&self.state);
        let result = if let Some((entity_id, pin)) =
            temper_runtime::persistence::schema_deployment::split_scoped_journal_entity_id(
                journal_entity_id,
            ) {
            service
                .get_scoped_typed(
                    &self.authority.tenant,
                    short_type(entity_type),
                    entity_id,
                    pin,
                )
                .await
        } else {
            service
                .get_typed(
                    &self.authority.tenant,
                    short_type(entity_type),
                    journal_entity_id,
                )
                .await
        };
        result.map_err(read_service_error)
    }

    pub(super) async fn get_target_entity(
        &self,
        entity_type: &str,
        entity_id: &str,
    ) -> Result<crate::entity_actor::EntityResponse, ModuleDataError> {
        let service = GovernedApplicationDataService::new(&self.state);
        match &self.authority.target {
            ModuleDataTarget::TenantGlobal => {
                service
                    .get_typed(&self.authority.tenant, short_type(entity_type), entity_id)
                    .await
            }
            ModuleDataTarget::Scoped(pin) => {
                service
                    .get_scoped_typed(
                        &self.authority.tenant,
                        short_type(entity_type),
                        entity_id,
                        pin.clone(),
                    )
                    .await
            }
        }
        .map_err(read_service_error)
    }

    pub(super) async fn target_entity_exists(
        &self,
        entity_type: &str,
        entity_id: &str,
    ) -> Result<bool, ModuleDataError> {
        match &self.authority.target {
            ModuleDataTarget::TenantGlobal => Ok(self.state.entity_exists(
                &self.authority.tenant,
                short_type(entity_type),
                entity_id,
            )),
            ModuleDataTarget::Scoped(pin) => GovernedApplicationDataService::new(&self.state)
                .exists_scoped(
                    &self.authority.tenant,
                    short_type(entity_type),
                    entity_id,
                    pin,
                )
                .await
                .map_err(not_applied_internal_error),
        }
    }

    pub(super) fn operation_agent_context(&self, expected_sequence: Option<u64>) -> AgentContext {
        AgentContext {
            security_ctx: Some(self.authority.security.clone()),
            agent_id: Some(self.authority.security.principal.id.clone()),
            expected_entity_sequence: expected_sequence,
            schema_pin: self.authority.target.schema_pin().cloned(),
            ..AgentContext::default()
        }
    }
}
