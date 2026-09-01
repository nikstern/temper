use super::*;
use temper_wasm_sdk::data::{
    ManifestCreateRoleV1, ManifestPatchRoleV1, ManifestPropertyWritePolicyV1,
};

#[tokio::test]
async fn schema_validation_rejects_noncanonical_guid_before_dispatch() {
    let invocation = invocation(
        BTreeSet::from([DataOperationKind::EntityCreate]),
        SecurityContext::system(),
    );
    let response = call(
        &invocation,
        DataOperationV1::EntityCreate {
            entity_type: "Temper.Example.Customer".into(),
            value: serde_json::json!({"Id":"NOT-A-GUID"})
                .as_object()
                .cloned()
                .unwrap_or_default(),
        },
    )
    .await;
    assert_eq!(
        response_error(response).kind,
        ModuleDataErrorKind::SchemaMismatch
    );
}

#[tokio::test]
async fn handcrafted_create_and_patch_cannot_bypass_write_roles() {
    let mut invocation = invocation(
        BTreeSet::from([
            DataOperationKind::EntityCreate,
            DataOperationKind::EntityPatch,
        ]),
        SecurityContext::system(),
    );
    let entity = &mut std::sync::Arc::get_mut(&mut invocation)
        .expect("fixture invocation is unshared")
        .authority
        .binding
        .entities[0];
    for property in &mut entity.properties {
        property.write_policy = Some(match property.canonical_name.as_str() {
            "Id" => ManifestPropertyWritePolicyV1 {
                create: ManifestCreateRoleV1::Required,
                patch: ManifestPatchRoleV1::Forbidden,
            },
            "Name" => ManifestPropertyWritePolicyV1 {
                create: ManifestCreateRoleV1::Optional,
                patch: ManifestPatchRoleV1::Writable,
            },
            _ => ManifestPropertyWritePolicyV1 {
                create: ManifestCreateRoleV1::Forbidden,
                patch: ManifestPatchRoleV1::Forbidden,
            },
        });
    }

    let id = "018f1f80-7b2d-7000-8000-000000000031";
    for forbidden in [
        serde_json::json!({"Id": id, "Status": "Disabled"}),
        serde_json::json!({"Id": id, "RenameCount": 99}),
    ] {
        let response = call(
            &invocation,
            DataOperationV1::EntityCreate {
                entity_type: "Temper.Example.Customer".into(),
                value: forbidden.as_object().cloned().unwrap(),
            },
        )
        .await;
        assert_eq!(response_error(response).code, "ForbiddenCreateProperty");
    }

    let created = call(
        &invocation,
        DataOperationV1::EntityCreate {
            entity_type: "Temper.Example.Customer".into(),
            value: serde_json::json!({"Id": id, "Name": "Ada"})
                .as_object()
                .cloned()
                .unwrap(),
        },
    )
    .await;
    assert!(matches!(created.outcome, DataOutcomeV1::Ok { .. }));

    for forbidden in [
        serde_json::json!({"Id": id}),
        serde_json::json!({"Status": "Disabled"}),
        serde_json::json!({"RenameCount": 99}),
    ] {
        let response = call(
            &invocation,
            DataOperationV1::EntityPatch {
                entity_type: "Temper.Example.Customer".into(),
                entity_id: id.into(),
                expected_sequence: None,
                value: forbidden.as_object().cloned().unwrap(),
            },
        )
        .await;
        assert_eq!(response_error(response).code, "ForbiddenPatchProperty");
    }
}
