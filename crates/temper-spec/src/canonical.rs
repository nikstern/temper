//! IOA-authoritative canonical entity linking for scoped bundle v2.

use std::collections::{BTreeMap, BTreeSet};

use crate::automaton::{ActionParam, Automaton, Guard};
use crate::bundle::{BundleError, BundleErrorCode};
use crate::csdl::{
    Action, Annotation, AnnotationValue, CsdlDocument, EntityType, EnumMember, Parameter,
    ReturnType, emit_csdl_xml, parse_csdl,
};

const STATES_TERM: &str = "Temper.Vocab.StateMachine.States";
const INITIAL_STATE_TERM: &str = "Temper.Vocab.StateMachine.InitialState";
const VALID_FROM_STATES_TERM: &str = "Temper.Vocab.StateMachine.ValidFromStates";
const TARGET_STATE_TERM: &str = "Temper.Vocab.StateMachine.TargetState";
const CREATE_PROPERTIES_TERM: &str = "Temper.Vocab.Write.CreateProperties";
const PATCH_PROPERTIES_TERM: &str = "Temper.Vocab.Write.PatchProperties";

/// One CSDL parameter in a linked bound-action wire contract.
#[derive(Debug, Clone)]
pub struct CanonicalActionParameter {
    /// Case-sensitive CSDL wire name.
    name: String,
    /// Exact CSDL wire type.
    type_name: String,
    /// Whether the wire value may be omitted or null.
    nullable: bool,
    /// Optional authored structural default.
    default_value: Option<String>,
}

/// One callable bound action linked across IOA behavior and CSDL wire shape.
#[derive(Debug, Clone)]
pub struct CanonicalActionContract {
    /// Case-sensitive shared action identity.
    name: String,
    /// Binding parameter from CSDL.
    binding: CanonicalActionParameter,
    /// Non-binding CSDL parameters in canonical wire order.
    parameters: Vec<CanonicalActionParameter>,
    /// Exact CSDL result shape.
    return_type: Option<ReturnType>,
    /// IOA-ordered states from which the action may execute.
    valid_from_states: Vec<String>,
    /// IOA target state, or `None` for a lifecycle-preserving action.
    target_state: Option<String>,
}

/// One fully-qualified CSDL entity and its optional linked IOA behavior.
#[derive(Debug, Clone)]
pub struct CanonicalEntityModel {
    /// Fully-qualified CSDL entity type.
    entity_type: String,
    /// Structural entity declaration with generated behavior removed.
    structural_entity: EntityType,
    /// Parsed authoritative automaton for behavioral entities.
    automaton: Option<Automaton>,
    /// Explicit CSDL lifecycle property for behavioral entities.
    lifecycle_property: Option<String>,
    /// Lifecycle states in IOA declaration order.
    lifecycle_states: Vec<String>,
    /// IOA initial lifecycle state.
    initial_state: Option<String>,
    /// Linked callable action contracts keyed by action identity.
    actions: BTreeMap<String, CanonicalActionContract>,
    /// Effective operation-specific caller write ownership.
    write_contract: CanonicalEntityWriteContract,
}

/// Fully linked immutable schema model consumed by bundle v2 downstream paths.
#[derive(Debug, Clone)]
pub struct CanonicalSpecModel {
    /// Canonically ordered structural CSDL with generated behavior removed.
    structural_csdl: CsdlDocument,
    /// Complete emitted CSDL with IOA-derived behavior regenerated.
    emitted_csdl: CsdlDocument,
    /// Deterministic XML serialization of [`Self::emitted_csdl`].
    emitted_csdl_xml: String,
    /// Every structural entity keyed by fully-qualified CSDL name.
    entities: BTreeMap<String, CanonicalEntityModel>,
}

impl CanonicalActionParameter {
    /// Case-sensitive CSDL wire name.
    pub fn name(&self) -> &str {
        &self.name
    }
    /// Exact CSDL wire type.
    pub fn type_name(&self) -> &str {
        &self.type_name
    }
    /// Whether the wire value may be omitted or null.
    pub fn nullable(&self) -> bool {
        self.nullable
    }
    /// Optional authored structural default.
    pub fn default_value(&self) -> Option<&str> {
        self.default_value.as_deref()
    }
}

