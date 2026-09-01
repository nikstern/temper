fn strip_behavior<'a>(
    document: &mut CsdlDocument,
    locations: &BTreeMap<String, (usize, usize)>,
    lifecycle_properties: &BTreeMap<String, String>,
    lifecycle_enums: impl Iterator<Item = &'a String>,
) {
    let lifecycle_enums = lifecycle_enums.cloned().collect::<BTreeSet<_>>();
    for (entity_type, (schema_index, entity_index)) in locations {
        let schema = &mut document.schemas[*schema_index];
        let entity = &mut schema.entity_types[*entity_index];
        if let Some(property_name) = lifecycle_properties.get(entity_type) {
            entity.annotations.retain(|annotation| {
                !is_term(&annotation.term, STATES_TERM) && !is_term(&annotation.term, INITIAL_STATE_TERM)
            });
            if let Some(property) = entity.properties.iter_mut().find(|property| property.name == *property_name) {
                property.default_value = None;
            }
            for action in &mut schema.actions {
                if action.binding_type() == Some(entity_type.as_str()) {
                    action.annotations.retain(|annotation| {
                        !is_term(&annotation.term, VALID_FROM_STATES_TERM)
                            && !is_term(&annotation.term, TARGET_STATE_TERM)
                    });
                }
            }
        }
    }
    for entity_type in lifecycle_properties.keys() {
        for schema in &mut document.schemas {
            for action in &mut schema.actions {
                if action.binding_type() == Some(entity_type.as_str()) {
                    action.annotations.retain(|annotation| {
                        !is_term(&annotation.term, VALID_FROM_STATES_TERM)
                            && !is_term(&annotation.term, TARGET_STATE_TERM)
                    });
                }
            }
        }
    }
    for schema in &mut document.schemas {
        for enum_type in &mut schema.enum_types {
            let qualified = format!("{}.{}", schema.namespace, enum_type.name);
            if lifecycle_enums.contains(&qualified) {
                enum_type.members.clear();
            }
        }
    }
}

fn emit_behavior(
    document: &mut CsdlDocument,
    _original_locations: &BTreeMap<String, (usize, usize)>,
    automata: &BTreeMap<String, Automaton>,
    lifecycle_properties: &BTreeMap<String, String>,
    actions: &BTreeMap<String, BTreeMap<String, CanonicalActionContract>>,
    lifecycle_enum_states: &BTreeMap<String, Vec<String>>,
) -> Result<(), BundleError> {
    let locations = entity_locations(document)?;
    for (entity_type, automaton) in automata {
        let (schema_index, entity_index) = locations[entity_type];
        let schema = &mut document.schemas[schema_index];
        let entity = &mut schema.entity_types[entity_index];
        entity.annotations.push(Annotation {
            term: STATES_TERM.into(),
            value: AnnotationValue::Collection(automaton.automaton.states.clone()),
        });
        entity.annotations.push(Annotation {
            term: INITIAL_STATE_TERM.into(),
            value: AnnotationValue::String(automaton.automaton.initial.clone()),
        });
        entity.annotations.sort_by(|left, right| left.term.cmp(&right.term));
        let property_name = &lifecycle_properties[entity_type];
        let property = entity
            .properties
            .iter_mut()
            .find(|property| property.name == *property_name)
            .expect("validated lifecycle property must remain present");
        property.default_value = Some(automaton.automaton.initial.clone());
    }
    for entity_type in automata.keys() {
        for schema in &mut document.schemas {
            for action in &mut schema.actions {
                if action.binding_type() != Some(entity_type.as_str()) {
                    continue;
                }
                let contract = &actions[entity_type][&action.name];
                action.annotations.push(Annotation {
                    term: VALID_FROM_STATES_TERM.into(),
                    value: AnnotationValue::Collection(contract.valid_from_states.clone()),
                });
                if let Some(target) = &contract.target_state {
                    action.annotations.push(Annotation {
                        term: TARGET_STATE_TERM.into(),
                        value: AnnotationValue::String(target.clone()),
                    });
                }
                action.annotations.sort_by(|left, right| left.term.cmp(&right.term));
            }
        }
    }
    for (enum_name, states) in lifecycle_enum_states {
        let (_, enum_type) = resolve_enum_mut(document, enum_name).expect("validated lifecycle enum must remain present");
        enum_type.members = states
            .iter()
            .enumerate()
            .map(|(index, state)| EnumMember {
                name: state.clone(),
                value: Some(index as i64),
            })
            .collect();
    }
    Ok(())
}

fn behavior_annotation<'a>(annotations: &'a [Annotation], term: &str) -> Option<&'a Annotation> {
    annotations.iter().find(|annotation| is_term(&annotation.term, term))
}

fn is_term(actual: &str, expected: &str) -> bool {
    actual == expected || actual == expected.trim_start_matches("Temper.Vocab.")
}

fn collection(annotation: &Annotation, entity_type: &str, term: &str) -> Result<Vec<String>, BundleError> {
    match &annotation.value {
        AnnotationValue::Collection(values) => Ok(values.clone()),
        _ => Err(invalid(entity_type, format!("{term} must be a string collection"))),
    }
}

fn string(annotation: &Annotation, entity_type: &str, term: &str) -> Result<String, BundleError> {
    match &annotation.value {
        AnnotationValue::String(value) => Ok(value.clone()),
        _ => Err(invalid(entity_type, format!("{term} must be a string"))),
    }
}

fn validate_partial_set(
    entity_type: &str,
    term: &str,
    actual: &[String],
    expected: &[String],
) -> Result<(), BundleError> {
    let expected = expected.iter().map(String::as_str).collect::<BTreeSet<_>>();
    let mut seen = BTreeSet::new();
    for value in actual {
        if !seen.insert(value.as_str()) || !expected.contains(value.as_str()) {
            return Err(invalid(
                entity_type,
                format!("{term} value '{value}' is duplicated or contradicts IOA"),
            ));
        }
    }
    Ok(())
}

fn qualify_type(namespace: &str, type_name: &str) -> String {
    if type_name.contains('.') {
        type_name.to_string()
    } else {
        format!("{namespace}.{type_name}")
    }
}

fn resolve_enum<'a>(
    document: &'a CsdlDocument,
    qualified_name: &str,
) -> Option<(&'a str, &'a crate::csdl::EnumType)> {
    let (namespace, name) = qualified_name.rsplit_once('.')?;
    document
        .schemas
        .iter()
        .find(|schema| schema.namespace == namespace)
        .and_then(|schema| schema.enum_type(name).map(|value| (schema.namespace.as_str(), value)))
}

fn resolve_enum_mut<'a>(
    document: &'a mut CsdlDocument,
    qualified_name: &str,
) -> Option<(&'a str, &'a mut crate::csdl::EnumType)> {
    let (namespace, name) = qualified_name.rsplit_once('.')?;
    document
        .schemas
        .iter_mut()
        .find(|schema| schema.namespace == namespace)
        .and_then(|schema| {
            let namespace = schema.namespace.as_str();
            schema
                .enum_types
                .iter_mut()
                .find(|value| value.name == name)
                .map(|value| (namespace, value))
        })
}

fn parameter_contract(parameter: &Parameter) -> CanonicalActionParameter {
    CanonicalActionParameter {
        name: parameter.name.clone(),
        type_name: parameter.type_name.clone(),
        nullable: parameter.nullable,
        default_value: parameter.default_value.clone(),
    }
}

fn invalid(entity_type: &str, message: impl Into<String>) -> BundleError {
    BundleError::new(
        BundleErrorCode::InvalidBundle,
        format!("{entity_type}: {}", message.into()),
    )
}
