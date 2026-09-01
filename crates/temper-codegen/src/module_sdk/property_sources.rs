use temper_spec::CanonicalSpecModel;
use temper_spec::csdl::EntityType;
use temper_wasm_sdk::data::{
    ManifestCreateRoleV1, ManifestPatchRoleV1, ManifestPropertyV1, ManifestPropertyWritePolicyV1,
    ManifestValueSourceV1,
};

use super::ModuleSdkCodegenError;

pub(super) fn assign_entity_property_sources(
    model: &CanonicalSpecModel,
    entity_type: &str,
    entity: &EntityType,
    properties: &mut [ManifestPropertyV1],
) -> Result<Vec<String>, ModuleSdkCodegenError> {
    if entity.key_properties.len() != 1 {
        return Err(ModuleSdkCodegenError::UnsupportedEntityKey {
            entity_type: entity_type.into(),
            key_properties: entity.key_properties.clone(),
        });
    }
    let key = &entity.key_properties[0];
    let key_property = properties
        .iter_mut()
        .find(|property| property.canonical_name == *key)
        .ok_or_else(|| ModuleSdkCodegenError::MissingSymbol {
            entity_type: entity_type.into(),
            symbol: format!("entity key property '{key}'"),
        })?;
    key_property.source = ManifestValueSourceV1::EntityId;

    let canonical = model
        .entities()
        .get(entity_type)
        .expect("resolved generated entity must exist in the canonical model");
    if let Some(lifecycle_property) = canonical.lifecycle_property() {
        let lifecycle_property = properties
            .iter_mut()
            .find(|property| property.canonical_name == lifecycle_property)
            .ok_or_else(|| ModuleSdkCodegenError::MissingSymbol {
                entity_type: entity_type.into(),
                symbol: format!("canonical lifecycle property '{lifecycle_property}'"),
            })?;
        lifecycle_property.source = ManifestValueSourceV1::LifecycleStatus;
    }

    let write_contract = canonical.write_contract();
    for property in properties {
        let create = match property.source {
            ManifestValueSourceV1::EntityId => ManifestCreateRoleV1::Required,
            ManifestValueSourceV1::LifecycleStatus | ManifestValueSourceV1::Input => {
                ManifestCreateRoleV1::Forbidden
            }
            ManifestValueSourceV1::StoredField
                if write_contract
                    .create_properties()
                    .contains(&property.canonical_name) =>
            {
                if property.nullable || property.default_value.is_some() {
                    ManifestCreateRoleV1::Optional
                } else {
                    ManifestCreateRoleV1::Required
                }
            }
            ManifestValueSourceV1::StoredField => ManifestCreateRoleV1::Forbidden,
        };
        let patch = match property.source {
            ManifestValueSourceV1::StoredField
                if write_contract
                    .patch_properties()
                    .contains(&property.canonical_name) =>
            {
                ManifestPatchRoleV1::Writable
            }
            ManifestValueSourceV1::StoredField
            | ManifestValueSourceV1::EntityId
            | ManifestValueSourceV1::LifecycleStatus
            | ManifestValueSourceV1::Input => ManifestPatchRoleV1::Forbidden,
        };
        property.write_policy = Some(ManifestPropertyWritePolicyV1 { create, patch });
    }
    Ok(canonical.lifecycle_states().to_vec())
}
