use std::collections::BTreeSet;

use temper_authz::SecurityContext;
use temper_runtime::persistence::schema_deployment::{
    ClaimSchemaVerification, ClaimSchemaVerificationOutcome, SchemaExecutionPin,
    SchemaOperationIdentity, SchemaScope, SchemaScopeKind, SchemaVerificationReceipt,
};
use temper_spec::{IoaSourceInput, ScopedBundleBudgets, ScopedSpecBundle, ScopedSpecBundleInput};
use temper_wasm_sdk::data::{
    DataOperationKind, DataOperationV1, DataOutcomeV1, DataResultV1, EntityDataGrant,
    FileOperationKind, ModuleDataErrorKind, ModuleDataGrant, PageV1,
};
use temper_wasm_sdk::schema_deployment::{
    ActivateSchemaBundleRequestV1, SchemaBundleBudgetsV1, SchemaIoaSourceV1, SchemaScopeV1,
    SubmitSchemaBundleRequestV1,
};
use temper_wasm_sdk::{FailureDetailValue, FailureRetryability};

use super::tests::{CSDL, IOA, call, invocation, response_error};
use super::{ApplicationDataInvocation, ModuleDataTarget, ModuleInvocationAuthority};

const FILE_CSDL: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<edmx:Edmx Version="4.0" xmlns:edmx="http://docs.oasis-open.org/odata/ns/edmx"><edmx:DataServices><Schema Namespace="Temper.ScopedFile" xmlns="http://docs.oasis-open.org/odata/ns/edm"><EntityType Name="File" HasStream="true"><Key><PropertyRef Name="Id"/></Key><Property Name="Id" Type="Edm.String" Nullable="false"/><Property Name="Status" Type="Edm.String" Nullable="false" DefaultValue="Created"/><Property Name="workspace_id" Type="Edm.String" Nullable="true"/><Annotation Term="Temper.Vocab.Stream.Mutability" String="Mutable"/></EntityType><Action Name="StreamUpdated" IsBound="true"><Parameter Name="bindingParameter" Type="Temper.ScopedFile.File" Nullable="false"/><Parameter Name="content_hash" Type="Edm.String" Nullable="false"/><Parameter Name="size_bytes" Type="Edm.String" Nullable="false"/><Parameter Name="mime_type" Type="Edm.String" Nullable="false"/><Parameter Name="version_number" Type="Edm.String" Nullable="false"/><Parameter Name="previous_version_id" Type="Edm.String" Nullable="false"/><Parameter Name="created_by" Type="Edm.String" Nullable="false"/></Action><EntityContainer Name="Container"><EntitySet Name="Files" EntityType="Temper.ScopedFile.File"/></EntityContainer></Schema></edmx:DataServices></edmx:Edmx>"#;

const FILE_IOA: &str = r#"[automaton]
name = "File"
states = ["Created", "Ready"]
initial = "Created"
lifecycle_property = "Status"

[[action]]
name = "StreamUpdated"
kind = "input"
from = ["Created", "Ready"]
to = "Ready"
params = ["content_hash", "size_bytes", "mime_type", "version_number", "previous_version_id", "created_by"]
"#;

fn pin(scope_id: &str, digest: &str) -> SchemaExecutionPin {
    SchemaExecutionPin {
        scope: SchemaScope {
            kind: SchemaScopeKind::Task,
            id: scope_id.into(),
        },
        bundle_digest: digest.into(),
    }
}

