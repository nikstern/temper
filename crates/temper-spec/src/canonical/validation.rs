fn entity_locations(document: &CsdlDocument) -> Result<BTreeMap<String, (usize, usize)>, BundleError> {
    let mut locations = BTreeMap::new();
    for (schema_index, schema) in document.schemas.iter().enumerate() {
        for (entity_index, entity) in schema.entity_types.iter().enumerate() {
            let qualified = format!("{}.{}", schema.namespace, entity.name);
            if locations.insert(qualified.clone(), (schema_index, entity_index)).is_some() {
                return Err(BundleError::new(
                    BundleErrorCode::DuplicateSymbol,
                    format!("duplicate CSDL entity type '{qualified}'"),
                ));
            }
        }
    }
    Ok(locations)
}

fn validate_automaton_states(entity_type: &str, automaton: &Automaton) -> Result<(), BundleError> {
    let states = &automaton.automaton.states;
    if states.is_empty() {
        return Err(invalid(entity_type, "IOA lifecycle state list must not be empty"));
    }
    let mut seen = BTreeSet::new();
    for state in states {
        if state.is_empty() || !seen.insert(state.as_str()) {
            return Err(invalid(
                entity_type,
                format!("IOA lifecycle state '{state}' is empty or duplicated"),
            ));
        }
    }
    if !seen.contains(automaton.automaton.initial.as_str()) {
        return Err(invalid(
            entity_type,
            format!(
                "IOA initial state '{}' is not declared in states",
                automaton.automaton.initial
            ),
        ));
    }
    Ok(())
}

fn validate_lifecycle_property(
    document: &CsdlDocument,
    schema_index: usize,
    entity_type: &str,
    property: &crate::csdl::Property,
    automaton: &Automaton,
    lifecycle_enum_states: &mut BTreeMap<String, Vec<String>>,
) -> Result<(), BundleError> {
    if property.nullable {
        return Err(invalid(
            entity_type,
            format!("lifecycle property '{}' must be non-nullable", property.name),
        ));
    }
    if let Some(default) = &property.default_value
        && default != &automaton.automaton.initial
    {
        return Err(invalid(
            entity_type,
            format!(
                "lifecycle property '{}' default '{}' contradicts IOA initial state '{}'",
                property.name, default, automaton.automaton.initial
            ),
        ));
    }
    if property.type_name == "Edm.String" {
        return Ok(());
    }
    let enum_name = qualify_type(&document.schemas[schema_index].namespace, &property.type_name);
    let (enum_schema, enum_type) = resolve_enum(document, &enum_name).ok_or_else(|| {
        invalid(
            entity_type,
            format!(
                "lifecycle property '{}' type '{}' is neither Edm.String nor a declared enum",
                property.name, property.type_name
            ),
        )
    })?;
    let _ = enum_schema;
    let mut seen = BTreeSet::new();
    for (authored_index, member) in enum_type.members.iter().enumerate() {
        if !seen.insert(member.name.as_str()) {
            return Err(invalid(
                entity_type,
                format!("lifecycle enum '{enum_name}' duplicates member '{}'", member.name),
            ));
        }
        let Some(position) = automaton
            .automaton
            .states
            .iter()
            .position(|state| state == &member.name)
        else {
            return Err(invalid(
                entity_type,
                format!(
                    "lifecycle enum '{enum_name}' member '{}' contradicts IOA states",
                    member.name
                ),
            ));
        };
        let value = member.value.unwrap_or(authored_index as i64);
        if value != position as i64 {
            return Err(invalid(
                entity_type,
                format!(
                    "lifecycle enum '{enum_name}' member '{}' value {value} contradicts IOA ordinal {position}",
                    member.name
                ),
            ));
        }
    }
    match lifecycle_enum_states.get(&enum_name) {
        Some(states) if states != &automaton.automaton.states => Err(invalid(
            entity_type,
            format!("shared lifecycle enum '{enum_name}' has incompatible IOA state order"),
        )),
        Some(_) => Ok(()),
        None => {
            lifecycle_enum_states.insert(enum_name, automaton.automaton.states.clone());
            Ok(())
        }
    }
}

fn validate_entity_annotations(
    entity_type: &str,
    entity: &EntityType,
    automaton: &Automaton,
) -> Result<(), BundleError> {
    validate_unique_behavior_annotations(entity_type, &entity.annotations, &[STATES_TERM, INITIAL_STATE_TERM])?;
    if let Some(annotation) = behavior_annotation(&entity.annotations, STATES_TERM) {
        let values = collection(annotation, entity_type, STATES_TERM)?;
        validate_partial_set(entity_type, STATES_TERM, &values, &automaton.automaton.states)?;
    }
    if let Some(annotation) = behavior_annotation(&entity.annotations, INITIAL_STATE_TERM) {
        let value = string(annotation, entity_type, INITIAL_STATE_TERM)?;
        if value != automaton.automaton.initial {
            return Err(invalid(
                entity_type,
                format!(
                    "{INITIAL_STATE_TERM} '{}' contradicts IOA initial state '{}'",
                    value, automaton.automaton.initial
                ),
            ));
        }
    }
    Ok(())
}

