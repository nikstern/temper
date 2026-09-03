use super::*;

/// Seed an active bundle encoded with the historical data-binding contract.
pub(super) async fn seed_active_historical_bundle(
    store: &temper_store_turso::TursoEventStore,
    compiled: &ScopedSpecBundle,
    scope: &SchemaScope,
    manifest: &temper_wasm_sdk::data::ModuleSdkManifest,
    budgets: &SchemaBundleBudgetsV1,
) {
    let digest = compiled.digest().to_string();
    let binding_digest = manifest
        .binding_digest()
        .map(|value| format!("sha256:{value}"))
        .expect("historical binding digest");
    store
        .submit_schema_bundle(SubmitSchemaBundle {
            bundle: SchemaBundleRecord {
                tenant: TenantId::default().to_string(),
                scope: scope.clone(),
                digest: digest.clone(),
                predecessor_digest: None,
                canonicalization_version: compiled.canonicalization_version().to_string(),
                canonical_csdl: compiled.canonical_csdl().to_string(),
                canonical_ioa: compiled
                    .ioa_specs()
                    .iter()
                    .map(|spec| (spec.entity_type.clone(), spec.canonical_source.clone()))
                    .collect(),
                cedar_policies: std::collections::BTreeMap::new(),
                wasm_module_digests: std::collections::BTreeMap::from([(
                    MODULE_NAME.into(),
                    format!("sha256:{}", manifest.artifact_digest),
                )]),
                wasm_module_data_bindings: std::collections::BTreeMap::from([(
                    MODULE_NAME.into(),
                    ScopedModuleDataBinding {
                        binding_digest,
                        canonical_manifest_json: serde_json::to_string(manifest)
                            .expect("historical binding encodes"),
                    },
                )]),
                migration_module_name: None,
                migration_module_digest: None,
                migration_abi_version: None,
                canonical_budgets: serde_json::to_string(budgets).expect("budgets encode"),
            },
            idempotency_key: "historical-v1-submit".into(),
            request_digest: format!("sha256:{}", "1".repeat(64)),
            request_id: "historical-v1-submit".into(),
        })
        .await
        .expect("historical bundle persists");
    let claimed = store
        .claim_schema_verification(ClaimSchemaVerification {
            tenant: TenantId::default().to_string(),
            scope: scope.clone(),
            bundle_digest: digest.clone(),
            logical_now: 1,
            lease_expires_at: 2,
            operation: SchemaOperationIdentity {
                idempotency_key: "historical-v1-verify".into(),
                request_digest: format!("sha256:{}", "2".repeat(64)),
                request_id: "historical-v1-verify".into(),
            },
        })
        .await
        .expect("historical verification claim persists");
    let fence = match claimed {
        ClaimSchemaVerificationOutcome::Claimed(record)
        | ClaimSchemaVerificationOutcome::Replayed(record) => record.fence,
    };
    let verified = store
        .finish_schema_verification(
            TenantId::default().as_str(),
            scope,
            &digest,
            fence,
            SchemaVerificationReceipt {
                id: "historical-v1-receipt".into(),
                verifier_version: "historical/v1".into(),
                input_digest: format!("sha256:{}", "3".repeat(64)),
                passed: true,
            },
        )
        .await
        .expect("historical verification receipt persists");
    store
        .activate_schema_bundle(ActivateSchemaBundle {
            tenant: TenantId::default().to_string(),
            scope: scope.clone(),
            bundle_digest: digest,
            expected_predecessor: None,
            expected_fence: verified.fence,
            verification_receipt_id: "historical-v1-receipt".into(),
            stream_publication_fence: None,
            operation: SchemaOperationIdentity {
                idempotency_key: "historical-v1-activate".into(),
                request_digest: format!("sha256:{}", "4".repeat(64)),
                request_id: "historical-v1-activate".into(),
            },
        })
        .await
        .expect("historical active pointer persists");
}
