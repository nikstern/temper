use std::collections::BTreeSet;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::{Extension, Router};
use temper_authz::{AuthenticatedRequestContext, SecurityContext};
use temper_runtime::{ActorSystem, tenant::TenantId};
use temper_spec::csdl::parse_csdl;
use temper_wasm_sdk::data::{
    DATA_ABI_VERSION_V1, DataOperationKind, DataOperationV1, DataOutcomeV1, DataOutcomeV2,
    DataRequestV1, DataRequestV2, DataResponseV1, DataResponseV2, EntityDataGrant,
    FileOperationKind, ManifestActionV1, ManifestEntityV1, ManifestPropertyV1, ModuleDataErrorKind,
    ModuleDataGrant, ModuleSdkManifest, ModuleSdkMetadataDigests,
};
use tower::ServiceExt;

use super::{ApplicationDataInvocation, ModuleInvocationAuthority};
use crate::state::ServerState;
pub(super) const CSDL: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<edmx:Edmx Version="4.0" xmlns:edmx="http://docs.oasis-open.org/odata/ns/edmx"><edmx:DataServices><Schema Namespace="Temper.Example" xmlns="http://docs.oasis-open.org/odata/ns/edm"><EnumType Name="Phase"><Member Name="Ready"/><Member Name="Done"/></EnumType><EntityType Name="Customer"><Key><PropertyRef Name="Id"/></Key><Property Name="Id" Type="Edm.Guid" Nullable="false"/><Property Name="Name" Type="Edm.String" Nullable="true"/><Property Name="Status" Type="Edm.String" Nullable="false"/><Property Name="RenameCount" Type="Edm.Int64" Nullable="true"/><Property Name="FailureReason" Type="Edm.String" Nullable="false" DefaultValue=""/><Property Name="Label" Type="Edm.String" Nullable="false" DefaultValue="unknown"/><Property Name="AttemptCount" Type="Edm.Int64" Nullable="false" DefaultValue="0"/><Property Name="Enabled" Type="Edm.Boolean" Nullable="false" DefaultValue="false"/><Property Name="Phase" Type="Temper.Example.Phase" Nullable="false" DefaultValue="Ready"/></EntityType><Action Name="Rename" IsBound="true"><Parameter Name="bindingParameter" Type="Temper.Example.Customer" Nullable="false"/><Parameter Name="Name" Type="Edm.String" Nullable="false"/><ReturnType Type="Temper.Example.Customer" Nullable="false"/></Action><Action Name="Reject" IsBound="true"><Parameter Name="bindingParameter" Type="Temper.Example.Customer" Nullable="false"/></Action><Action Name="Disable" IsBound="true"><Parameter Name="bindingParameter" Type="Temper.Example.Customer" Nullable="false"/></Action><EntityContainer Name="Container"><EntitySet Name="Customers" EntityType="Temper.Example.Customer"/></EntityContainer></Schema></edmx:DataServices></edmx:Edmx>"#;

pub(super) const IOA: &str = r#"[automaton]
name = "Customer"
states = ["Active", "Disabled"]
initial = "Active"
lifecycle_property = "Status"

[[state]]
name = "RenameCount"
type = "counter"
initial = "0"

[[action]]
name = "Rename"
kind = "input"
from = ["Active"]
to = "Active"
params = ["Name"]
effect = [{ type = "increment", var = "RenameCount" }]

[[action]]
name = "Reject"
kind = "input"
from = ["Disabled"]
to = "Disabled"

[[action]]
name = "Disable"
kind = "input"
from = ["Active"]
to = "Disabled"
"#;

fn manifest_property(
    canonical_name: &str,
    type_name: &str,
    nullable: bool,
    default_value: Option<serde_json::Value>,
    enum_members: Vec<String>,
    source: temper_wasm_sdk::data::ManifestValueSourceV1,
) -> ManifestPropertyV1 {
    ManifestPropertyV1 {
        canonical_name: canonical_name.into(),
        generated_name: temper_spec::to_snake_case(canonical_name),
        type_name: type_name.into(),
        nullable,
        source,
        default_value,
        enum_members,
        write_policy: None,
    }
}

mod file_capability_tests;
#[path = "tests/structured_failures.rs"]
mod structured_failures;
mod write_role_tests;

