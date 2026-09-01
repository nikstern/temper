use super::*;
use temper_wasm_sdk::data::{
    DataOperationKind, EntityDataGrant, ManifestActionV1, ManifestEntityV1, ManifestPropertyV1,
    ManifestValueSourceV1, ModuleSdkCompatibilityProof, ModuleSdkMetadataDigests,
    bind_module_sdk_artifact,
};

fn action_grant() -> ModuleDataGrant {
    ModuleDataGrant {
        operations: [DataOperationKind::ActionInvoke].into_iter().collect(),
        entities: vec![EntityDataGrant {
            entity_type: "Temper.App.Task".into(),
            actions: ["Close".to_string()].into_iter().collect(),
            ..EntityDataGrant::default()
        }],
        ..ModuleDataGrant::default()
    }
}

fn action_manifest(nullable: bool, closure: &str) -> ModuleSdkManifest {
    ModuleSdkManifest::new(
        "worker",
        ModuleSdkMetadataDigests {
            closure: closure.into(),
            dependency_lock: closure.into(),
            schema: format!("schema-{nullable}"),
        },
        "artifact",
        action_grant(),
        vec![ManifestEntityV1 {
            entity_type: "Temper.App.Task".into(),
            entity_set: "Tasks".into(),
            generated_name: "Task".into(),
            lifecycle_states: Vec::new(),
            properties: Vec::new(),
            actions: vec![ManifestActionV1 {
                canonical_name: "Close".into(),
                generated_name: "close".into(),
                parameters: vec![ManifestPropertyV1 {
                    canonical_name: "Reason".into(),
                    generated_name: "reason".into(),
                    type_name: "Edm.String".into(),
                    nullable,
                    source: ManifestValueSourceV1::Input,
                    default_value: None,
                    enum_members: Vec::new(),
                }],
                result_type: None,
                result_enum_members: Vec::new(),
                composite: false,
            }],
        }],
        ["Close".to_string()].into_iter().collect(),
    )
    .unwrap()
}

fn bound_wasm(manifest: &ModuleSdkManifest) -> Vec<u8> {
    bind_module_sdk_artifact(
        b"\0asm\x01\0\0\0",
        &ArtifactModuleSdkBinding::from_manifest(manifest).unwrap(),
    )
    .unwrap()
}

#[test]
fn global_binding_gate_names_nullable_to_required_narrowing_without_a_proof() {
    let prior = action_manifest(true, "prior");
    let candidate = action_manifest(false, "candidate");
    let error = verify_module_data_binding(
        &bound_wasm(&prior),
        "worker",
        &action_grant(),
        &prior,
        &candidate,
    )
    .unwrap_err();
    assert!(error.contains("entity='Temper.App.Task'"));
    assert!(error.contains("action='Close'"));
    assert!(error.contains("parameter='Reason'"));
    assert!(error.contains("old_nullable=true new_nullable=false"));
}

#[test]
fn global_binding_gate_accepts_required_to_nullable_with_valid_proof() {
    let mut prior = action_manifest(false, "prior");
    let candidate = action_manifest(true, "candidate");
    prior.compatibility_proof = Some(ModuleSdkCompatibilityProof {
        prior_closure_digest: prior.closure_digest.clone(),
        candidate_closure_digest: candidate.closure_digest.clone(),
        prior_used_symbol_hashes: prior.used_symbol_hashes().unwrap(),
        candidate_used_symbol_hashes: candidate.used_symbol_hashes().unwrap(),
        prior_grant_digest: prior.grant_digest.clone(),
        candidate_grant_digest: candidate.grant_digest.clone(),
    });
    assert!(
        verify_module_data_binding(
            &bound_wasm(&prior),
            "worker",
            &action_grant(),
            &prior,
            &candidate,
        )
        .is_ok()
    );
}