impl CanonicalActionContract {
    /// Case-sensitive shared action identity.
    pub fn name(&self) -> &str {
        &self.name
    }
    /// Binding parameter from CSDL.
    pub fn binding(&self) -> &CanonicalActionParameter {
        &self.binding
    }
    /// Non-binding CSDL parameters in canonical wire order.
    pub fn parameters(&self) -> &[CanonicalActionParameter] {
        &self.parameters
    }
    /// Exact CSDL result shape.
    pub fn return_type(&self) -> Option<&ReturnType> {
        self.return_type.as_ref()
    }
    /// IOA-ordered states from which the action may execute.
    pub fn valid_from_states(&self) -> &[String] {
        &self.valid_from_states
    }
    /// IOA target state, if the action changes lifecycle state.
    pub fn target_state(&self) -> Option<&str> {
        self.target_state.as_deref()
    }
}

impl CanonicalEntityModel {
    /// Fully-qualified CSDL entity type.
    pub fn entity_type(&self) -> &str {
        &self.entity_type
    }
    /// Structural entity declaration with generated behavior removed.
    pub fn structural_entity(&self) -> &EntityType {
        &self.structural_entity
    }
    /// Parsed authoritative automaton for a behavioral entity.
    pub fn automaton(&self) -> Option<&Automaton> {
        self.automaton.as_ref()
    }
    /// Explicit lifecycle property for a behavioral entity.
    pub fn lifecycle_property(&self) -> Option<&str> {
        self.lifecycle_property.as_deref()
    }
    /// Lifecycle states in IOA declaration order.
    pub fn lifecycle_states(&self) -> &[String] {
        &self.lifecycle_states
    }
    /// IOA initial lifecycle state.
    pub fn initial_state(&self) -> Option<&str> {
        self.initial_state.as_deref()
    }
    /// Linked callable actions keyed by identity.
    pub fn actions(&self) -> &BTreeMap<String, CanonicalActionContract> {
        &self.actions
    }
    /// Effective operation-specific caller write ownership.
    pub fn write_contract(&self) -> &CanonicalEntityWriteContract {
        &self.write_contract
    }
}

impl CanonicalSpecModel {
    /// Construct the frozen v1 compatibility view used only to restore persisted v1 bundles.
    ///
    /// This deliberately performs no v2 behavioral projection. New compilation must use
    /// [`Self::link_v2_sources`] or [`Self::link_v2`].
    pub fn from_legacy_v1(
        csdl: &CsdlDocument,
        automata: BTreeMap<String, Automaton>,
        lifecycle_properties: BTreeMap<String, String>,
    ) -> Self {
        Self::from_legacy_v1_with_emitted_xml(
            csdl,
            emit_csdl_xml(csdl),
            automata,
            lifecycle_properties,
        )
    }

    /// Construct the frozen v1 compatibility view while retaining persisted metadata bytes.
    pub fn from_legacy_v1_with_emitted_xml(
        csdl: &CsdlDocument,
        emitted_csdl_xml: String,
        automata: BTreeMap<String, Automaton>,
        lifecycle_properties: BTreeMap<String, String>,
    ) -> Self {
        let mut entities = BTreeMap::new();
        for schema in &csdl.schemas {
            for entity in &schema.entity_types {
                let entity_type = format!("{}.{}", schema.namespace, entity.name);
                let automaton = automata.get(&entity_type).cloned();
                entities.insert(
                    entity_type.clone(),
                    CanonicalEntityModel {
                        entity_type: entity_type.clone(),
                        structural_entity: entity.clone(),
                        lifecycle_states: automaton
                            .as_ref()
                            .map(|value| value.automaton.states.clone())
                            .unwrap_or_default(),
                        initial_state: automaton
                            .as_ref()
                            .map(|value| value.automaton.initial.clone()),
                        automaton,
                        lifecycle_property: lifecycle_properties.get(&entity_type).cloned(),
                        actions: BTreeMap::new(),
                        write_contract: legacy_write_contract(
                            entity,
                            lifecycle_properties.get(&entity_type).map(String::as_str),
                        ),
                    },
                );
            }
        }
        for (entity_type, automaton) in automata {
            if entities.contains_key(&entity_type) {
                continue;
            }
            let lifecycle_property = lifecycle_properties.get(&entity_type).cloned();
            entities.insert(
                entity_type.clone(),
                CanonicalEntityModel {
                    entity_type: entity_type.clone(),
                    structural_entity: EntityType {
                        name: entity_type
                            .rsplit('.')
                            .next()
                            .unwrap_or(&entity_type)
                            .to_string(),
                        key_properties: Vec::new(),
                        properties: Vec::new(),
                        navigation_properties: Vec::new(),
                        annotations: Vec::new(),
                        has_stream: false,
                    },
                    lifecycle_states: automaton.automaton.states.clone(),
                    initial_state: Some(automaton.automaton.initial.clone()),
                    automaton: Some(automaton),
                    lifecycle_property,
                    actions: BTreeMap::new(),
                    write_contract: CanonicalEntityWriteContract {
                        explicit: false,
                        create_properties: BTreeSet::new(),
                        patch_properties: BTreeSet::new(),
                    },
                },
            );
        }
        Self {
            structural_csdl: csdl.clone(),
            emitted_csdl: csdl.clone(),
            emitted_csdl_xml,
            entities,
        }
    }