async fn install_scope(state: &crate::state::ServerState, scope_id: &str) -> SchemaExecutionPin {
    let budgets = ScopedBundleBudgets::default();
    let compiled = ScopedSpecBundle::compile(ScopedSpecBundleInput {
        scope_id: scope_id.into(),
        predecessor_digest: None,
        csdl_xml: CSDL.into(),
        ioa_sources: vec![IoaSourceInput {
            entity_type: "Temper.Example.Customer".into(),
            source: IOA.into(),
        }],
        cedar_policies: Vec::new(),
        wasm_modules: Vec::new(),
        migration: None,
        budgets: budgets.clone(),
    })
    .expect("scope compiles");
    let digest = compiled.digest().to_string();
    let scope = SchemaScope {
        kind: SchemaScopeKind::Task,
        id: scope_id.into(),
    };
    let service = crate::schema_deployment::GovernedSchemaDeploymentService::new(state);
    let security = SecurityContext::system();
    let submitted = service
        .submit(
            "default",
            &security,
            SubmitSchemaBundleRequestV1 {
                request_id: format!("submit-{scope_id}"),
                idempotency_key: format!("submit-{scope_id}"),
                scope: SchemaScopeV1 {
                    kind: "task".into(),
                    id: scope_id.into(),
                },
                expected_predecessor: None,
                expected_digest: digest.clone(),
                canonicalization_version: temper_spec::bundle::SCOPED_SPEC_BUNDLE_CONTRACT_V2
                    .into(),
                csdl: CSDL.into(),
                ioa: vec![SchemaIoaSourceV1 {
                    entity_type: "Temper.Example.Customer".into(),
                    source: IOA.into(),
                }],
                cedar_policies: Vec::new(),
                wasm_modules: Vec::new(),
                migration: None,
                budgets: SchemaBundleBudgetsV1 {
                    verification_steps: budgets.verification_steps,
                    migration_fuel_per_entity: budgets.migration_fuel_per_entity,
                    migration_memory_pages: budgets.migration_memory_pages,
                    migration_input_bytes: budgets.migration_input_bytes,
                    migration_output_bytes: budgets.migration_output_bytes,
                    migration_entities_per_batch: budgets.migration_entities_per_batch,
                    migration_total_entities: budgets.migration_total_entities,
                    migration_total_batches: budgets.migration_total_batches,
                    migration_attempts: budgets.migration_attempts,
                },
            },
        )
        .await
        .expect("scope submits");
    let store = state
        .storage_stack
        .as_ref()
        .and_then(|stack| stack.schema_deployments.as_ref())
        .expect("schema store");
    let claimed = store
        .claim_schema_verification(ClaimSchemaVerification {
            tenant: "default".into(),
            scope: scope.clone(),
            bundle_digest: submitted.bundle_digest.clone(),
            logical_now: 1,
            lease_expires_at: 10,
            operation: SchemaOperationIdentity {
                idempotency_key: format!("verify-{scope_id}"),
                request_digest: format!("sha256:{}", "d".repeat(64)),
                request_id: format!("verify-{scope_id}"),
            },
        })
        .await
        .expect("verification claims");
    let record = match claimed {
        ClaimSchemaVerificationOutcome::Claimed(record) => record,
        ClaimSchemaVerificationOutcome::Replayed(record) => record,
    };
    let verified = store
        .finish_schema_verification(
            "default",
            &scope,
            &submitted.bundle_digest,
            record.fence,
            SchemaVerificationReceipt {
                id: format!("receipt-{scope_id}"),
                verifier_version: "scoped-data-test/v1".into(),
                input_digest: format!("sha256:{}", "e".repeat(64)),
                passed: true,
            },
        )
        .await
        .expect("verification finishes");
    service
        .activate(
            "default",
            &security,
            ActivateSchemaBundleRequestV1 {
                request_id: format!("activate-{scope_id}"),
                idempotency_key: format!("activate-{scope_id}"),
                scope: SchemaScopeV1 {
                    kind: "task".into(),
                    id: scope_id.into(),
                },
                bundle_digest: submitted.bundle_digest.clone(),
                expected_predecessor: None,
                expected_fence: verified.fence,
                verification_receipt_id: verified
                    .verification_receipt_id
                    .expect("receipt id committed"),
                stream_descriptor_completion_receipt_id: None,
            },
        )
        .await
        .expect("scope activates");
    pin(scope_id, &submitted.bundle_digest)
}