pub(super) fn authenticated_router(state: ServerState, security: SecurityContext) -> Router {
    crate::build_router(state).layer(Extension(AuthenticatedRequestContext::new(
        TenantId::default(),
        security,
    )))
}

pub(super) fn invocation(
    operations: BTreeSet<DataOperationKind>,
    security: SecurityContext,
) -> std::sync::Arc<ApplicationDataInvocation> {
    let state = ServerState::with_specs(
        ActorSystem::new("application-data-service-test"),
        parse_csdl(CSDL).expect("valid fixture CSDL"),
        CSDL.into(),
        std::collections::BTreeMap::from([("Customer".into(), IOA.into())]),
    )
    .expect("verified service state");
    let grant = ModuleDataGrant {
        operations,
        entities: vec![EntityDataGrant {
            entity_type: "Temper.Example.Customer".into(),
            actions: BTreeSet::from(["Reject".into(), "Rename".into()]),
            ..EntityDataGrant::default()
        }],
        ..ModuleDataGrant::default()
    };
    let binding = ModuleSdkManifest::new(
        "worker",
        ModuleSdkMetadataDigests {
            closure: "closure".into(),
            dependency_lock: "closure".into(),
            schema: "schema".into(),
        },
        "artifact",
        grant,
        vec![ManifestEntityV1 {
            entity_type: "Temper.Example.Customer".into(),
            entity_set: "Customers".into(),
            generated_name: "Customer".into(),
            lifecycle_states: Vec::new(),
            properties: vec![
                manifest_property(
                    "Id",
                    "Edm.Guid",
                    false,
                    None,
                    Vec::new(),
                    temper_wasm_sdk::data::ManifestValueSourceV1::EntityId,
                ),
                manifest_property(
                    "Name",
                    "Edm.String",
                    true,
                    None,
                    Vec::new(),
                    temper_wasm_sdk::data::ManifestValueSourceV1::StoredField,
                ),
                manifest_property(
                    "Status",
                    "Edm.String",
                    true,
                    None,
                    Vec::new(),
                    temper_wasm_sdk::data::ManifestValueSourceV1::LifecycleStatus,
                ),
                manifest_property(
                    "RenameCount",
                    "Edm.Int64",
                    true,
                    None,
                    Vec::new(),
                    temper_wasm_sdk::data::ManifestValueSourceV1::StoredField,
                ),
                manifest_property(
                    "FailureReason",
                    "Edm.String",
                    false,
                    Some(serde_json::json!("")),
                    Vec::new(),
                    temper_wasm_sdk::data::ManifestValueSourceV1::StoredField,
                ),
                manifest_property(
                    "Label",
                    "Edm.String",
                    false,
                    Some(serde_json::json!("unknown")),
                    Vec::new(),
                    temper_wasm_sdk::data::ManifestValueSourceV1::StoredField,
                ),
                manifest_property(
                    "AttemptCount",
                    "Edm.Int64",
                    false,
                    Some(serde_json::json!(0)),
                    Vec::new(),
                    temper_wasm_sdk::data::ManifestValueSourceV1::StoredField,
                ),
                manifest_property(
                    "Enabled",
                    "Edm.Boolean",
                    false,
                    Some(serde_json::json!(false)),
                    Vec::new(),
                    temper_wasm_sdk::data::ManifestValueSourceV1::StoredField,
                ),
                manifest_property(
                    "Phase",
                    "Temper.Example.Phase",
                    false,
                    Some(serde_json::json!("Ready")),
                    vec!["Done".into(), "Ready".into()],
                    temper_wasm_sdk::data::ManifestValueSourceV1::StoredField,
                ),
            ],
            actions: vec![
                ManifestActionV1 {
                    canonical_name: "Reject".into(),
                    generated_name: "reject".into(),
                    parameters: Vec::new(),
                    result_type: None,
                    result_enum_members: Vec::new(),
                    result_cardinality: None,
                    composite: false,
                },
                ManifestActionV1 {
                    canonical_name: "Rename".into(),
                    generated_name: "rename".into(),
                    parameters: vec![manifest_property(
                        "Name",
                        "Edm.String",
                        false,
                        None,
                        Vec::new(),
                        temper_wasm_sdk::data::ManifestValueSourceV1::Input,
                    )],
                    result_type: Some("Temper.Example.Customer".into()),
                    result_enum_members: Vec::new(),
                    result_cardinality: None,
                    composite: false,
                },
            ],
        }],
        BTreeSet::new(),
    )
    .expect("valid binding");
    let authority = ModuleInvocationAuthority::new(
        TenantId::default(),
        "worker".into(),
        "artifact".into(),
        "Created".into(),
        "Customer".into(),
        security,
        binding,
        super::ModuleDataTarget::TenantGlobal,
    );
    ApplicationDataInvocation::new(state, authority)
}

