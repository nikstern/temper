//! Governed entity-action invocation for the module-data boundary.

use temper_wasm_sdk::data::{
    DataOperationKind, DataResultV1, ModuleDataError, ModuleDataErrorKind,
};

use super::{
    ApplicationDataInvocation, GovernedApplicationDataService, commit, data_error, internal_error,
    short_type,
};

impl ApplicationDataInvocation {
    pub(super) async fn action_invoke(
        &self,
        kind: DataOperationKind,
        entity_type: &str,
        entity_id: &str,
        action: &str,
        expected: Option<u64>,
        params: serde_json::Map<String, serde_json::Value>,
    ) -> Result<DataResultV1, ModuleDataError> {
        self.require(kind, entity_type, Some(action))?;
        self.validate_action_params(entity_type, action, &params)?;
        self.authorize(action, entity_type, Some(entity_id))?;
        let current = self.get_target_entity(entity_type, entity_id).await?;
        self.state
            .enforce_commons_verified_owner_for_action(
                &self.authority.tenant,
                short_type(entity_type),
                &current.state.fields,
                &serde_json::Value::Object(params.clone()),
            )
            .await
            .map_err(|_| {
                data_error(
                    ModuleDataErrorKind::AuthorizationDenied,
                    "AccountVerificationRequired",
                    "commons account verification rejected the action",
                )
            })?;
        let agent = self.operation_agent_context(expected);
        let response = GovernedApplicationDataService::new(&self.state)
            .action(
                &self.authority.tenant,
                short_type(entity_type),
                entity_id,
                action,
                params.into(),
                &agent,
            )
            .await
            .map_err(internal_error)?;
        if !response.success {
            if response.error.as_deref() == Some("SequenceConflict") {
                return Err(data_error(
                    ModuleDataErrorKind::Conflict,
                    "SequenceConflict",
                    "entity sequence does not match expected_sequence",
                ));
            }
            return Err(data_error(
                ModuleDataErrorKind::GuardRejected,
                "ActionRejected",
                response.error.as_deref().unwrap_or("action rejected"),
            ));
        }
        let result = match self.action_result_type(entity_type, action) {
            None => None,
            Some(result_entity_type) if result_entity_type == entity_type => Some(
                serde_json::Value::Object(
                    self.canonical_entity_value(result_entity_type, &response.state)?,
                ),
            ),
            Some(_) => Some(response.state.fields),
        };
        Ok(DataResultV1::Action {
            commit: commit(entity_type, entity_id, response.state.sequence_nr),
            result,
            result_omitted: false,
        })
    }
}
