//! Exact artifact-carried generated-client binding verification.

use super::*;

fn regenerated_comparison_view(
    supplied: &temper_wasm_sdk::data::ModuleSdkManifest,
    regenerated: &temper_wasm_sdk::data::ModuleSdkManifest,
) -> temper_wasm_sdk::data::ModuleSdkManifest {
    let mut comparison = regenerated.clone();
    comparison.compatibility_proof = None;
    if supplied.contract_version.is_none() {
        comparison.contract_version = None;
        for entity in &mut comparison.entities {
            for property in &mut entity.properties {
                property.write_policy = None;
            }
            for action in &mut entity.actions {
                action.result_cardinality = None;
            }
        }
    }
    comparison
}

fn binding_matches_regenerated(
    wasm: &[u8],
    module_name: &str,
    supplied: &temper_wasm_sdk::data::ModuleSdkManifest,
    regenerated: &temper_wasm_sdk::data::ModuleSdkManifest,
) -> Result<(), String> {
    use temper_wasm_sdk::data::{ArtifactModuleSdkBinding, read_module_sdk_artifact_binding};

    supplied.verify_binding()?;
    if supplied.module_name != module_name {
        return Err("module data binding name mismatch".into());
    }
    let embedded = read_module_sdk_artifact_binding(wasm)?
        .ok_or_else(|| "module artifact has no SDK binding custom section".to_string())?;
    if embedded != ArtifactModuleSdkBinding::from_manifest(supplied)? {
        return Err("module SDK sidecar is not carried by the loaded artifact".into());
    }
    let mut supplied_without_proof = supplied.clone();
    supplied_without_proof.compatibility_proof = None;
    let regenerated_without_proof = regenerated_comparison_view(supplied, regenerated);
    if supplied_without_proof == regenerated_without_proof {
        return Ok(());
    }
    let prior_hashes = supplied.used_symbol_hashes()?;
    let candidate_hashes = regenerated_without_proof.used_symbol_hashes()?;
    let compatible_nullability_changes =
        supplied.compatible_action_nullability_widenings(&regenerated_without_proof)?;
    let proof = supplied
        .compatibility_proof
        .as_ref()
        .ok_or_else(|| "module data binding differs without an artifact-bound proof".to_string())?;
    if proof.prior_closure_digest != supplied.closure_digest
        || proof.candidate_closure_digest != regenerated.closure_digest
        || proof.prior_grant_digest != supplied.grant_digest
        || proof.candidate_grant_digest != regenerated.grant_digest
        || proof.prior_used_symbol_hashes != prior_hashes
        || proof.candidate_used_symbol_hashes != candidate_hashes
        || prior_hashes.iter().any(|(symbol, hash)| {
            candidate_hashes.get(symbol) != Some(hash)
                && !compatible_nullability_changes.contains(symbol)
        })
        || regenerated.grant != supplied.grant
    {
        return Err("module data compatibility proof failed host recomputation".into());
    }
    Ok(())
}

