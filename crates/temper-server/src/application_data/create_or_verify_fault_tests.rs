use std::collections::BTreeSet;

use temper_authz::SecurityContext;
use temper_wasm_sdk::data::{DataOperationKind, DataOperationV1, ModuleDataErrorKind};

use super::tests::{CSDL, IOA, call, invocation, response_error};
use super::{ApplicationDataInvocation, ModuleInvocationAuthority};

#[tokio::test]
async fn partial_post_commit_blob_failure_is_healed_by_restart_retry() {
    let store = temper_store_sim::SimEventStore::no_faults(82);
    let template = invocation(
        BTreeSet::from([DataOperationKind::EntityCreateOrVerify]),
        SecurityContext::system(),
    );
    let dir = tempfile::tempdir().unwrap();
    let ioa = format!(
        "{IOA}\n[[state]]\nname = \"Name\"\ntype = \"string\"\ninitial = \"\"\noverflow_inline_max_bytes = 1024\n\n[[state]]\nname = \"FailureReason\"\ntype = \"string\"\ninitial = \"\"\noverflow_inline_max_bytes = 1024\n"
    );
    let mut state = crate::state::ServerState::with_specs(
        temper_runtime::ActorSystem::new("create-or-verify-blob-fault"),
        temper_spec::csdl::parse_csdl(CSDL).unwrap(),
        CSDL.into(),
        std::collections::BTreeMap::from([("Customer".into(), ioa)]),
    )
    .unwrap();
    let mut stack = crate::storage::StorageStack::from_sim(store.clone(), None);
    stack.backend = crate::storage::BackendLabel::Turso;
    state.set_storage_stack(stack);
    state.blob_store_override = Some(crate::blob_store::BlobStore::failing_local_fs(
        dir.path(),
        1,
    ));
    let invocation = ApplicationDataInvocation::new(
        state.clone(),
        ModuleInvocationAuthority::new(
            template.authority.tenant.clone(),
            template.authority.module_name.clone(),
            template.authority.artifact_digest.clone(),
            template.authority.trigger.clone(),
            template.authority.triggering_entity_type.clone(),
            template.authority.security.clone(),
            template.authority.binding.clone(),
            template.authority.target.clone(),
        ),
    );
    let id = "018f1f80-7b2d-7000-8000-000000000092";
    let response = call(
        &invocation,
        DataOperationV1::EntityCreateOrVerify {
            entity_type: "Temper.Example.Customer".into(),
            idempotency_key: "request-92".into(),
            value: serde_json::json!({
                "Id": id,
                "Name": "n".repeat(4_096),
                "FailureReason": "f".repeat(4_096)
            })
            .as_object()
            .unwrap()
            .clone(),
        },
    )
    .await;
    let error = response_error(response);
    assert_eq!(error.kind, ModuleDataErrorKind::Internal, "{error:?}");
    let persistence_id = format!("default:Customer:{id}");
    assert_eq!(store.dump_journal(&persistence_id).len(), 1);
    let projection = store
        .dump_first_event_projection(&persistence_id)
        .expect("projection is co-committed before external blob writes");
    let keys = ["Name", "FailureReason"].map(|field| {
        projection.fields[field][crate::blobs::FIELD_OVERFLOW_REF_KEY]
            .as_str()
            .expect("large field projection contains a blob reference")
            .to_string()
    });
    let healthy_blob_store = crate::blob_store::BlobStore::local_fs(dir.path());
    let mut before = 0;
    for key in &keys {
        before += usize::from(healthy_blob_store.get(key).await.unwrap().is_some());
    }
    assert_eq!(
        before, 1,
        "the injected fault occurs after one object write"
    );

    let mut restarted_state = state.clone();
    let mut restarted_stack = crate::storage::StorageStack::from_sim(store.clone(), None);
    restarted_stack.backend = crate::storage::BackendLabel::Turso;
    restarted_state.set_storage_stack(restarted_stack);
    restarted_state.blob_store_override = Some(healthy_blob_store.clone());
    let restarted = ApplicationDataInvocation::new(
        restarted_state,
        ModuleInvocationAuthority::new(
            template.authority.tenant.clone(),
            template.authority.module_name.clone(),
            template.authority.artifact_digest.clone(),
            template.authority.trigger.clone(),
            template.authority.triggering_entity_type.clone(),
            template.authority.security.clone(),
            template.authority.binding.clone(),
            template.authority.target.clone(),
        ),
    );
    let retry = call(
        &restarted,
        DataOperationV1::EntityCreateOrVerify {
            entity_type: "Temper.Example.Customer".into(),
            idempotency_key: "request-92".into(),
            value: serde_json::json!({
                "Id": id,
                "Name": "n".repeat(4_096),
                "FailureReason": "f".repeat(4_096)
            })
            .as_object()
            .unwrap()
            .clone(),
        },
    )
    .await;
    assert!(matches!(
        retry.outcome,
        temper_wasm_sdk::data::DataOutcomeV1::Ok { .. }
    ));
    assert_eq!(store.dump_journal(&persistence_id).len(), 1);
    for key in keys {
        assert!(
            healthy_blob_store.get(&key).await.unwrap().is_some(),
            "restart retry repairs every referenced object"
        );
    }
}