    /// Canonically ordered structural CSDL with generated behavior removed.
    pub fn structural_csdl(&self) -> &CsdlDocument {
        &self.structural_csdl
    }

    /// Complete CSDL with IOA-derived behavior regenerated.
    pub fn emitted_csdl(&self) -> &CsdlDocument {
        &self.emitted_csdl
    }

    /// Deterministic XML serialization of the emitted CSDL.
    pub fn emitted_csdl_xml(&self) -> &str {
        &self.emitted_csdl_xml
    }

    /// Every structural entity keyed by fully-qualified CSDL name.
    pub fn entities(&self) -> &BTreeMap<String, CanonicalEntityModel> {
        &self.entities
    }

    /// Link an already parsed CSDL document and submitted IOA sources under v2 rules.
    pub fn link_v2_sources(
        csdl: &CsdlDocument,
        ioa_sources: &[crate::bundle::IoaSourceInput],
    ) -> Result<Self, BundleError> {
        let mut automata = BTreeMap::new();
        for source in ioa_sources {
            let automaton = crate::automaton::parse_automaton(&source.source).map_err(|error| {
                BundleError::new(
                    BundleErrorCode::InvalidIoa,
                    format!("failed to parse '{}': {error}", source.entity_type),
                )
            })?;
            let short_name = source.entity_type.rsplit('.').next().unwrap_or_default();
            if short_name != automaton.automaton.name {
                return Err(BundleError::new(
                    BundleErrorCode::EntityNameMismatch,
                    format!(
                        "submitted entity '{}' has short name '{short_name}', but its automaton declares '{}'",
                        source.entity_type, automaton.automaton.name
                    ),
                ));
            }
            if automata
                .insert(source.entity_type.clone(), automaton)
                .is_some()
            {
                return Err(BundleError::new(
                    BundleErrorCode::DuplicateSymbol,
                    format!("duplicate IOA entity '{}'", source.entity_type),
                ));
            }
        }
        Self::link_v2(&emit_csdl_xml(csdl), &automata)
    }

