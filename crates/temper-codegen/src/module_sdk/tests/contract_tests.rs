use super::*;

#[test]
fn lifecycle_wire_values_are_escaped_and_invalid_variants_fail_closed() {
    let csdl = parse_csdl(
        r#"<edmx:Edmx Version="4.0" xmlns:edmx="http://docs.oasis-open.org/odata/ns/edmx"><edmx:DataServices><Schema Namespace="Example" xmlns="http://docs.oasis-open.org/odata/ns/edm"><EntityType Name="Task"><Key><PropertyRef Name="Id"/></Key><Property Name="Id" Type="Edm.String" Nullable="false"/><Property Name="Status" Type="Edm.String" Nullable="false"/></EntityType><EntityContainer Name="Container"><EntitySet Name="Tasks" EntityType="Example.Task"/></EntityContainer></Schema></edmx:DataServices></edmx:Edmx>"#,
    )
    .unwrap();
    let grant = ModuleDataGrant {
        operations: [DataOperationKind::EntityGet].into_iter().collect(),
        entities: vec![EntityDataGrant {
            entity_type: "Example.Task".into(),
            ..EntityDataGrant::default()
        }],
        ..ModuleDataGrant::default()
    };
    let linked = CanonicalSpecModel::link_v2_sources(
        &csdl,
        &[IoaSourceInput {
            entity_type: "Example.Task".into(),
            source: r#"[automaton]
name = "Task"
states = ['open"quote', 'closed\path']
initial = 'open"quote'
lifecycle_property = "Status"
"#
            .into(),
        }],
    )
    .unwrap();
    let generated = super::super::generate_module_sdk(
        &linked,
        "worker",
        "closure",
        "lock",
        "artifact",
        grant.clone(),
    )
    .unwrap();
    assert!(
        generated
            .source
            .contains(r##"#[serde(rename = "open\"quote")]"##)
    );
    assert!(
        generated
            .source
            .contains(r##"#[serde(rename = "closed\\path")]"##)
    );

    let invalid = CanonicalSpecModel::link_v2_sources(
        &csdl,
        &[IoaSourceInput {
            entity_type: "Example.Task".into(),
            source: r#"[automaton]
name = "Task"
states = ["1-open"]
initial = "1-open"
lifecycle_property = "Status"
"#
            .into(),
        }],
    )
    .unwrap();
    assert!(matches!(
        super::super::generate_module_sdk(&invalid, "worker", "closure", "lock", "artifact", grant),
        Err(ModuleSdkCodegenError::IdentifierCollision(_))
    ));
}

#[test]
fn record_annotations_have_a_canonical_schema_digest() {
    let source = CSDL.replace(
        "</EntityType>",
        "<Annotation Term=\"Example.Metadata\"><Record><PropertyValue Property=\"Zulu\" String=\"last\"/><PropertyValue Property=\"Alpha\" String=\"first\"/></Record></Annotation></EntityType>",
    );
    let mut generated = Vec::new();
    for _ in 0..16 {
        let csdl = parse_csdl(&source).unwrap();
        let sdk = generate_module_sdk(&csdl, "worker", "closure", "closure", "artifact", grant())
            .unwrap();
        generated.push((sdk.source, sdk.manifest.schema_digest));
    }
    assert!(generated.windows(2).all(|pair| pair[0] == pair[1]));
}

#[test]
fn cross_entity_action_results_fail_closed() {
    let csdl = parse_csdl(CSDL).unwrap();
    let mut invalid = grant();
    invalid.entities[0].actions.insert("IssueReceipt".into());
    assert!(matches!(
        generate_module_sdk(&csdl, "worker", "closure", "closure", "artifact", invalid),
        Err(ModuleSdkCodegenError::UnsupportedEntityResult { action, entity_type, result_type })
            if action == "IssueReceipt" && entity_type == "Temper.App.Task" && result_type == "Temper.App.Receipt"
    ));
}

#[test]
fn generated_names_preserve_word_boundaries_and_escape_keywords() {
    assert_eq!(rust_field_name("CreatedAt"), "created_at");
    assert_eq!(rust_field_name("created_at"), "created_at");
    assert_eq!(rust_field_name("type"), "type_");
    assert_eq!(rust_field_name("gen"), "gen_");
}

#[test]
fn commit_sequence_property_cannot_collide_with_host_order_helper() {
    let csdl = parse_csdl(&CSDL.replace(
        "<Property Name=\"Status\"",
        "<Property Name=\"CommitSequence\" Type=\"Edm.Int64\" Nullable=\"false\"/><Property Name=\"Status\"",
    ))
    .unwrap();
    let mut invalid = grant();
    invalid.entities[0]
        .query_order_fields
        .insert("CommitSequence".into());
    invalid.entities[0].query_order_by_sequence = true;
    assert!(matches!(
        generate_module_sdk(&csdl, "worker", "closure", "closure", "artifact", invalid),
        Err(ModuleSdkCodegenError::IdentifierCollision(message)) if message.contains("commit_sequence")
    ));
}

#[test]
fn methods_are_not_emitted_without_global_operation_grants() {
    let csdl = parse_csdl(CSDL).unwrap();
    let mut scoped = grant();
    scoped.operations.remove(&DataOperationKind::EntityGet);
    scoped.operations.remove(&DataOperationKind::ActionInvoke);
    let generated =
        generate_module_sdk(&csdl, "worker", "closure", "closure", "artifact", scoped).unwrap();
    assert!(!generated.source.contains("pub fn get("));
    assert!(!generated.source.contains("pub fn start_work("));
}

#[test]
fn unknown_granted_symbol_fails_closed() {
    let csdl = parse_csdl(CSDL).unwrap();
    let mut invalid = grant();
    invalid.entities[0]
        .actions
        .insert("DeleteEverything".into());
    assert!(matches!(
        generate_module_sdk(&csdl, "worker", "closure", "closure", "artifact", invalid),
        Err(ModuleSdkCodegenError::MissingSymbol { .. })
    ));
}

#[test]
fn packaging_binds_manifest_into_exact_artifact() {
    let csdl = parse_csdl(CSDL).unwrap();
    let generated =
        generate_module_sdk(&csdl, "worker", "closure", "closure", "", grant()).unwrap();
    let packaged = package_generated_module_sdk(b"\0asm\x01\0\0\0", generated).unwrap();
    assert_eq!(
        packaged.manifest.artifact_digest,
        hex_sha256(&packaged.wasm)
    );
    let embedded = temper_wasm_sdk::data::read_module_sdk_artifact_binding(&packaged.wasm)
        .unwrap()
        .unwrap();
    assert_eq!(embedded.module_name, "worker");
    assert_eq!(embedded.grant_digest, packaged.manifest.grant_digest);
}
