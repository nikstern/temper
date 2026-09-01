use super::*;

#[test]
fn canonical_grant_digest_ignores_input_set_order() {
    let mut first = ModuleDataGrant::default();
    first.operations.insert(DataOperationKind::EntityPatch);
    first.operations.insert(DataOperationKind::EntityGet);
    let mut second = ModuleDataGrant::default();
    second.operations.insert(DataOperationKind::EntityGet);
    second.operations.insert(DataOperationKind::EntityPatch);
    assert_eq!(first.digest().unwrap(), second.digest().unwrap());
}

#[test]
fn canonical_grant_digest_ignores_entity_declaration_order() {
    let entity = |name: &str| EntityDataGrant {
        entity_type: name.into(),
        ..EntityDataGrant::default()
    };
    let first = ModuleDataGrant {
        entities: vec![entity("Temper.B"), entity("Temper.A")],
        ..ModuleDataGrant::default()
    };
    let second = ModuleDataGrant {
        entities: vec![entity("Temper.A"), entity("Temper.B")],
        ..ModuleDataGrant::default()
    };
    assert_eq!(first.digest().unwrap(), second.digest().unwrap());
}

#[test]
fn sequence_order_grant_is_wire_absent_until_enabled() {
    let mut entity = EntityDataGrant {
        entity_type: "Temper.Task".into(),
        ..EntityDataGrant::default()
    };
    let disabled = serde_json::to_value(&entity).unwrap();
    assert!(disabled.get("query_order_by_sequence").is_none());

    entity.query_order_by_sequence = true;
    let enabled = serde_json::to_value(&entity).unwrap();
    assert_eq!(enabled["query_order_by_sequence"], serde_json::json!(true));
}

#[test]
fn duplicate_entity_grants_fail() {
    let grant = ModuleDataGrant {
        entities: vec![
            EntityDataGrant {
                entity_type: "Temper.Task".into(),
                ..EntityDataGrant::default()
            },
            EntityDataGrant {
                entity_type: "Temper.Task".into(),
                ..EntityDataGrant::default()
            },
        ],
        ..ModuleDataGrant::default()
    };
    assert!(grant.validate().is_err());
}

#[test]
fn missing_operation_denies_even_when_entity_exists() {
    let grant = ModuleDataGrant {
        entities: vec![EntityDataGrant {
            entity_type: "Temper.Task".into(),
            ..EntityDataGrant::default()
        }],
        ..ModuleDataGrant::default()
    };
    assert!(!grant.permits(DataOperationKind::EntityGet, "Temper.Task", None));
}

#[test]
fn file_metadata_reads_require_the_exact_file_capability() {
    let mut grant = ModuleDataGrant {
        operations: BTreeSet::from([DataOperationKind::EntityGet, DataOperationKind::EntityQuery]),
        entities: vec![EntityDataGrant {
            entity_type: "Temper.FileSystem.File".into(),
            ..EntityDataGrant::default()
        }],
        ..ModuleDataGrant::default()
    };

    assert!(!grant.permits(DataOperationKind::EntityGet, "Temper.FileSystem.File", None));
    assert!(!grant.permits(
        DataOperationKind::EntityQuery,
        "Temper.FileSystem.File",
        None
    ));

    grant.entities[0]
        .file_operations
        .insert(FileOperationKind::MetadataRead);
    assert!(grant.permits(DataOperationKind::EntityGet, "Temper.FileSystem.File", None));
    assert!(grant.permits(
        DataOperationKind::EntityQuery,
        "Temper.FileSystem.File",
        None
    ));
}

#[test]
fn semantic_hashes_change_when_a_used_property_changes() {
    let entity = |properties| ManifestEntityV1 {
        entity_type: "Temper.Task".into(),
        entity_set: "Tasks".into(),
        generated_name: "Task".into(),
        lifecycle_states: Vec::new(),
        properties,
        actions: Vec::new(),
    };
    let property = |name: &str, nullable| ManifestPropertyV1 {
        canonical_name: name.into(),
        generated_name: name.to_lowercase(),
        type_name: "Edm.String".into(),
        nullable,
        source: ManifestValueSourceV1::StoredField,
        default_value: None,
        enum_members: Vec::new(),
        write_policy: Some(ManifestPropertyWritePolicyV1 {
            create: ManifestCreateRoleV1::Required,
            patch: ManifestPatchRoleV1::Writable,
        }),
    };
    let manifest = |properties| {
        ModuleSdkManifest::new(
            "worker",
            ModuleSdkMetadataDigests {
                closure: "closure".into(),
                dependency_lock: "closure".into(),
                schema: "schema".into(),
            },
            "artifact",
            ModuleDataGrant::default(),
            vec![entity(properties)],
            BTreeSet::new(),
        )
        .unwrap()
    };
    let old = manifest(vec![property("Id", false)]);
    let changed = manifest(vec![property("Id", true)]);
    assert_ne!(
        old.used_symbol_hashes().unwrap(),
        changed.used_symbol_hashes().unwrap()
    );

    let mut defaulted_property = property("Id", false);
    defaulted_property.default_value = Some(serde_json::json!("fallback"));
    let defaulted = manifest(vec![defaulted_property]);
    assert_ne!(
        old.used_symbol_hashes().unwrap(),
        defaulted.used_symbol_hashes().unwrap()
    );

    let mut lifecycle_property = property("Id", false);
    lifecycle_property.source = ManifestValueSourceV1::LifecycleStatus;
    let lifecycle = manifest(vec![lifecycle_property]);
    assert_ne!(
        old.used_symbol_hashes().unwrap(),
        lifecycle.used_symbol_hashes().unwrap()
    );

    let mut host_owned_property = property("Id", false);
    host_owned_property.write_policy = Some(ManifestPropertyWritePolicyV1 {
        create: ManifestCreateRoleV1::Forbidden,
        patch: ManifestPatchRoleV1::Forbidden,
    });
    let host_owned = manifest(vec![host_owned_property]);
    assert_ne!(
        old.used_symbol_hashes().unwrap(),
        host_owned.used_symbol_hashes().unwrap()
    );
}

