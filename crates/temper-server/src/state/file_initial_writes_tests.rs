use temper_runtime::ActorSystem;
use temper_runtime::tenant::TenantId;

use super::*;

const FILE_CSDL: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<edmx:Edmx Version="4.0" xmlns:edmx="http://docs.oasis-open.org/odata/ns/edmx">
  <edmx:DataServices><Schema Namespace="Temper.FileFault" xmlns="http://docs.oasis-open.org/odata/ns/edm">
    <EntityType Name="File" HasStream="true"><Key><PropertyRef Name="Id"/></Key>
      <Property Name="Id" Type="Edm.String" Nullable="false"/><Property Name="Status" Type="Edm.String"/>
      <Property Name="content_hash" Type="Edm.String"/><Property Name="mime_type" Type="Edm.String"/>
      <Property Name="has_content" Type="Edm.Boolean"/><Property Name="size_bytes" Type="Edm.Int64"/>
      <Annotation Term="Temper.Vocab.Stream.Mutability" String="Mutable"/>
    </EntityType><EntityContainer Name="Container"><EntitySet Name="Files" EntityType="Temper.FileFault.File"/></EntityContainer>
  </Schema></edmx:DataServices>
</edmx:Edmx>"#;

const FILE_IOA: &str = r#"[automaton]
name = "File"
states = ["Created", "Ready"]
initial = "Created"

[[state]]
name = "content_hash"
type = "string"
initial = ""

[[state]]
name = "has_content"
type = "bool"
initial = "false"

[[state]]
name = "size_bytes"
type = "counter"
initial = "0"

[[action]]
name = "StreamUpdated"
kind = "input"
from = ["Created", "Ready"]
to = "Ready"
params = ["content_hash", "size_bytes", "mime_type", "version_number", "previous_version_id", "created_by"]
effect = [{ type = "set_counter_from_param", var = "size_bytes", param = "size_bytes" }, { type = "set_bool", var = "has_content", value = "true" }]
"#;

async fn assert_initial_file_fault(
    faults: temper_store_sim::SimFaultConfig,
    expected: fn(String) -> FileStreamContentError,
) {
    let store = temper_store_sim::SimEventStore::new(9_301, faults);
    let mut registry = crate::registry::SpecRegistry::new();
    registry.register_tenant(
        "default",
        temper_spec::parse_csdl(FILE_CSDL).expect("File CSDL parses"),
        FILE_CSDL.to_string(),
        &[("File", FILE_IOA)],
    );
    let mut state = ServerState::from_registry(ActorSystem::new("initial-file-fault"), registry);
    state.set_storage_stack(crate::storage::StorageStack::from_sim(store.clone(), None));
    state.data_dir = tempfile::tempdir().expect("blob directory").keep();

    let error = state
        .create_file_with_initial_stream_content_checked(
            &TenantId::default(),
            "file-fault",
            serde_json::json!({}),
            b"bytes",
            "text/plain",
            &crate::request_context::AgentContext::for_service("fault-test"),
        )
        .await
        .expect_err("injected persistence failure must be surfaced");
    assert_eq!(
        std::mem::discriminant(&error),
        std::mem::discriminant(&expected(String::new()))
    );
}

#[tokio::test]
async fn initial_file_append_preserves_all_persistence_phases() {
    assert_initial_file_fault(
        temper_store_sim::SimFaultConfig {
            write_failure_prob: 1.0,
            ..temper_store_sim::SimFaultConfig::none()
        },
        FileStreamContentError::PersistenceNotApplied,
    )
    .await;
    assert_initial_file_fault(
        temper_store_sim::SimFaultConfig {
            append_post_commit_failure_prob: 1.0,
            ..temper_store_sim::SimFaultConfig::none()
        },
        FileStreamContentError::PersistenceApplied,
    )
    .await;
    assert_initial_file_fault(
        temper_store_sim::SimFaultConfig {
            append_acknowledgement_loss_prob: 1.0,
            ..temper_store_sim::SimFaultConfig::none()
        },
        FileStreamContentError::PersistenceUnknown,
    )
    .await;
}
