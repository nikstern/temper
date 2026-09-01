//! Frozen v1 SDK-linking adapter for persisted bundle regeneration.

use std::collections::{BTreeMap, BTreeSet};

use temper_spec::CanonicalSpecModel;
use temper_spec::bundle::IoaSourceInput;
use temper_spec::csdl::CsdlDocument;

use super::ModuleSdkCodegenError;

pub(super) fn model_v1(
    csdl: &CsdlDocument,
    ioa_sources: &[IoaSourceInput],
) -> Result<CanonicalSpecModel, ModuleSdkCodegenError> {
    let mut automata = BTreeMap::new();
    for source in ioa_sources {
        let automaton = temper_spec::parse_automaton(&source.source).map_err(|error| {
            ModuleSdkCodegenError::InvalidIoaSource {
                entity_type: source.entity_type.clone(),
                message: error.to_string(),
            }
        })?;
        automata.insert(source.entity_type.clone(), automaton);
    }

    let mut lifecycle_properties = BTreeMap::new();
    for schema in &csdl.schemas {
        for entity in &schema.entity_types {
            let entity_type = format!("{}.{}", schema.namespace, entity.name);
            if let Some(automaton) = automata.get(&entity_type) {
                lifecycle_properties.insert(
                    entity_type.clone(),
                    infer_lifecycle_property(csdl, &entity_type, entity, automaton)?,
                );
            }
        }
    }
    Ok(CanonicalSpecModel::from_legacy_v1(
        csdl,
        automata,
        lifecycle_properties,
    ))
}

fn infer_lifecycle_property(
    csdl: &CsdlDocument,
    entity_type: &str,
    entity: &temper_spec::csdl::EntityType,
    automaton: &temper_spec::Automaton,
) -> Result<String, ModuleSdkCodegenError> {
    if let Some(explicit) = automaton.automaton.lifecycle_property.as_ref() {
        if entity
            .properties
            .iter()
            .any(|property| property.name == *explicit)
        {
            return Ok(explicit.clone());
        }
        return Err(ModuleSdkCodegenError::MissingLifecycleProperty {
            entity_type: entity_type.into(),
            initial_state: automaton.automaton.initial.clone(),
        });
    }
    let states = automaton
        .automaton
        .states
        .iter()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    let mut candidates = entity
        .properties
        .iter()
        .filter(|property| {
            let enum_members = enum_members(csdl, &property.type_name);
            let enum_states = enum_members
                .iter()
                .map(String::as_str)
                .collect::<BTreeSet<_>>();
            let domain_accepts = property.type_name == "Edm.String"
                || (!enum_states.is_empty() && states.is_subset(&enum_states));
            domain_accepts
                && (property.default_value.as_deref() == Some(automaton.automaton.initial.as_str())
                    || (!enum_states.is_empty() && enum_states == states))
        })
        .map(|property| property.name.clone())
        .collect::<Vec<_>>();
    candidates.sort();
    match candidates.as_slice() {
        [candidate] => Ok(candidate.clone()),
        [] => Err(ModuleSdkCodegenError::MissingLifecycleProperty {
            entity_type: entity_type.into(),
            initial_state: automaton.automaton.initial.clone(),
        }),
        _ => Err(ModuleSdkCodegenError::AmbiguousLifecycleProperty {
            entity_type: entity_type.into(),
            initial_state: automaton.automaton.initial.clone(),
            candidates,
        }),
    }
}

fn enum_members(csdl: &CsdlDocument, type_name: &str) -> Vec<String> {
    let Some((namespace, name)) = type_name.rsplit_once('.') else {
        return Vec::new();
    };
    csdl.schemas
        .iter()
        .find(|schema| schema.namespace == namespace)
        .and_then(|schema| schema.enum_type(name))
        .map(|enum_type| {
            enum_type
                .members
                .iter()
                .map(|member| member.name.clone())
                .collect()
        })
        .unwrap_or_default()
}