pub(super) async fn call(
    invocation: &ApplicationDataInvocation,
    operation: DataOperationV1,
) -> DataResponseV1 {
    let request = serde_json::to_vec(&DataRequestV2::new(operation)).expect("request encodes");
    let response = invocation
        .call_encoded(&request)
        .await
        .expect("host response");
    let response: DataResponseV2 = serde_json::from_slice(&response).expect("response decodes");
    response.into()
}

pub(super) fn response_error(response: DataResponseV1) -> temper_wasm_sdk::data::ModuleDataError {
    let DataOutcomeV1::Error { error } = response.outcome else {
        panic!("expected structured error")
    };
    error
}

#[tokio::test]
async fn artifact_binding_selects_one_exact_data_abi() {
    let current = invocation(
        BTreeSet::from([DataOperationKind::EntityGet]),
        SecurityContext::system(),
    );
    let mut legacy_binding = current.authority.binding.clone();
    legacy_binding.abi = DATA_ABI_VERSION_V1;
    let legacy = ApplicationDataInvocation::new(
        current.state.clone(),
        ModuleInvocationAuthority::new(
            current.authority.tenant.clone(),
            current.authority.module_name.clone(),
            current.authority.artifact_digest.clone(),
            current.authority.trigger.clone(),
            current.authority.triggering_entity_type.clone(),
            current.authority.security.clone(),
            legacy_binding,
            current.authority.target.clone(),
        ),
    );
    let operation = DataOperationV1::EntityGet {
        entity_type: "Temper.Example.Customer".into(),
        entity_id: "018f1f80-7b2d-7000-8000-000000000099".into(),
        at_least_sequence: None,
    };
    let request =
        serde_json::to_vec(&DataRequestV1::new(operation.clone())).expect("encode legacy request");
    let encoded = legacy
        .call_encoded(&request)
        .await
        .expect("legacy response");
    let wire: serde_json::Value = serde_json::from_slice(&encoded).expect("legacy response JSON");
    assert_eq!(wire["abi"], 1);
    assert_eq!(
        wire["outcome"]["error"]["message"],
        "application-data operation failed"
    );
    assert!(wire["outcome"]["error"].get("outcome").is_none());

    let wrong_request =
        serde_json::to_vec(&DataRequestV2::new(operation.clone())).expect("encode v2 request");
    let wrong_encoded = legacy
        .call_encoded(&wrong_request)
        .await
        .expect("legacy mismatch response");
    let mismatch: DataResponseV1 =
        serde_json::from_slice(&wrong_encoded).expect("decode legacy mismatch");
    assert_eq!(response_error(mismatch).code().as_str(), "AbiMismatch");

    let old_request =
        serde_json::to_vec(&DataRequestV1::new(operation)).expect("encode v1 request");
    let new_encoded = current
        .call_encoded(&old_request)
        .await
        .expect("v2 mismatch response");
    let mismatch: DataResponseV2 =
        serde_json::from_slice(&new_encoded).expect("decode v2 mismatch");
    assert_eq!(mismatch.abi, 2);
    let DataOutcomeV2::Error { error } = mismatch.outcome else {
        panic!("expected v2 mismatch error")
    };
    assert_eq!(error.code().as_str(), "AbiMismatch");
    assert_eq!(error.outcome(), temper_wasm_sdk::FailureOutcome::NotApplied);
}

#[tokio::test]
async fn capability_denies_before_system_cedar_authority() {
    let invocation = invocation(BTreeSet::new(), SecurityContext::system());
    let response = call(
        &invocation,
        DataOperationV1::EntityCreate {
            entity_type: "Temper.Example.Customer".into(),
            value: serde_json::json!({"Id":"018f1f80-7b2d-7000-8000-000000000001"})
                .as_object()
                .cloned()
                .unwrap_or_default(),
        },
    )
    .await;
    assert_eq!(
        response_error(response).kind(),
        ModuleDataErrorKind::AuthorizationDenied
    );
}