#[test]
fn empty_lifecycle_states_preserve_the_frozen_v1_entity_hash() {
    let manifest = ModuleSdkManifest::new(
        "worker",
        ModuleSdkMetadataDigests {
            closure: "closure".into(),
            dependency_lock: "closure".into(),
            schema: "schema".into(),
        },
        "artifact",
        ModuleDataGrant::default(),
        vec![ManifestEntityV1 {
            entity_type: "Temper.Task".into(),
            entity_set: "Tasks".into(),
            generated_name: "Task".into(),
            lifecycle_states: Vec::new(),
            properties: Vec::new(),
            actions: Vec::new(),
        }],
        BTreeSet::new(),
    )
    .unwrap();

    assert_eq!(
        manifest.used_symbol_hashes().unwrap()["entity:Temper.Task"],
        "a216c7aa5562d11723b59b041ec5c5961777480b9db158dd6bd5a1ff432e4cb2"
    );
}

#[test]
fn property_metadata_without_source_fails_closed() {
    let property = serde_json::from_value::<ManifestPropertyV1>(serde_json::json!({
        "canonical_name": "Id",
        "generated_name": "id",
        "type_name": "Edm.String",
        "nullable": false,
        "enum_members": []
    }));
    assert!(property.is_err());
}

fn manifest_with_action_parameter(nullable: bool) -> ModuleSdkManifest {
    let parameter = ManifestPropertyV1 {
        canonical_name: "ReasonCode".into(),
        generated_name: "reason_code".into(),
        type_name: "Edm.String".into(),
        nullable,
        source: ManifestValueSourceV1::Input,
        default_value: None,
        enum_members: Vec::new(),
        write_policy: None,
    };
    let entity = ManifestEntityV1 {
        entity_type: "Temper.Task".into(),
        entity_set: "Tasks".into(),
        generated_name: "Task".into(),
        lifecycle_states: Vec::new(),
        properties: Vec::new(),
        actions: vec![ManifestActionV1 {
            canonical_name: "Close".into(),
            generated_name: "close".into(),
            parameters: vec![parameter],
            result_type: None,
            result_enum_members: Vec::new(),
            result_cardinality: Some(ManifestActionResultCardinalityV1::Void),
            composite: false,
        }],
    };
    ModuleSdkManifest::new(
        "worker",
        ModuleSdkMetadataDigests {
            closure: "closure".into(),
            dependency_lock: "closure".into(),
            schema: "schema".into(),
        },
        "artifact",
        ModuleDataGrant::default(),
        vec![entity],
        BTreeSet::new(),
    )
    .unwrap()
}

#[test]
fn required_to_nullable_action_parameter_is_a_compatible_widening() {
    let prior = manifest_with_action_parameter(false);
    let candidate = manifest_with_action_parameter(true);
    assert_eq!(
        prior
            .compatible_action_nullability_widenings(&candidate)
            .unwrap(),
        BTreeSet::from(["action:Temper.Task:Close".to_string()])
    );
}

#[test]
fn nullable_to_required_action_parameter_names_the_breaking_narrowing() {
    let prior = manifest_with_action_parameter(true);
    let candidate = manifest_with_action_parameter(false);
    let error = prior
        .compatible_action_nullability_widenings(&candidate)
        .unwrap_err();
    assert!(error.contains("entity='Temper.Task'"));
    assert!(error.contains("action='Close'"));
    assert!(error.contains("parameter='ReasonCode'"));
    assert!(error.contains("old_nullable=true new_nullable=false"));
}

#[test]
fn action_result_cardinality_changes_semantic_hash() {
    let required = manifest_with_action_parameter(false);
    let mut nullable = required.clone();
    nullable.entities[0].actions[0].result_type = Some("Edm.String".into());
    nullable.entities[0].actions[0].result_cardinality =
        Some(ManifestActionResultCardinalityV1::Nullable);
    let mut required_result = nullable.clone();
    required_result.entities[0].actions[0].result_cardinality =
        Some(ManifestActionResultCardinalityV1::Required);

    assert_ne!(
        nullable.used_symbol_hashes().unwrap(),
        required_result.used_symbol_hashes().unwrap()
    );
}

#[test]
fn authenticated_historical_manifest_preserves_legacy_json_and_digest() {
    let mut historical = manifest_with_action_parameter(false);
    historical.contract_version = None;
    for entity in &mut historical.entities {
        for property in &mut entity.properties {
            property.write_policy = None;
        }
        for action in &mut entity.actions {
            action.result_cardinality = None;
        }
    }
    let json = serde_json::to_string(&historical).unwrap();
    assert!(!json.contains("contract_version"));
    assert!(!json.contains("write_policy"));
    assert!(!json.contains("result_cardinality"));

    let restored: ModuleSdkManifest = serde_json::from_str(&json).unwrap();
    assert_eq!(
        restored.binding_digest().unwrap(),
        historical.binding_digest().unwrap()
    );
    restored.verify_binding().unwrap();
    assert!(restored.verify_current_binding().is_err());
}