fn link_actions(
    document: &CsdlDocument,
    entity_type: &str,
    automaton: &Automaton,
) -> Result<BTreeMap<String, CanonicalActionContract>, BundleError> {
    let ioa_actions = automaton
        .actions
        .iter()
        .filter(|action| action.kind != "output")
        .map(|action| (action.name.as_str(), action))
        .collect::<BTreeMap<_, _>>();
    if ioa_actions.len()
        != automaton
            .actions
            .iter()
            .filter(|action| action.kind != "output")
            .count()
    {
        return Err(invalid(entity_type, "IOA callable action names must be unique"));
    }
    let mut csdl_actions = BTreeMap::<&str, &Action>::new();
    for schema in &document.schemas {
        for action in &schema.actions {
            if action.binding_type() == Some(entity_type)
                && let Some(existing) = csdl_actions.insert(action.name.as_str(), action)
            {
                    return Err(invalid(
                        entity_type,
                        format!(
                            "bound CSDL action '{}' is ambiguous between bindings {:?} and {:?}",
                            action.name,
                            existing.binding_type(),
                            action.binding_type()
                        ),
                    ));
            }
        }
    }
    let ioa_names = ioa_actions.keys().copied().collect::<BTreeSet<_>>();
    let csdl_names = csdl_actions.keys().copied().collect::<BTreeSet<_>>();
    if ioa_names != csdl_names {
        let missing = ioa_names.difference(&csdl_names).copied().collect::<Vec<_>>();
        let extra = csdl_names.difference(&ioa_names).copied().collect::<Vec<_>>();
        return Err(invalid(
            entity_type,
            format!(
                "callable action parity mismatch: missing bound CSDL actions {missing:?}, extra bound CSDL actions {extra:?}"
            ),
        ));
    }

    let mut contracts = BTreeMap::new();
    for (name, ioa_action) in ioa_actions {
        let csdl_action = csdl_actions[name];
        validate_unique_behavior_annotations(
            entity_type,
            &csdl_action.annotations,
            &[VALID_FROM_STATES_TERM, TARGET_STATE_TERM],
        )?;
        let binding = csdl_action.parameters.first().ok_or_else(|| {
            invalid(entity_type, format!("bound action '{name}' has no binding parameter"))
        })?;
        if binding.nullable {
            return Err(invalid(
                entity_type,
                format!("bound action '{name}' binding parameter must be non-nullable"),
            ));
        }
        let parameters = link_parameters(
            document,
            entity_type,
            name,
            &ioa_action.params,
            &csdl_action.parameters[1..],
        )?;
        let valid_from_states = effective_valid_from(entity_type, ioa_action, automaton)?;
        if let Some(annotation) = behavior_annotation(&csdl_action.annotations, VALID_FROM_STATES_TERM) {
            let values = collection(annotation, entity_type, VALID_FROM_STATES_TERM)?;
            validate_partial_set(entity_type, VALID_FROM_STATES_TERM, &values, &valid_from_states)?;
        }
        if let Some(annotation) = behavior_annotation(&csdl_action.annotations, TARGET_STATE_TERM) {
            let value = string(annotation, entity_type, TARGET_STATE_TERM)?;
            if ioa_action.to.as_deref() != Some(value.as_str()) {
                return Err(invalid(
                    entity_type,
                    format!(
                        "action '{name}' {TARGET_STATE_TERM} '{value}' contradicts IOA target {:?}",
                        ioa_action.to
                    ),
                ));
            }
        }
        contracts.insert(
            name.to_string(),
            CanonicalActionContract {
                name: name.to_string(),
                binding: parameter_contract(binding),
                parameters,
                return_type: csdl_action.return_type.clone(),
                valid_from_states,
                target_state: ioa_action.to.clone(),
            },
        );
    }
    Ok(contracts)
}

fn validate_unique_behavior_annotations(
    entity_type: &str,
    annotations: &[Annotation],
    terms: &[&str],
) -> Result<(), BundleError> {
    for term in terms {
        if annotations
            .iter()
            .filter(|annotation| is_term(&annotation.term, term))
            .count()
            > 1
        {
            return Err(invalid(
                entity_type,
                format!("duplicate behavioral annotation '{term}'"),
            ));
        }
    }
    Ok(())
}