#[tokio::test]
async fn successful_create_and_read_share_governed_service() {
    let invocation = invocation(
        BTreeSet::from([
            DataOperationKind::EntityCreate,
            DataOperationKind::EntityGet,
            DataOperationKind::EntityQuery,
            DataOperationKind::Batch,
        ]),
        SecurityContext::system(),
    );
    let id = "018f1f80-7b2d-7000-8000-000000000001";
    let created = call(
        &invocation,
        DataOperationV1::EntityCreate {
            entity_type: "Temper.Example.Customer".into(),
            value: serde_json::json!({"Id": id})
                .as_object()
                .cloned()
                .unwrap_or_default(),
        },
    )
    .await;
    let DataOutcomeV1::Ok {
        result: temper_wasm_sdk::data::DataResultV1::Write { commit, .. },
    } = created.outcome
    else {
        panic!("create should return a write commit")
    };
    let read = call(
        &invocation,
        DataOperationV1::EntityGet {
            entity_type: "Temper.Example.Customer".into(),
            entity_id: id.into(),
            at_least_sequence: Some(commit.sequence),
        },
    )
    .await;
    let DataOutcomeV1::Ok {
        result: temper_wasm_sdk::data::DataResultV1::Entity { value, .. },
    } = read.outcome
    else {
        panic!("keyed read should return the created entity")
    };
    super::canonical_defaults_tests::assert_generated_customer_defaults(
        serde_json::Value::Object(value),
        None,
    );
    let impossible = call(
        &invocation,
        DataOperationV1::EntityGet {
            entity_type: "Temper.Example.Customer".into(),
            entity_id: id.into(),
            at_least_sequence: Some(commit.sequence + 1),
        },
    )
    .await;
    assert_eq!(
        response_error(impossible).kind(),
        ModuleDataErrorKind::ConsistencyUnavailable
    );
    let page = call(
        &invocation,
        DataOperationV1::EntityQuery {
            entity_type: "Temper.Example.Customer".into(),
            filter: None,
            order_by: Vec::new(),
            page: temper_wasm_sdk::data::PageV1 {
                limit: 10,
                cursor: None,
            },
        },
    )
    .await;
    let DataOutcomeV1::Ok {
        result: temper_wasm_sdk::data::DataResultV1::Page { values, .. },
    } = page.outcome
    else {
        panic!("query should return a canonical page")
    };
    assert_eq!(values.len(), 1);
    super::canonical_defaults_tests::assert_generated_customer_defaults(
        serde_json::Value::Object(values[0].value.clone()),
        None,
    );
    let batch = call(
        &invocation,
        DataOperationV1::Batch {
            items: vec![temper_wasm_sdk::data::BatchItemV1::EntityGet {
                entity_type: "Temper.Example.Customer".into(),
                entity_id: id.into(),
                at_least_sequence: Some(commit.sequence),
            }],
        },
    )
    .await;
    assert!(matches!(
        batch.outcome,
        DataOutcomeV1::Ok {
            result: temper_wasm_sdk::data::DataResultV1::Batch { .. }
        }
    ));
}

#[tokio::test]
async fn cedar_still_denies_after_capability_and_schema_accept() {
    let security = SecurityContext::from_resolved_identity("user-1", "test-agent", None);
    let invocation = invocation(
        BTreeSet::from([DataOperationKind::EntityCreate]),
        security.clone(),
    );
    invocation
        .state
        .authz
        .reload_tenant_policies(
            TenantId::default().as_str(),
            r#"permit(principal, action == Action::"read", resource is Customer);"#,
        )
        .expect("install restrictive Cedar policy");
    let response = call(
        &invocation,
        DataOperationV1::EntityCreate {
            entity_type: "Temper.Example.Customer".into(),
            value: serde_json::json!({"Id":"018f1f80-7b2d-7000-8000-000000000001"})
                .as_object()
                .cloned()
                .unwrap_or_default(),
        },
    )
    .await;
    let error = response_error(response);
    assert_eq!(
        error.kind(),
        ModuleDataErrorKind::AuthorizationDenied,
        "{error:?}"
    );
    let odata = authenticated_router(invocation.state.clone(), security)
        .oneshot(
            Request::post("/tdata/Customers")
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{"Id":"018f1f80-7b2d-7000-8000-000000000002"}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(odata.status(), StatusCode::FORBIDDEN);
}
