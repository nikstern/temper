use super::super::*;

pub(super) fn build_creation_manifests(
    tenant: &str,
    model: &CanonicalSpecModel,
    structural_csdl: &CsdlDocument,
    entity_types: impl Iterator<Item = String>,
) -> Result<BTreeMap<String, temper_wasm_sdk::data::ManifestEntityV1>, RegistryError> {
    let submitted = entity_types.collect::<Vec<_>>();
    let mut manifests = BTreeMap::new();
    let mut short_counts = BTreeMap::<String, usize>::new();
    for entity_type in submitted {
        let qualified = qualify_entity_type(structural_csdl, &entity_type).map_err(|error| {
            RegistryError::CanonicalLink {
                tenant: tenant.to_string(),
                source: format!("cannot compile creation contract for '{entity_type}': {error}"),
            }
        })?;
        let generated = temper_codegen::generate_module_sdk(
            model,
            "temper-kernel-create",
            "registry-creation-contract",
            "registry-creation-contract",
            "registry-creation-contract",
            temper_wasm_sdk::data::ModuleDataGrant {
                operations: std::collections::BTreeSet::from([
                    temper_wasm_sdk::data::DataOperationKind::EntityCreate,
                ]),
                entities: vec![temper_wasm_sdk::data::EntityDataGrant {
                    entity_type: qualified.clone(),
                    ..Default::default()
                }],
                ..Default::default()
            },
        )
        .map_err(|error| RegistryError::CanonicalLink {
            tenant: tenant.to_string(),
            source: format!("cannot compile creation contract for '{entity_type}': {error}"),
        })?;
        let entity = generated
            .manifest
            .entities
            .into_iter()
            .next()
            .ok_or_else(|| RegistryError::CanonicalLink {
                tenant: tenant.to_string(),
                source: format!("creation contract manifest omitted '{entity_type}'"),
            })?;
        let short = qualified
            .rsplit('.')
            .next()
            .unwrap_or(&qualified)
            .to_string();
        *short_counts.entry(short.clone()).or_default() += 1;
        manifests.insert(qualified, entity.clone());
        manifests.insert(entity_type, entity);
    }
    for (short, count) in short_counts {
        if count > 1 {
            manifests.remove(&short);
        }
    }
    Ok(manifests)
}
