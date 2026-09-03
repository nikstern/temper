//! Invocation capability and Cedar authorization checks.

use std::collections::BTreeMap;

use temper_authz::AuthzDenial;
use temper_wasm_sdk::data::{
    DataOperationKind, FileOperationKind, ModuleDataError, ModuleDataErrorKind,
};

use super::{
    ApplicationDataInvocation, GovernedApplicationDataService, ModuleDataTarget, not_applied_error,
    short_type,
};

impl ApplicationDataInvocation {
    pub(super) fn require(
        &self,
        kind: DataOperationKind,
        entity_type: &str,
        action: Option<&str>,
    ) -> Result<(), ModuleDataError> {
        if let ModuleDataTarget::Scoped(pin) = &self.authority.target {
            let exact_schema_available = self
                .state
                .registry
                .read()
                .map(|registry| {
                    registry
                        .get_scoped_spec_at_digest(
                            &self.authority.tenant,
                            &pin.scope,
                            &pin.bundle_digest,
                            short_type(entity_type),
                        )
                        .is_some()
                })
                .unwrap_or(false);
            if !exact_schema_available {
                return Err(not_applied_error(
                    ModuleDataErrorKind::SchemaMismatch,
                    "ScopedSchemaUnavailable",
                    "exact scoped schema is unavailable",
                ));
            }
        }
        if self
            .authority
            .binding
            .grant
            .permits(kind, entity_type, action)
        {
            return Ok(());
        }
        Err(not_applied_error(
            ModuleDataErrorKind::AuthorizationDenied,
            "CapabilityDenied",
            "module data grant does not permit this operation",
        ))
    }

    pub(super) fn require_file(
        &self,
        kind: DataOperationKind,
        file_operation: FileOperationKind,
    ) -> Result<String, ModuleDataError> {
        if !self.authority.binding.grant.operations.contains(&kind) {
            return Err(not_applied_error(
                ModuleDataErrorKind::AuthorizationDenied,
                "CapabilityDenied",
                "module data grant does not permit this File operation",
            ));
        }
        self.authority
            .binding
            .grant
            .entities
            .iter()
            .find(|entity| {
                short_type(&entity.entity_type) == "File"
                    && entity.file_operations.contains(&file_operation)
            })
            .map(|entity| entity.entity_type.clone())
            .ok_or_else(|| {
                not_applied_error(
                    ModuleDataErrorKind::AuthorizationDenied,
                    "FileCapabilityDenied",
                    "module data grant does not permit this File capability",
                )
            })
    }

    pub(super) fn authorize(
        &self,
        action: &str,
        entity_type: &str,
        entity_id: Option<&str>,
    ) -> Result<(), ModuleDataError> {
        self.authorize_value(action, entity_type, entity_id, None)
    }

    pub(super) fn authorize_value(
        &self,
        action: &str,
        entity_type: &str,
        entity_id: Option<&str>,
        value: Option<&serde_json::Map<String, serde_json::Value>>,
    ) -> Result<(), ModuleDataError> {
        let mut attrs = BTreeMap::new();
        if let Some(entity_id) = entity_id {
            attrs.insert("id".into(), serde_json::Value::String(entity_id.into()));
        }
        attrs.insert(
            "module_name".into(),
            self.authority.module_name.clone().into(),
        );
        attrs.insert(
            "module_artifact".into(),
            self.authority.artifact_digest.clone().into(),
        );
        attrs.insert(
            "module_trigger".into(),
            self.authority.trigger.clone().into(),
        );
        attrs.insert(
            "module_trigger_entity_type".into(),
            self.authority.triggering_entity_type.clone().into(),
        );
        attrs.insert(
            "module_grant_digest".into(),
            self.authority.grant_digest.clone().into(),
        );
        if let ModuleDataTarget::Scoped(pin) = &self.authority.target {
            attrs.insert("schema_scope_kind".into(), "task".into());
            attrs.insert("schema_scope_id".into(), pin.scope.id.clone().into());
            attrs.insert(
                "schema_bundle_digest".into(),
                pin.bundle_digest.clone().into(),
            );
        }
        if let Some(value) = value {
            attrs.extend(value.clone());
            let status = value
                .get("Status")
                .or_else(|| value.get("status"))
                .cloned()
                .unwrap_or_else(|| serde_json::Value::String(String::new()));
            attrs.insert("status".into(), status);
        }
        let has_spec = self
            .state
            .registry
            .read()
            .map(|registry| match &self.authority.target {
                ModuleDataTarget::TenantGlobal => registry
                    .get_spec(&self.authority.tenant, short_type(entity_type))
                    .is_some(),
                ModuleDataTarget::Scoped(pin) => registry
                    .get_scoped_spec_at_digest(
                        &self.authority.tenant,
                        &pin.scope,
                        &pin.bundle_digest,
                        short_type(entity_type),
                    )
                    .is_some(),
            })
            .unwrap_or(false);
        attrs.insert("has_spec".into(), has_spec.into());
        GovernedApplicationDataService::new(&self.state)
            .authorize(
                &self.authority.tenant,
                &self.authority.security,
                action,
                short_type(entity_type),
                &attrs,
            )
            .map_err(module_data_authorization_error)
    }
}

fn module_data_authorization_error(denial: AuthzDenial) -> ModuleDataError {
    let mut error = ModuleDataError::new(
        ModuleDataErrorKind::AuthorizationDenied,
        "AuthorizationDenied",
        "caller is not authorized for this operation",
        temper_wasm_sdk::FailureRetryability::AfterAuthorization,
        temper_wasm_sdk::FailureOutcome::NotApplied,
    )
    .expect("static authorization failure contract must be valid");
    let (denial_class, policy_ids, decision_id) = match denial {
        AuthzDenial::PolicyDenied { mut policy_ids } => {
            policy_ids.sort();
            policy_ids.dedup();
            policy_ids.truncate(16);
            let decision_id = policy_ids
                .first()
                .cloned()
                .or_else(|| Some("cedar:policy-denied".to_string()));
            ("policy_denied", Some(policy_ids), decision_id)
        }
        AuthzDenial::NoMatchingPermit => (
            "no_matching_permit",
            None,
            Some("cedar:no-matching-permit".to_string()),
        ),
        AuthzDenial::InvalidPrincipal(_) => ("invalid_principal", None, None),
        AuthzDenial::InvalidAction(_) => ("invalid_action", None, None),
        AuthzDenial::InvalidResource(_) => ("invalid_resource", None, None),
        AuthzDenial::InvalidContext(_) => ("invalid_context", None, None),
        AuthzDenial::EngineError(_) => ("engine_error", None, None),
    };
    error.insert_detail_or_omit(
        temper_wasm_sdk::DetailKey::new("denial_class").expect("static detail key is valid"),
        temper_wasm_sdk::FailureDetailValue::String(
            temper_wasm_sdk::BoundedDetailString::new(denial_class)
                .expect("closed denial class is bounded"),
        ),
    );
    if let Some(policy_ids) = policy_ids {
        error.insert_detail_or_omit(
            temper_wasm_sdk::DetailKey::new("policy_count").expect("static detail key is valid"),
            temper_wasm_sdk::FailureDetailValue::Unsigned(policy_ids.len() as u64),
        );
        error.mark_details_omitted();
    }
    if let Some(decision_id) = decision_id
        && let Ok(with_decision_id) = error.clone().with_decision_id(decision_id)
    {
        error = with_decision_id;
    }
    error
}
