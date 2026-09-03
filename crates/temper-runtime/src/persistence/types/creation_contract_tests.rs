use super::*;

fn field(name: &str, value: &str) -> CreationContractField {
    CreationContractField {
        name: name.to_string(),
        type_descriptor: "Edm.String".to_string(),
        value_source: "stored_field".to_string(),
        nullable: false,
        create_required: Some(true),
        default_digest: String::new(),
        value_digest: value.to_string(),
    }
}

fn contract(fields: Vec<CreationContractField>) -> CreationContract {
    let digest = format!("{fields:?}");
    CreationContract {
        version: CREATION_CONTRACT_VERSION_V1,
        schema_digest: "schema".to_string(),
        fields,
        digest,
    }
}

#[test]
fn removed_fields_are_dropped_and_added_optional_defaults_are_synthesized() {
    let stored = contract(vec![field("Id", "one"), field("Removed", "old")]);
    let mut added = field("Added", "default");
    added.nullable = true;
    added.create_required = Some(false);
    added.default_digest = "default".to_string();
    let requested = contract(vec![field("Id", "one"), added]);
    assert_eq!(
        compare_creation_contracts(&stored, &requested),
        CreationContractComparison::Matches
    );
}

#[test]
fn added_nullable_but_required_field_requires_migration() {
    let stored = contract(vec![field("Id", "one")]);
    let mut added = field("RequiredNullable", "null");
    added.nullable = true;
    added.create_required = Some(true);
    assert_eq!(
        compare_creation_contracts(&stored, &contract(vec![field("Id", "one"), added])),
        CreationContractComparison::MigrationRequired
    );
}

#[test]
fn added_required_or_changed_type_requires_migration() {
    let stored = contract(vec![field("Id", "one")]);
    let requested = contract(vec![field("Id", "one"), field("Required", "new")]);
    assert_eq!(
        compare_creation_contracts(&stored, &requested),
        CreationContractComparison::MigrationRequired
    );

    let mut changed = field("Id", "one");
    changed.type_descriptor = "Edm.Int64".to_string();
    assert_eq!(
        compare_creation_contracts(&stored, &contract(vec![changed])),
        CreationContractComparison::MigrationRequired
    );
}

#[test]
fn conflicts_are_sorted_and_bounded() {
    let stored = contract(
        (0..40)
            .rev()
            .map(|index| field(&format!("Field{index:02}"), "old"))
            .collect(),
    );
    let requested = contract(
        (0..40)
            .map(|index| field(&format!("Field{index:02}"), "new"))
            .collect(),
    );
    let CreationContractComparison::Conflict { fields, truncated } =
        compare_creation_contracts(&stored, &requested)
    else {
        panic!("expected conflict");
    };
    assert!(truncated);
    assert_eq!(fields.len(), CREATE_OR_VERIFY_CONFLICT_FIELD_BUDGET);
    assert_eq!(fields.first().map(String::as_str), Some("Field00"));
    assert_eq!(fields.last().map(String::as_str), Some("Field31"));
}

#[test]
fn legacy_contract_without_requiredness_fails_closed_even_when_digest_matches() {
    let legacy: CreationContract = serde_json::from_value(serde_json::json!({
        "version": 1,
        "schema_digest": "schema",
        "fields": [{
            "name": "Id",
            "type_descriptor": "Edm.String",
            "value_source": "entity_id",
            "nullable": false,
            "default_digest": "",
            "value_digest": "one"
        }],
        "digest": "same"
    }))
    .expect("legacy contract decodes with an absent marker");
    let mut current = contract(vec![field("Id", "one")]);
    current.digest = "same".to_string();
    assert_eq!(
        compare_creation_contracts(&legacy, &current),
        CreationContractComparison::MigrationRequired
    );
}