async fn install_file_scope(state: &crate::state::ServerState) -> SchemaExecutionPin {
    let scope_id = "file-scope";
    let budgets = ScopedBundleBudgets::default();
    let compiled = ScopedSpecBundle::compile(ScopedSpecBundleInput {
        scope_id: scope_id.into(),
        predecessor_digest: None,
        csdl_xml: FILE_CSDL.into(),
        ioa_sources: vec![IoaSourceInput {
            entity_type: "Temper.ScopedFile.File".into(),
            source: FILE_IOA.into(),
        }],
        cedar_policies: Vec::new(),
        wasm_modules: Vec::new(),
        migration: None,
        budgets: budgets.clone(),
    })
    .expect("File scope compiles");
    let digest = compiled.digest().to_string();
    let scope = SchemaScope {
        kind: SchemaScopeKind::Task,
        id: scope_id.into(),
    };
    let service = crate::schema_deployment::GovernedSchemaDeploymentService::new(state);
    let security = SecurityContext::system();
    let submitted = service
        .submit(
            "default",
            &security,
            SubmitSchemaBundleRequestV1 {
                request_id: "submit-file-scope".into(),
                idempotency_key: "submit-file-scope".into(),
                scope: SchemaScopeV1 {
                    kind: "task".into(),
                    id: scope_id.into(),
                },
                expected_predecessor: None,
                expected_digest: digest,
                canonicalization_version: temper_spec::bundle::SCOPED_SPEC_BUNDLE_CONTRACT_V2
                    .into(),
                csdl: FILE_CSDL.into(),
                ioa: vec![SchemaIoaSourceV1 {
                    entity_type: "Temper.ScopedFile.File".into(),
                    source: FILE_IOA.into(),
                }],
                cedar_policies: Vec::new(),
                wasm_modules: Vec::new(),
                migration: None,
                budgets: SchemaBundleBudgetsV1 {
                    verification_steps: budgets.verification_steps,
                    migration_fuel_per_entity: budgets.migration_fuel_per_entity,
                    migration_memory_pages: budgets.migration_memory_pages,
                    migration_input_bytes: budgets.migration_input_bytes,
                    migration_output_bytes: budgets.migration_output_bytes,
                    migration_entities_per_batch: budgets.migration_entities_per_batch,
                    migration_total_entities: budgets.migration_total_entities,
                    migration_total_batches: budgets.migration_total_batches,
                    migration_attempts: budgets.migration_attempts,
                },
            },
        )
        .await
        .expect("File scope submits");
    let store = state
        .storage_stack
        .as_ref()
        .and_then(|stack| stack.schema_deployments.as_ref())
        .expect("schema store");
    let claimed = store
        .claim_schema_verification(ClaimSchemaVerification {
            tenant: "default".into(),
            scope: scope.clone(),
            bundle_digest: submitted.bundle_digest.clone(),
            logical_now: 1,
            lease_expires_at: 10,
            operation: SchemaOperationIdentity {
                idempotency_key: "verify-file-scope".into(),
                request_digest: format!("sha256:{}", "f".repeat(64)),
                request_id: "verify-file-scope".into(),
            },
        })
        .await
        .expect("File verification claims");
    let record = match claimed {
        ClaimSchemaVerificationOutcome::Claimed(record) => record,
        ClaimSchemaVerificationOutcome::Replayed(record) => record,
    };
    let verified = store
        .finish_schema_verification(
            "default",
            &scope,
            &submitted.bundle_digest,
            record.fence,
            SchemaVerificationReceipt {
                id: "receipt-file-scope".into(),
                verifier_version: "scoped-file-test/v1".into(),
                input_digest: format!("sha256:{}", "a".repeat(64)),
                passed: true,
            },
        )
        .await
        .expect("File verification finishes");
    service
        .activate(
            "default",
            &security,
            ActivateSchemaBundleRequestV1 {
                request_id: "activate-file-scope".into(),
                idempotency_key: "activate-file-scope".into(),
                scope: SchemaScopeV1 {
                    kind: "task".into(),
                    id: scope_id.into(),
                },
                bundle_digest: submitted.bundle_digest.clone(),
                expected_predecessor: None,
                expected_fence: verified.fence,
                verification_receipt_id: verified
                    .verification_receipt_id
                    .expect("File receipt id committed"),
                stream_descriptor_completion_receipt_id: None,
            },
        )
        .await
        .expect("File scope activates");
    pin(scope_id, &submitted.bundle_digest)
}

fn scoped_invocation(
    state: crate::state::ServerState,
    template: &ModuleInvocationAuthority,
    pin: SchemaExecutionPin,
) -> std::sync::Arc<ApplicationDataInvocation> {
    ApplicationDataInvocation::new(
        state,
        ModuleInvocationAuthority::new(
            template.tenant.clone(),
            template.module_name.clone(),
            template.artifact_digest.clone(),
            template.trigger.clone(),
            template.triggering_entity_type.clone(),
            template.security.clone(),
            template.binding.clone(),
            ModuleDataTarget::Scoped(pin),
        ),
    )
}

fn entity_value(response: &temper_wasm_sdk::data::DataResponseV1) -> serde_json::Value {
    let DataOutcomeV1::Ok {
        result: DataResultV1::Entity { value, .. },
    } = &response.outcome
    else {
        panic!("expected entity result, got {response:?}")
    };
    serde_json::Value::Object(value.clone())
}

#[path = "scoped_tests/enforcement.rs"]
mod enforcement;
#[path = "scoped_tests/file_restart.rs"]
mod file_restart;
#[path = "scoped_tests/isolation.rs"]
mod isolation;
