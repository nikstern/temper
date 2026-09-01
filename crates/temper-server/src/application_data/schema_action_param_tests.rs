use super::*;
use temper_wasm_sdk::data::ManifestActionV1;

fn csdl() -> temper_spec::csdl::CsdlDocument {
    temper_spec::parse_csdl(
        r#"<?xml version="1.0"?><edmx:Edmx Version="4.0" xmlns:edmx="http://docs.oasis-open.org/odata/ns/edmx"><edmx:DataServices><Schema Namespace="Temper" xmlns="http://docs.oasis-open.org/odata/ns/edm"><EntityType Name="Task"></EntityType><EntityType Name="User"></EntityType><EnumType Name="Phase"><Member Name="Open"/><Member Name="Closed"/></EnumType><ComplexType Name="Payload"><Property Name="Value" Type="Edm.String"/></ComplexType></Schema></edmx:DataServices></edmx:Edmx>"#,
    )
    .expect("test CSDL")
}

fn entity() -> ManifestEntityV1 {
    ManifestEntityV1 {
        entity_type: "Temper.Task".into(),
        entity_set: "Tasks".into(),
        generated_name: "Task".into(),
        lifecycle_states: Vec::new(),
        properties: Vec::new(),
        actions: vec![ManifestActionV1 {
            canonical_name: "Close".into(),
            generated_name: "close".into(),
            parameters: vec![
                ManifestPropertyV1 {
                    canonical_name: "ReasonCode".into(),
                    generated_name: "reason_code".into(),
                    type_name: "Edm.String".into(),
                    nullable: false,
                    source: ManifestValueSourceV1::Input,
                    default_value: None,
                    enum_members: Vec::new(),
                    write_policy: None,
                },
                ManifestPropertyV1 {
                    canonical_name: "Payload".into(),
                    generated_name: "payload".into(),
                    type_name: "Temper.Payload".into(),
                    nullable: true,
                    source: ManifestValueSourceV1::Input,
                    default_value: None,
                    enum_members: Vec::new(),
                    write_policy: None,
                },
                ManifestPropertyV1 {
                    canonical_name: "Phase".into(),
                    generated_name: "phase".into(),
                    type_name: "Temper.Phase".into(),
                    nullable: true,
                    source: ManifestValueSourceV1::Input,
                    default_value: None,
                    enum_members: vec!["Open".into(), "Closed".into()],
                    write_policy: None,
                },
                ManifestPropertyV1 {
                    canonical_name: "Owner".into(),
                    generated_name: "owner".into(),
                    type_name: "Temper.User".into(),
                    nullable: true,
                    source: ManifestValueSourceV1::Input,
                    default_value: None,
                    enum_members: Vec::new(),
                    write_policy: None,
                },
            ],
            result_type: None,
            result_enum_members: Vec::new(),
            result_cardinality: None,
            composite: false,
        }],
    }
}

#[test]
fn module_action_missing_and_null_required_values_share_the_stable_code() {
    for params in [
        serde_json::json!({}),
        serde_json::json!({"ReasonCode": null}),
    ] {
        let entity = entity();
        let error =
            validate_manifest_action_params(&csdl(), &entity, "Close", params.as_object().unwrap())
                .unwrap_err();
        assert_eq!(error.kind, ModuleDataErrorKind::SchemaMismatch);
        assert_eq!(error.code, "MissingActionParameter");
    }
}

#[test]
fn module_action_aliases_are_accepted_and_extras_use_type_mismatch() {
    let entity = entity();
    validate_manifest_action_params(
        &csdl(),
        &entity,
        "Close",
        serde_json::json!({"reason_code": "done"})
            .as_object()
            .unwrap(),
    )
    .unwrap();
    let error = validate_manifest_action_params(
        &csdl(),
        &entity,
        "Close",
        serde_json::json!({"ReasonCode": "done", "Other": true})
            .as_object()
            .unwrap(),
    )
    .unwrap_err();
    assert_eq!(error.code, "ActionParameterTypeMismatch");
}

#[test]
fn module_action_rejects_unknown_enum_and_wrong_reference_shape() {
    let entity = entity();
    for params in [
        serde_json::json!({"ReasonCode": "done", "Phase": "Unknown"}),
        serde_json::json!({"ReasonCode": "done", "Owner": {"Id": "user-1"}}),
        serde_json::json!({"ReasonCode": "done", "Payload": "not-an-object"}),
    ] {
        let result =
            validate_manifest_action_params(&csdl(), &entity, "Close", params.as_object().unwrap());
        let error = match result {
            Ok(()) => panic!("invalid params unexpectedly accepted: {params}"),
            Err(error) => error,
        };
        assert_eq!(error.code, "ActionParameterTypeMismatch");
    }
    validate_manifest_action_params(
        &csdl(),
        &entity,
        "Close",
        serde_json::json!({
            "ReasonCode": "done",
            "Phase": "Open",
            "Owner": "user-1",
            "Payload": {"Value": "ok"}
        })
        .as_object()
        .unwrap(),
    )
    .unwrap();
}
