use std::collections::BTreeSet;

use super::ModuleSdkManifest;

pub(super) fn compatible_action_nullability_widenings(
    prior: &ModuleSdkManifest,
    candidate: &ModuleSdkManifest,
) -> Result<BTreeSet<String>, String> {
    let mut compatible = BTreeSet::new();
    for prior_entity in &prior.entities {
        let Some(candidate_entity) = candidate
            .entities
            .iter()
            .find(|entity| entity.entity_type == prior_entity.entity_type)
        else {
            continue;
        };
        for prior_action in &prior_entity.actions {
            let Some(candidate_action) = candidate_entity
                .actions
                .iter()
                .find(|action| action.canonical_name == prior_action.canonical_name)
            else {
                continue;
            };
            if prior_action == candidate_action {
                continue;
            }
            if prior_action.generated_name != candidate_action.generated_name
                || prior_action.result_type != candidate_action.result_type
                || prior_action.result_enum_members != candidate_action.result_enum_members
                || prior_action.composite != candidate_action.composite
                || prior_action.parameters.len() != candidate_action.parameters.len()
            {
                continue;
            }
            let mut widened = false;
            let mut otherwise_equal = true;
            for prior_parameter in &prior_action.parameters {
                let Some(candidate_parameter) = candidate_action
                    .parameters
                    .iter()
                    .find(|parameter| parameter.canonical_name == prior_parameter.canonical_name)
                else {
                    otherwise_equal = false;
                    break;
                };
                if prior_parameter.nullable && !candidate_parameter.nullable {
                    return Err(format!(
                        "module data action parameter nullability narrowing: entity='{}' action='{}' parameter='{}' old_nullable=true new_nullable=false",
                        prior_entity.entity_type,
                        prior_action.canonical_name,
                        prior_parameter.canonical_name,
                    ));
                }
                let mut prior_without_nullability = prior_parameter.clone();
                let mut candidate_without_nullability = candidate_parameter.clone();
                prior_without_nullability.nullable = false;
                candidate_without_nullability.nullable = false;
                if prior_without_nullability != candidate_without_nullability {
                    otherwise_equal = false;
                    break;
                }
                widened |= !prior_parameter.nullable && candidate_parameter.nullable;
            }
            if otherwise_equal && widened {
                compatible.insert(format!(
                    "action:{}:{}",
                    prior_entity.entity_type, prior_action.canonical_name
                ));
            }
        }
    }
    Ok(compatible)
}
