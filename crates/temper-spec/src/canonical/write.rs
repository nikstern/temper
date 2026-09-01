/// Effective caller-owned stored properties for create and patch operations.
#[derive(Debug, Clone)]
pub struct CanonicalEntityWriteContract {
    /// Whether the structural CSDL explicitly declares the paired write terms.
    explicit: bool,
    /// Stored properties admitted during create.
    create_properties: BTreeSet<String>,
    /// Stored properties admitted during patch.
    patch_properties: BTreeSet<String>,
}

impl CanonicalEntityWriteContract {
    /// Whether the structural CSDL explicitly declares the paired write terms.
    pub const fn explicit(&self) -> bool {
        self.explicit
    }

    /// Stored properties admitted during create.
    pub fn create_properties(&self) -> &BTreeSet<String> {
        &self.create_properties
    }

    /// Stored properties admitted during patch.
    pub fn patch_properties(&self) -> &BTreeSet<String> {
        &self.patch_properties
    }
}

fn legacy_write_contract(
    entity: &EntityType,
    lifecycle_property: Option<&str>,
) -> CanonicalEntityWriteContract {
    let keys = entity
        .key_properties
        .iter()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    let stored = entity
        .properties
        .iter()
        .filter(|property| {
            !keys.contains(property.name.as_str())
                && lifecycle_property != Some(property.name.as_str())
        })
        .map(|property| property.name.clone())
        .collect::<BTreeSet<_>>();
    CanonicalEntityWriteContract {
        explicit: false,
        create_properties: stored.clone(),
        patch_properties: stored,
    }
}

fn link_write_contract(
    entity_type: &str,
    entity: &EntityType,
    lifecycle_property: Option<&str>,
) -> Result<CanonicalEntityWriteContract, BundleError> {
    let create = exact_annotation(&entity.annotations, CREATE_PROPERTIES_TERM, entity_type)?;
    let patch = exact_annotation(&entity.annotations, PATCH_PROPERTIES_TERM, entity_type)?;
    let (create, patch) = match (create, patch) {
        (Some(create), Some(patch)) => (create, patch),
        (None, None) => return Ok(legacy_write_contract(entity, lifecycle_property)),
        _ => {
            return Err(invalid(
                entity_type,
                format!(
                    "{CREATE_PROPERTIES_TERM} and {PATCH_PROPERTIES_TERM} must be declared together"
                ),
            ));
        }
    };
    let create = write_property_set(entity_type, entity, lifecycle_property, create)?;
    let patch = write_property_set(entity_type, entity, lifecycle_property, patch)?;
    Ok(CanonicalEntityWriteContract {
        explicit: true,
        create_properties: create,
        patch_properties: patch,
    })
}

fn exact_annotation<'a>(
    annotations: &'a [Annotation],
    term: &str,
    entity_type: &str,
) -> Result<Option<&'a Annotation>, BundleError> {
    let mut matches = annotations
        .iter()
        .filter(|annotation| annotation.term == term);
    let result = matches.next();
    if matches.next().is_some() {
        return Err(invalid(
            entity_type,
            format!("duplicate write annotation '{term}'"),
        ));
    }
    Ok(result)
}

fn write_property_set(
    entity_type: &str,
    entity: &EntityType,
    lifecycle_property: Option<&str>,
    annotation: &Annotation,
) -> Result<BTreeSet<String>, BundleError> {
    let AnnotationValue::Collection(values) = &annotation.value else {
        return Err(invalid(
            entity_type,
            format!("write annotation '{}' must be a collection", annotation.term),
        ));
    };
    let keys = entity
        .key_properties
        .iter()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    let properties = entity
        .properties
        .iter()
        .map(|property| property.name.as_str())
        .collect::<BTreeSet<_>>();
    let mut result = BTreeSet::new();
    for value in values {
        if !properties.contains(value.as_str()) {
            return Err(invalid(
                entity_type,
                format!(
                    "write annotation '{}' names unknown property '{value}'",
                    annotation.term
                ),
            ));
        }
        if keys.contains(value.as_str()) || lifecycle_property == Some(value.as_str()) {
            return Err(invalid(
                entity_type,
                format!(
                    "write annotation '{}' must not name host-owned property '{value}'",
                    annotation.term
                ),
            ));
        }
        if !result.insert(value.clone()) {
            return Err(invalid(
                entity_type,
                format!(
                    "write annotation '{}' duplicates property '{value}'",
                    annotation.term
                ),
            ));
        }
    }
    Ok(result)
}
