//! Schema-wide success-response reservation and initial-table resolution.

use temper_wasm_sdk::data::{
    CommitToken, CreateOrVerifyResultV1, DataResponseV1, DataResultV1, ModuleDataError,
    ModuleDataErrorKind,
};

use super::super::{
    ApplicationDataInvocation, ModuleDataTarget, data_error, internal_error, short_type,
};

impl ApplicationDataInvocation {
    pub(super) fn lifecycle_field(&self, entity_type: &str) -> String {
        self.authority
            .binding
            .entities
            .iter()
            .find(|entity| entity.entity_type == entity_type)
            .and_then(|entity| {
                entity.properties.iter().find(|property| {
                    property.source == temper_wasm_sdk::data::ManifestValueSourceV1::LifecycleStatus
                })
            })
            .map_or_else(
                || "Status".to_string(),
                |property| property.canonical_name.clone(),
            )
    }

    pub(super) fn reserve_create_or_verify_response(
        &self,
        entity_type: &str,
    ) -> Result<(), ModuleDataError> {
        let runtime_type = short_type(entity_type);
        let manifest = self
            .authority
            .binding
            .entities
            .iter()
            .find(|entity| entity.entity_type == entity_type)
            .expect("granted entity type must exist in the bound schema");
        let table = self.initial_table(runtime_type)?;
        let entity_id = "\0".repeat(super::super::helpers::MAX_CANONICAL_IDENTIFIER_BYTES);
        let candidate = DataResultV1::CreateOrVerify {
            outcome: CreateOrVerifyResultV1::AlreadyMatches {
                commit: CommitToken {
                    entity_type: entity_type.to_string(),
                    entity_id: entity_id.clone(),
                    sequence: u64::MAX,
                },
                value: serde_json::Map::new(),
            },
        };
        let envelope_bytes = serde_json::to_vec(&DataResponseV1::ok(candidate))
            .map_err(|error| internal_error(error.to_string()))?
            .len();
        let encoded_entity_id = serde_json::to_vec(&entity_id)
            .map_err(|error| internal_error(error.to_string()))?
            .len();
        let lifecycle_value_bytes = table
            .states
            .iter()
            .chain(std::iter::once(&table.initial_state))
            .map(|state| serde_json::to_vec(state).map(|encoded| encoded.len()))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| internal_error(error.to_string()))?
            .into_iter()
            .max()
            .unwrap_or(4);
        let mut value_bytes = 0usize;
        for (index, property) in manifest.properties.iter().enumerate() {
            let key_bytes = serde_json::to_vec(&property.canonical_name)
                .map_err(|error| internal_error(error.to_string()))?
                .len();
            let property_bytes = match property.source {
                temper_wasm_sdk::data::ManifestValueSourceV1::EntityId => encoded_entity_id,
                temper_wasm_sdk::data::ManifestValueSourceV1::LifecycleStatus => {
                    lifecycle_value_bytes
                }
                temper_wasm_sdk::data::ManifestValueSourceV1::StoredField => {
                    let normalized = temper_spec::to_snake_case(&property.canonical_name);
                    let inline_bytes = table
                        .state_var_metadata
                        .iter()
                        .find(|(name, _)| temper_spec::to_snake_case(name) == normalized)
                        .and_then(|(_, metadata)| metadata.overflow_inline_max_bytes)
                        .unwrap_or(crate::entity_actor::effects::DEFAULT_FIELD_INLINE_MAX);
                    let default_bytes = property
                        .default_value
                        .as_ref()
                        .map(|value| serde_json::to_vec(value).map(|encoded| encoded.len()))
                        .transpose()
                        .map_err(|error| internal_error(error.to_string()))?
                        .unwrap_or(0);
                    inline_bytes.max(512).max(default_bytes)
                }
                temper_wasm_sdk::data::ManifestValueSourceV1::Input => {
                    return Err(data_error(
                        ModuleDataErrorKind::SchemaMismatch,
                        "InvalidEntityPropertySource",
                        "entity property has an input-only manifest source",
                    ));
                }
            };
            value_bytes = value_bytes
                .saturating_add(usize::from(index > 0))
                .saturating_add(key_bytes)
                .saturating_add(1)
                .saturating_add(property_bytes);
        }
        if envelope_bytes.saturating_add(value_bytes)
            > self.authority.binding.grant.budgets.max_response_bytes as usize
        {
            return Err(data_error(
                ModuleDataErrorKind::BudgetExceeded,
                "ResponseReservationExceeded",
                "create-or-verify success response exceeds the invocation budget",
            ));
        }
        Ok(())
    }

    pub(super) fn initial_table(
        &self,
        entity_type: &str,
    ) -> Result<std::sync::Arc<temper_jit::TransitionTable>, ModuleDataError> {
        let registry = self.state.registry.read().map_err(|_| {
            data_error(
                ModuleDataErrorKind::Internal,
                "RegistryUnavailable",
                "schema registry is unavailable",
            )
        })?;
        let table = match &self.authority.target {
            ModuleDataTarget::TenantGlobal => registry
                .get_table(&self.authority.tenant, entity_type)
                .or_else(|| self.state.transition_tables.get(entity_type).cloned()),
            ModuleDataTarget::Scoped(pin) => registry.get_scoped_table_at_digest(
                &self.authority.tenant,
                &pin.scope,
                &pin.bundle_digest,
                entity_type,
            ),
        };
        table.ok_or_else(|| {
            data_error(
                ModuleDataErrorKind::VerificationFailed,
                "VerificationGateRejected",
                "entity specification is not verified",
            )
        })
    }

    pub(super) fn initial_status(&self, entity_type: &str) -> Result<String, ModuleDataError> {
        Ok(self.initial_table(entity_type)?.initial_state.clone())
    }
}
