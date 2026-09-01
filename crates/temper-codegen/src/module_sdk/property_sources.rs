use temper_spec::CanonicalSpecModel;
use temper_spec::csdl::EntityType;
use temper_wasm_sdk::data::{ManifestPropertyV1, ManifestValueSourceV1};

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

    let Some(canonical) = model.behavioral_entity(entity_type) else {
        return Ok(Vec::new());
    };
    let lifecycle_property = canonical
        .lifecycle_property()
        .expect("behavioral canonical entity must name a lifecycle property");
    let lifecycle_property = properties
        .iter_mut()
        .find(|property| property.canonical_name == lifecycle_property)
        .ok_or_else(|| ModuleSdkCodegenError::MissingSymbol {
            entity_type: entity_type.into(),
            symbol: format!("canonical lifecycle property '{lifecycle_property}'"),
        })?;
    lifecycle_property.source = ManifestValueSourceV1::LifecycleStatus;
    Ok(canonical.lifecycle_states().to_vec())
}
