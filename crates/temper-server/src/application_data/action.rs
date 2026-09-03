//! Governed entity-action invocation for the module-data boundary.

use temper_wasm_sdk::data::{
    DataOperationKind, DataResultV1, ModuleDataError, ModuleDataErrorKind,
};

use super::{
    ApplicationDataInvocation, GovernedApplicationDataService, applied_internal_error, commit,
    not_applied_error, short_type, unknown_internal_error, write_service_error,
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
        self.state
            .enforce_commons_verified_owner_for_action(
                &self.authority.tenant,
                short_type(entity_type),
                &current.state.fields,
                &serde_json::Value::Object(params.clone()),
            )
            .await
            .map_err(|_| {
                not_applied_error(
                    ModuleDataErrorKind::AuthorizationDenied,
                    "AccountVerificationRequired",
                    "commons account verification rejected the action",
                )
            })?;
        let agent = self.operation_agent_context(expected);
        let response = GovernedApplicationDataService::new(&self.state)
            .action_typed(
                &self.authority.tenant,
                short_type(entity_type),
                entity_id,
                action,
                params.into(),
                &agent,
                current.state.sequence_nr,
            )
            .await
            .map_err(write_service_error)?;
        if !response.success {
            let diagnostic = response
                .error
                .unwrap_or_else(|| "action failed without a diagnostic".to_string());
            return match response.failure_outcome {
                Some(temper_wasm_sdk::FailureOutcome::Applied) => {
                    Err(applied_internal_error(diagnostic))
                }
                Some(temper_wasm_sdk::FailureOutcome::Unknown) => {
                    Err(unknown_internal_error(diagnostic))
                }
                Some(temper_wasm_sdk::FailureOutcome::NotApplied) => Err(not_applied_error(
                    ModuleDataErrorKind::GuardRejected,
                    "ActionRejected",
                    &diagnostic,
                )),
                None => Err(unknown_internal_error(diagnostic)),
            };
        }
        let result = match self.action_result_type(entity_type, action) {
            None => None,
            Some(result_entity_type) if result_entity_type == entity_type => {
                Some(serde_json::Value::Object(
                    self.canonical_entity_value(result_entity_type, &response.state)
                        .map_err(|error| applied_internal_error(error.to_string()))?,
                ))
            }
            Some(_) => Some(response.state.fields),
        };
        Ok(DataResultV1::Action {
            commit: commit(entity_type, entity_id, response.state.sequence_nr),
            result,
            result_omitted: false,
        })
    }
}