    /// Link structural CSDL and fully-qualified parsed IOA automata under v2 rules.
    pub fn link_v2(
        csdl_source: &str,
        automata: &BTreeMap<String, Automaton>,
    ) -> Result<Self, BundleError> {
        let mut document = parse_csdl(csdl_source).map_err(|error| {
            BundleError::new(
                BundleErrorCode::InvalidCsdl,
                format!("failed to parse CSDL: {error}"),
            )
        })?;
        if document.version.is_empty() || document.schemas.is_empty() {
            return Err(BundleError::new(
                BundleErrorCode::InvalidCsdl,
                "CSDL must declare an Edmx version and at least one schema",
            ));
        }

        let locations = entity_locations(&document)?;
        for entity_type in automata.keys() {
            if !locations.contains_key(entity_type) {
                return Err(BundleError::new(
                    BundleErrorCode::InvalidBundle,
                    format!("IOA entity '{entity_type}' is absent from the canonical CSDL"),
                ));
            }
        }

        let mut lifecycle_enum_states = BTreeMap::<String, Vec<String>>::new();
        let mut linked_actions =
            BTreeMap::<String, BTreeMap<String, CanonicalActionContract>>::new();
        let mut lifecycle_properties = BTreeMap::<String, String>::new();

        for (entity_type, automaton) in automata {
            validate_automaton_states(entity_type, automaton)?;
            let (schema_index, entity_index) = locations[entity_type];
            let schema = &document.schemas[schema_index];
            let entity = &schema.entity_types[entity_index];
            let lifecycle_property = automaton
                .automaton
                .lifecycle_property
                .as_deref()
                .filter(|name| !name.trim().is_empty())
                .ok_or_else(|| {
                    BundleError::new(
                        BundleErrorCode::InvalidBundle,
                        format!(
                            "IOA entity '{entity_type}' must declare automaton.lifecycle_property for bundle v2"
                        ),
                    )
                })?;
            let property = entity
                .properties
                .iter()
                .find(|property| property.name == lifecycle_property)
                .ok_or_else(|| {
                    BundleError::new(
                        BundleErrorCode::InvalidBundle,
                        format!(
                            "IOA entity '{entity_type}' lifecycle property '{lifecycle_property}' is absent from CSDL"
                        ),
                    )
                })?;
            validate_lifecycle_property(
                &document,
                schema_index,
                entity_type,
                property,
                automaton,
                &mut lifecycle_enum_states,
            )?;
            validate_entity_annotations(entity_type, entity, automaton)?;
            let actions = link_actions(&document, entity_type, automaton)?;
            lifecycle_properties.insert(entity_type.clone(), lifecycle_property.to_string());
            linked_actions.insert(entity_type.clone(), actions);
        }
        validate_lifecycle_enum_dedication(
            &document,
            &locations,
            &lifecycle_properties,
            &lifecycle_enum_states,
        )?;

        let mut write_contracts = BTreeMap::new();
        for (entity_type, (schema_index, entity_index)) in &locations {
            let entity = &document.schemas[*schema_index].entity_types[*entity_index];
            let contract = link_write_contract(
                entity_type,
                entity,
                lifecycle_properties.get(entity_type).map(String::as_str),
            )?;
            write_contracts.insert(entity_type.clone(), contract);
        }

        strip_behavior(
            &mut document,
            &locations,
            &lifecycle_properties,
            lifecycle_enum_states.keys(),
        );
        crate::bundle::csdl::canonicalize_document(&mut document)?;
        let structural_csdl = document.clone();
        let mut emitted_csdl = document;
        emit_behavior(
            &mut emitted_csdl,
            &locations,
            automata,
            &lifecycle_properties,
            &linked_actions,
            &lifecycle_enum_states,
        )?;
        let emitted_csdl_xml = emit_csdl_xml(&emitted_csdl);

        let emitted_locations = entity_locations(&emitted_csdl)?;
        let structural_locations = entity_locations(&structural_csdl)?;
        let mut entities = BTreeMap::new();
        for (entity_type, (schema_index, entity_index)) in structural_locations {
            let structural_entity =
                structural_csdl.schemas[schema_index].entity_types[entity_index].clone();
            let automaton = automata.get(&entity_type).cloned();
            let actions = linked_actions.remove(&entity_type).unwrap_or_default();
            let lifecycle_property = lifecycle_properties.get(&entity_type).cloned();
            let lifecycle_states = automaton
                .as_ref()
                .map(|value| value.automaton.states.clone())
                .unwrap_or_default();
            let initial_state = automaton
                .as_ref()
                .map(|value| value.automaton.initial.clone());
            let write_contract = write_contracts
                .remove(&entity_type)
                .expect("every structural entity must have a linked write contract");
            debug_assert!(emitted_locations.contains_key(&entity_type));
            entities.insert(
                entity_type.clone(),
                CanonicalEntityModel {
                    entity_type,
                    structural_entity,
                    automaton,
                    lifecycle_property,
                    lifecycle_states,
                    initial_state,
                    actions,
                    write_contract,
                },
            );
        }

        Ok(Self {
            structural_csdl,
            emitted_csdl,
            emitted_csdl_xml,
            entities,
        })
    }

    /// Return the linked behavioral entity for a fully-qualified type.
    pub fn behavioral_entity(&self, entity_type: &str) -> Option<&CanonicalEntityModel> {
        self.entities
            .get(entity_type)
            .filter(|entity| entity.automaton.is_some())
    }

    /// Iterate all parsed authoritative automata in qualified-name order.
    pub fn automata(&self) -> impl Iterator<Item = (&str, &Automaton)> {
        self.entities.iter().filter_map(|(name, entity)| {
            entity
                .automaton
                .as_ref()
                .map(|automaton| (name.as_str(), automaton))
        })
    }
}

include!("canonical/write.rs");
include!("canonical/validation.rs");
include!("canonical/projection.rs");
