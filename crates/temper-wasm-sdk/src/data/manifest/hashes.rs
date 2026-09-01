use std::collections::BTreeMap;

use super::{ModuleSdkManifest, digest_json};

pub(super) fn used_symbol_hashes(
    manifest: &ModuleSdkManifest,
) -> Result<BTreeMap<String, String>, String> {
    let mut hashes = BTreeMap::new();
    for entity in &manifest.entities {
        let entity_hash = if entity.lifecycle_states.is_empty() {
            digest_json(&(
                &entity.entity_type,
                &entity.entity_set,
                &entity.generated_name,
            ))?
        } else {
            digest_json(&(
                "manifest-entity-lifecycle/v1",
                &entity.entity_type,
                &entity.entity_set,
                &entity.generated_name,
                &entity.lifecycle_states,
            ))?
        };
        hashes.insert(format!("entity:{}", entity.entity_type), entity_hash);
        hashes.insert(
            format!("entity_set:{}", entity.entity_set),
            digest_json(&entity.entity_set)?,
        );
        for property in &entity.properties {
            hashes.insert(
                format!(
                    "property:{}:{}",
                    entity.entity_type, property.canonical_name
                ),
                digest_json(property)?,
            );
        }
        for action in &entity.actions {
            hashes.insert(
                format!("action:{}:{}", entity.entity_type, action.canonical_name),
                digest_json(action)?,
            );
        }
    }
    Ok(hashes)
}