fn link_parameters(
    document: &CsdlDocument,
    entity_type: &str,
    action_name: &str,
    ioa: &[ActionParam],
    csdl: &[Parameter],
) -> Result<Vec<CanonicalActionParameter>, BundleError> {
    let mut ioa_by_name = BTreeMap::new();
    for parameter in ioa {
        let normalized = crate::naming::to_snake_case(parameter.name());
        if ioa_by_name.insert(normalized.clone(), parameter).is_some() {
            return Err(invalid(
                entity_type,
                format!("action '{action_name}' IOA parameters collide as '{normalized}'"),
            ));
        }
    }
    let mut csdl_by_name = BTreeMap::new();
    for parameter in csdl {
        let normalized = crate::naming::to_snake_case(&parameter.name);
        if csdl_by_name.insert(normalized.clone(), parameter).is_some() {
            return Err(invalid(
                entity_type,
                format!("action '{action_name}' CSDL parameters collide as '{normalized}'"),
            ));
        }
    }
    if ioa_by_name.keys().collect::<Vec<_>>() != csdl_by_name.keys().collect::<Vec<_>>() {
        return Err(invalid(
            entity_type,
            format!("action '{action_name}' IOA and CSDL parameter names differ"),
        ));
    }
    for (normalized, ioa_parameter) in ioa_by_name {
        let csdl_parameter = csdl_by_name[&normalized];
        if ioa_parameter.nullable() != csdl_parameter.nullable {
            return Err(invalid(
                entity_type,
                format!(
                    "action '{action_name}' parameter '{}' nullability differs between IOA and CSDL",
                    ioa_parameter.name()
                ),
            ));
        }
        if let ActionParam::Typed {
            param_type,
            entity_type: reference_type,
            ..
        } = ioa_parameter
            && !semantic_type_compatible(
                document,
                param_type,
                reference_type.as_deref(),
                &csdl_parameter.type_name,
            )
        {
            return Err(invalid(
                entity_type,
                format!(
                    "action '{action_name}' parameter '{}' IOA type '{param_type}' is incompatible with CSDL type '{}'",
                    ioa_parameter.name(), csdl_parameter.type_name
                ),
            ));
        }
    }
    Ok(csdl.iter().map(parameter_contract).collect())
}

fn semantic_type_compatible(
    document: &CsdlDocument,
    ioa: &str,
    reference_type: Option<&str>,
    csdl: &str,
) -> bool {
    match ioa {
        "string" => csdl == "Edm.String",
        "bool" | "boolean" => csdl == "Edm.Boolean",
        "counter" | "int" | "integer" => {
            matches!(csdl, "Edm.Byte" | "Edm.SByte" | "Edm.Int16" | "Edm.Int32" | "Edm.Int64")
        }
        "ref" => reference_type.is_some_and(|target| {
            if target.contains('.') {
                return target == csdl;
            }
            let matches = document
                .schemas
                .iter()
                .flat_map(|schema| {
                    schema
                        .entity_types
                        .iter()
                        .filter(move |entity| entity.name == target)
                        .map(move |entity| format!("{}.{}", schema.namespace, entity.name))
                })
                .collect::<Vec<_>>();
            matches.as_slice() == [csdl]
        }),
        explicit => explicit == csdl,
    }
}

fn effective_valid_from(
    entity_type: &str,
    action: &crate::automaton::Action,
    automaton: &Automaton,
) -> Result<Vec<String>, BundleError> {
    let state_order = &automaton.automaton.states;
    let declared = checked_state_set(entity_type, &action.name, "from", &action.from, state_order)?;
    let mut effective = if action.from.is_empty() {
        state_order.iter().map(String::as_str).collect::<BTreeSet<_>>()
    } else {
        declared
    };
    for guard in &action.guard {
        if let Guard::StateIn { values } = guard {
            let guarded = checked_state_set(entity_type, &action.name, "state_in", values, state_order)?;
            effective.retain(|state| guarded.contains(state));
        }
    }
    Ok(state_order
        .iter()
        .filter(|state| effective.contains(state.as_str()))
        .cloned()
        .collect())
}

fn checked_state_set<'a>(
    entity_type: &str,
    action: &str,
    source: &str,
    values: &'a [String],
    state_order: &'a [String],
) -> Result<BTreeSet<&'a str>, BundleError> {
    let allowed = state_order.iter().map(String::as_str).collect::<BTreeSet<_>>();
    let mut result = BTreeSet::new();
    for value in values {
        if !allowed.contains(value.as_str()) || !result.insert(value.as_str()) {
            return Err(invalid(
                entity_type,
                format!("action '{action}' {source} state '{value}' is unknown or duplicated"),
            ));
        }
    }
    Ok(result)
}

fn validate_lifecycle_enum_dedication(
    document: &CsdlDocument,
    locations: &BTreeMap<String, (usize, usize)>,
    lifecycle_properties: &BTreeMap<String, String>,
    lifecycle_enum_states: &BTreeMap<String, Vec<String>>,
) -> Result<(), BundleError> {
    for enum_name in lifecycle_enum_states.keys() {
        for (entity_type, (schema_index, entity_index)) in locations {
            let schema = &document.schemas[*schema_index];
            let entity = &schema.entity_types[*entity_index];
            for property in &entity.properties {
                if qualify_type(&schema.namespace, &property.type_name) == *enum_name
                    && lifecycle_properties.get(entity_type) != Some(&property.name)
                {
                    return Err(invalid(
                        entity_type,
                        format!(
                            "lifecycle enum '{enum_name}' is also used by unrelated property '{}'",
                            property.name
                        ),
                    ));
                }
            }
        }
    }
    Ok(())
}