pub(super) async fn verify_scoped_module_data_bindings(
    state: &ServerState,
    record: &SchemaDeploymentRecord,
) -> Result<bool, ServiceError> {
    if record.bundle.wasm_module_data_bindings.is_empty() {
        return Ok(true);
    }
    let csdl = temper_spec::parse_csdl(&record.bundle.canonical_csdl)
        .map_err(|error| ServiceError::new("verification_failed", error.to_string(), false))?;
    let ioa = record
        .bundle
        .canonical_ioa
        .iter()
        .map(
            |(entity_type, source)| temper_spec::bundle::IoaSourceInput {
                entity_type: entity_type.clone(),
                source: source.clone(),
            },
        )
        .collect::<Vec<_>>();
    let closure_digest = temper_spec::bundle::scoped_module_data_closure_digest_with_version(
        &record.bundle.canonical_csdl,
        ioa.clone(),
        &record.bundle.canonicalization_version,
    )
    .map_err(|error| ServiceError::new("verification_failed", error.to_string(), false))?;
    let canonical_model = match record.bundle.canonicalization_version.as_str() {
        temper_spec::bundle::SCOPED_SPEC_BUNDLE_CONTRACT_V2 => Some(
            temper_spec::CanonicalSpecModel::link_v2_sources(&csdl, &ioa).map_err(|error| {
                ServiceError::new("verification_failed", error.to_string(), false)
            })?,
        ),
        temper_spec::bundle::SCOPED_SPEC_BUNDLE_CONTRACT_V1 => None,
        version => {
            return Err(ServiceError::new(
                "verification_failed",
                format!("unsupported canonicalization version '{version}'"),
                false,
            ));
        }
    };
    let tenant = TenantId::new(&record.bundle.tenant);
    for (module_name, stored) in &record.bundle.wasm_module_data_bindings {
        let Some(artifact_digest) = record.bundle.wasm_module_digests.get(module_name) else {
            return Ok(false);
        };
        let Some(artifact_hash) = artifact_digest.strip_prefix("sha256:") else {
            return Ok(false);
        };
        let supplied: temper_wasm_sdk::data::ModuleSdkManifest =
            serde_json::from_str(&stored.canonical_manifest_json).map_err(|error| {
                ServiceError::new("verification_failed", error.to_string(), false)
            })?;
        let actual_binding_digest = supplied
            .binding_digest()
            .map(|digest| format!("sha256:{digest}"))
            .map_err(|error| ServiceError::new("verification_failed", error, false))?;
        if stored.binding_digest != actual_binding_digest
            || supplied.artifact_digest != artifact_hash
        {
            return Ok(false);
        }
        let regenerated = if let Some(model) = &canonical_model {
            temper_codegen::generate_module_sdk(
                model,
                module_name,
                &closure_digest,
                &closure_digest,
                artifact_hash,
                supplied.grant.clone(),
            )
        } else {
            temper_codegen::generate_module_sdk_v1(
                &csdl,
                &ioa,
                module_name,
                &closure_digest,
                &closure_digest,
                artifact_hash,
                supplied.grant.clone(),
            )
        }
        .map_err(|error| ServiceError::new("verification_failed", error.to_string(), false))?;
        let wasm = state
            .load_scoped_wasm_artifact_bytes(&tenant, module_name, artifact_hash)
            .await
            .map_err(|error| ServiceError::new("backend_unavailable", error, true))?;
        binding_matches_regenerated(&wasm, module_name, &supplied, &regenerated.manifest)
            .map_err(|error| ServiceError::new("verification_failed", error, false))?;
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use temper_wasm_sdk::data::{
        ArtifactModuleSdkBinding, DataOperationKind, EntityDataGrant,
        ManifestActionResultCardinalityV1, ManifestActionV1, ManifestEntityV1, ManifestPropertyV1,
        ManifestValueSourceV1, ModuleDataGrant, ModuleSdkCompatibilityProof, ModuleSdkManifest,
        ModuleSdkMetadataDigests, bind_module_sdk_artifact,
    };

    fn manifest(nullable: bool) -> ModuleSdkManifest {
        let grant = ModuleDataGrant {
            operations: [DataOperationKind::ActionInvoke].into_iter().collect(),
            entities: vec![EntityDataGrant {
                entity_type: "Temper.Task".into(),
                actions: ["Close".to_string()].into_iter().collect(),
                ..EntityDataGrant::default()
            }],
            ..ModuleDataGrant::default()
        };
        ModuleSdkManifest::new(
            "worker",
            ModuleSdkMetadataDigests {
                closure: format!("closure-{nullable}"),
                dependency_lock: format!("closure-{nullable}"),
                schema: format!("schema-{nullable}"),
            },
            "artifact",
            grant,
            vec![ManifestEntityV1 {
                entity_type: "Temper.Task".into(),
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
                        write_policy: None,
                    }],
                    result_type: None,
                    result_enum_members: Vec::new(),
                    result_cardinality: Some(ManifestActionResultCardinalityV1::Void),
                    composite: false,
                }],
            }],
            ["Close".to_string()].into_iter().collect(),
        )
        .unwrap()
    }

    fn historical_view(mut manifest: ModuleSdkManifest) -> ModuleSdkManifest {
        manifest.contract_version = None;
        for entity in &mut manifest.entities {
            for property in &mut entity.properties {
                property.write_policy = None;
            }
            for action in &mut entity.actions {
                action.result_cardinality = None;
            }
        }
        manifest
    }

    #[test]
    fn scoped_binding_gate_preserves_parameter_qualified_narrowing_error() {
        let prior = manifest(true);
        let candidate = manifest(false);
        let wasm = bind_module_sdk_artifact(
            b"\0asm\x01\0\0\0",
            &ArtifactModuleSdkBinding::from_manifest(&prior).unwrap(),
        )
        .unwrap();
        let error = binding_matches_regenerated(&wasm, "worker", &prior, &candidate).unwrap_err();
        assert!(error.contains("entity='Temper.Task'"));
        assert!(error.contains("action='Close'"));
        assert!(error.contains("parameter='Reason'"));
        assert!(error.contains("old_nullable=true new_nullable=false"));
    }

    #[test]
    fn scoped_restart_accepts_authenticated_historical_manifest_shape() {
        let candidate = manifest(false);
        let historical = historical_view(candidate.clone());
        let wasm = bind_module_sdk_artifact(
            b"\0asm\x01\0\0\0",
            &ArtifactModuleSdkBinding::from_manifest(&historical).unwrap(),
        )
        .unwrap();

        binding_matches_regenerated(&wasm, "worker", &historical, &candidate).unwrap();
    }

    #[test]
    fn scoped_restart_recomputes_historical_compatibility_proof_in_legacy_view() {
        let candidate = manifest(true);
        let historical_candidate = historical_view(candidate.clone());
        let mut historical_prior = historical_view(manifest(false));
        historical_prior.compatibility_proof = Some(ModuleSdkCompatibilityProof {
            prior_closure_digest: historical_prior.closure_digest.clone(),
            candidate_closure_digest: candidate.closure_digest.clone(),
            prior_used_symbol_hashes: historical_prior.used_symbol_hashes().unwrap(),
            candidate_used_symbol_hashes: historical_candidate.used_symbol_hashes().unwrap(),
            prior_grant_digest: historical_prior.grant_digest.clone(),
            candidate_grant_digest: candidate.grant_digest.clone(),
        });
        let wasm = bind_module_sdk_artifact(
            b"\0asm\x01\0\0\0",
            &ArtifactModuleSdkBinding::from_manifest(&historical_prior).unwrap(),
        )
        .unwrap();

        binding_matches_regenerated(&wasm, "worker", &historical_prior, &candidate).unwrap();
    }
}
