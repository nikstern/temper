use super::*;
use temper_wasm_sdk::data::{ManifestPatchRoleV1, ManifestPropertyWritePolicyV1};

fn property(name: &str, type_name: &str, default: Option<serde_json::Value>) -> ManifestPropertyV1 {
    ManifestPropertyV1 {
        canonical_name: name.into(),
        generated_name: name.to_lowercase(),
        type_name: type_name.into(),
        nullable: default.is_none(),
        source: ManifestValueSourceV1::StoredField,
        default_value: default,
        enum_members: Vec::new(),
        write_policy: Some(ManifestPropertyWritePolicyV1 {
            create: ManifestCreateRoleV1::Optional,
            patch: ManifestPatchRoleV1::Writable,
        }),
    }
}

#[test]
fn numeric_forms_defaults_and_nulls_are_canonical() {
    let entity = ManifestEntityV1 {
        entity_type: "Test.Item".into(),
        entity_set: "Items".into(),
        generated_name: "Item".into(),
        lifecycle_states: Vec::new(),
        properties: vec![
            property("Amount", "Edm.Decimal", Some(serde_json::json!("1.00"))),
            property("Note", "Edm.String", None),
        ],
        actions: Vec::new(),
    };
    let first = compile_creation_contract(&entity, "schema", &serde_json::Map::new()).unwrap();
    let second = compile_creation_contract(
        &entity,
        "schema",
        &serde_json::json!({"Amount": 1.0, "Note": null})
            .as_object()
            .unwrap()
            .clone(),
    )
    .unwrap();
    assert_eq!(first.digest, second.digest);
    assert_eq!(first.fields[1].value_digest, first.fields[1].default_digest);
}

#[test]
fn decimal_exponents_and_guid_casing_have_one_canonical_contract() {
    let entity = ManifestEntityV1 {
        entity_type: "Test.Item".into(),
        entity_set: "Items".into(),
        generated_name: "Item".into(),
        lifecycle_states: Vec::new(),
        properties: vec![
            property("Amount", "Edm.Decimal", Some(serde_json::json!("+1.00e2"))),
            property(
                "OwnerId",
                "Edm.Guid",
                Some(serde_json::json!("AAAAAAAA-BBBB-CCCC-DDDD-EEEEEEEEEEEE")),
            ),
        ],
        actions: Vec::new(),
    };
    let first = compile_creation_contract(&entity, "schema", &serde_json::Map::new()).unwrap();
    let second = compile_creation_contract(
        &entity,
        "schema",
        serde_json::json!({
            "Amount": "100",
            "OwnerId": "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee"
        })
        .as_object()
        .unwrap(),
    )
    .unwrap();
    assert_eq!(first.digest, second.digest);
}

#[test]
fn forbidden_file_content_is_excluded_and_declared_key_signature_is_exact() {
    let mut id = property("Id", "Edm.Guid", None);
    id.source = ManifestValueSourceV1::EntityId;
    let mut content = property("Content", "Edm.Binary", None);
    content.write_policy = Some(ManifestPropertyWritePolicyV1 {
        create: ManifestCreateRoleV1::Forbidden,
        patch: ManifestPatchRoleV1::Writable,
    });
    let entity = ManifestEntityV1 {
        entity_type: "Test.File".into(),
        entity_set: "Files".into(),
        generated_name: "File".into(),
        lifecycle_states: Vec::new(),
        properties: vec![id, content],
        actions: Vec::new(),
    };
    let values = serde_json::json!({
        "Id": "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee",
        "Content": "protected"
    });
    let contract =
        compile_creation_contract(&entity, "schema", values.as_object().unwrap()).unwrap();
    assert_eq!(contract.fields.len(), 1);
    assert_eq!(contract.fields[0].name, "Id");

    let first = declared_key_signature(
        &[DeclaredKey {
            name: "Identity".into(),
            properties: vec!["Id".into()],
            entity_id: true,
        }],
        &contract,
    );
    let second = declared_key_signature(
        &[DeclaredKey {
            name: "Identity".into(),
            properties: vec!["Id".into()],
            entity_id: false,
        }],
        &contract,
    );
    assert_ne!(first, second);
}

#[test]
fn opaque_server_entity_id_is_recorded_under_narrow_csdl_key_type() {
    let mut id = property("Id", "Edm.Guid", None);
    id.source = ManifestValueSourceV1::EntityId;
    let entity = ManifestEntityV1 {
        entity_type: "Test.Item".into(),
        entity_set: "Items".into(),
        generated_name: "Item".into(),
        lifecycle_states: Vec::new(),
        properties: vec![id],
        actions: Vec::new(),
    };
    let values = serde_json::json!({"Id": "item-opaque-id"});

    let contract =
        compile_creation_contract(&entity, "schema", values.as_object().unwrap()).unwrap();

    assert_eq!(contract.fields.len(), 1);
    assert_eq!(contract.fields[0].type_descriptor, "Edm.Guid");
    assert_eq!(contract.fields[0].value_source, "entity_id");
    assert!(!contract.fields[0].value_digest.is_empty());
}

#[test]
fn legacy_actor_absence_is_reserved_for_required_schema_fields() {
    let mut required = property("OrderNumber", "Edm.String", None);
    required.nullable = false;
    required.write_policy = Some(ManifestPropertyWritePolicyV1 {
        create: ManifestCreateRoleV1::Required,
        patch: ManifestPatchRoleV1::Writable,
    });
    let entity = ManifestEntityV1 {
        entity_type: "Test.Order".into(),
        entity_set: "Orders".into(),
        generated_name: "Order".into(),
        lifecycle_states: Vec::new(),
        properties: vec![required],
        actions: Vec::new(),
    };
    let mut values = serde_json::Map::new();

    materialize_actor_creation_fields(&entity, &mut values);
    let absent = compile_creation_contract(&entity, "schema", &values).unwrap();
    values.insert("OrderNumber".into(), serde_json::json!("A-1"));
    let supplied = compile_creation_contract(&entity, "schema", &values).unwrap();

    assert_eq!(absent.fields.len(), 1);
    assert!(!absent.fields[0].nullable);
    assert_ne!(absent.digest, supplied.digest);
}
