//! Compile the generator's real output for a representative closed SDK surface.

use std::collections::BTreeSet;
use std::fs;
use std::process::Command;

use temper_codegen::generate_module_sdk;
use temper_spec::bundle::IoaSourceInput;
use temper_spec::csdl::parse_csdl;
use temper_wasm_sdk::data::{
    DataOperationKind, EntityDataGrant, FileOperationKind, MODULE_SDK_MANIFEST_CONTRACT_VERSION_V2,
    ManifestActionResultCardinalityV1, ManifestCreateRoleV1, ManifestPatchRoleV1, ModuleDataGrant,
};

const CSDL: &str = r#"
<edmx:Edmx Version="4.0" xmlns:edmx="http://docs.oasis-open.org/odata/ns/edmx">
  <edmx:DataServices>
    <Schema Namespace="Temper.Generated" xmlns="http://docs.oasis-open.org/odata/ns/edm">
      <EnumType Name="TaskStatus"><Member Name="Open"/><Member Name="Done"/></EnumType>
      <EntityType Name="User"><Key><PropertyRef Name="Id"/></Key><Property Name="Id" Type="Edm.String" Nullable="false"/></EntityType>
      <EntityType Name="File" HasStream="true">
        <Key><PropertyRef Name="Id"/></Key>
        <Property Name="Id" Type="Edm.String" Nullable="false"/>
        <Property Name="Status" Type="Temper.Generated.TaskStatus" Nullable="false"/>
        <Property Name="Owner" Type="Temper.Generated.User" Nullable="false"/>
        <Property Name="Estimate" Type="Edm.Decimal" Nullable="true"/>
        <Property Name="CreatedAt" Type="Edm.DateTimeOffset" Nullable="false"/>
        <Property Name="FailureReason" Type="Edm.String" Nullable="false" DefaultValue=""/>
        <Annotation Term="Temper.Vocab.Stream.Mutability" String="Mutable"/>
        <Annotation Term="Temper.Vocab.Write.CreateProperties"><Collection><String>Owner</String><String>Estimate</String><String>FailureReason</String></Collection></Annotation>
        <Annotation Term="Temper.Vocab.Write.PatchProperties"><Collection><String>Owner</String><String>Estimate</String><String>FailureReason</String></Collection></Annotation>
      </EntityType>
      <Action Name="Complete" IsBound="true">
        <Parameter Name="binding" Type="Temper.Generated.File" Nullable="false"/>
        <Parameter Name="Note" Type="Edm.String" Nullable="true"/>
        <ReturnType Type="Temper.Generated.TaskStatus" Nullable="false"/>
      </Action>
      <Action Name="Snapshot" IsBound="true">
        <Parameter Name="binding" Type="Temper.Generated.File" Nullable="false"/>
        <ReturnType Type="Temper.Generated.File" Nullable="false"/>
      </Action>
      <Action Name="MaybeSnapshot" IsBound="true">
        <Parameter Name="binding" Type="Temper.Generated.File" Nullable="false"/>
        <Parameter Name="Owner" Type="Temper.Generated.User" Nullable="false"/>
        <Parameter Name="Digest" Type="Edm.String" Nullable="false"/>
        <ReturnType Type="Temper.Generated.File" Nullable="true"/>
      </Action>
      <Action Name="HandleFailure" IsBound="true">
        <Parameter Name="binding" Type="Temper.Generated.File" Nullable="false"/>
        <Parameter Name="Failure" Type="Temper.FailureEnvelopeV1" Nullable="false"/>
      </Action>
      <EntityContainer Name="Container">
        <EntitySet Name="Files" EntityType="Temper.Generated.File"/>
        <EntitySet Name="Users" EntityType="Temper.Generated.User"/>
      </EntityContainer>
    </Schema>
  </edmx:DataServices>
</edmx:Edmx>
"#;

#[test]
fn representative_generated_surface_compiles() {
    let csdl = parse_csdl(CSDL).unwrap();
    let sources = [IoaSourceInput {
        entity_type: "Temper.Generated.File".into(),
        source: r#"[automaton]
name = "File"
states = ["Open", "Done"]
initial = "Open"
lifecycle_property = "Status"

[[action]]
name = "Complete"
kind = "input"
to = "Done"
params = [{ name = "Note", type = "string", nullable = true }]

[[action]]
name = "Snapshot"
kind = "input"

[[action]]
name = "MaybeSnapshot"
kind = "input"
params = [
    { name = "Owner", type = "Temper.Generated.User" },
    { name = "Digest", type = "string" },
]

[[action]]
name = "HandleFailure"
kind = "input"
params = [{ name = "Failure", type = "Temper.FailureEnvelopeV1" }]
"#
        .into(),
    }];
    let model = temper_spec::CanonicalSpecModel::link_v2_sources(&csdl, &sources).unwrap();
    let generated = generate_module_sdk(
        &model,
        "worker",
        "closure",
        "closure",
        "unpackaged",
        ModuleDataGrant {
            operations: [
                DataOperationKind::EntityGet,
                DataOperationKind::EntityQuery,
                DataOperationKind::EntityCreate,
                DataOperationKind::EntityPatch,
                DataOperationKind::ActionInvoke,
                DataOperationKind::Batch,
                DataOperationKind::FileRead,
                DataOperationKind::FileWrite,
            ]
            .into_iter()
            .collect(),
            entities: vec![EntityDataGrant {
                entity_type: "Temper.Generated.File".into(),
                actions: BTreeSet::from([
                    "Complete".into(),
                    "HandleFailure".into(),
                    "MaybeSnapshot".into(),
                    "Snapshot".into(),
                ]),
                query_filter_fields: BTreeSet::from(["Status".into(), "Estimate".into()]),
                query_order_fields: BTreeSet::from(["CreatedAt".into(), "Estimate".into()]),
                query_order_by_sequence: true,
                file_operations: BTreeSet::from([
                    FileOperationKind::MetadataRead,
                    FileOperationKind::ContentRead,
                    FileOperationKind::VersionRead,
                    FileOperationKind::ContentWrite,
                ]),
                ..EntityDataGrant::default()
            }],
            ..ModuleDataGrant::default()
        },
    )
    .unwrap();

    assert_eq!(
        generated.manifest.contract_version,
        Some(MODULE_SDK_MANIFEST_CONTRACT_VERSION_V2)
    );
    let file = generated
        .manifest
        .entities
        .iter()
        .find(|entity| entity.entity_type == "Temper.Generated.File")
        .unwrap();
    let policy = |name: &str| {
        file.properties
            .iter()
            .find(|property| property.canonical_name == name)
            .unwrap()
            .write_policy
            .unwrap()
    };
    assert_eq!(policy("Id").create, ManifestCreateRoleV1::Required);
    assert_eq!(policy("Id").patch, ManifestPatchRoleV1::Forbidden);
    assert_eq!(policy("Status").create, ManifestCreateRoleV1::Forbidden);
    assert_eq!(policy("CreatedAt").create, ManifestCreateRoleV1::Forbidden);
    assert_eq!(policy("Estimate").create, ManifestCreateRoleV1::Optional);
    assert_eq!(policy("Estimate").patch, ManifestPatchRoleV1::Writable);
    let cardinality = |name: &str| {
        file.actions
            .iter()
            .find(|action| action.canonical_name == name)
            .unwrap()
            .result_cardinality
            .unwrap()
    };
    assert_eq!(
        cardinality("MaybeSnapshot"),
        ManifestActionResultCardinalityV1::Nullable
    );
    assert_eq!(
        cardinality("Snapshot"),
        ManifestActionResultCardinalityV1::Required
    );
    assert_eq!(
        cardinality("HandleFailure"),
        ManifestActionResultCardinalityV1::Void
    );

    let temp = tempfile::tempdir().unwrap();
    let sdk_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("temper-wasm-sdk");
    fs::create_dir(temp.path().join("src")).unwrap();
    fs::write(
        temp.path().join("Cargo.toml"),
        format!(
            "[package]\nname='generated-sdk-proof'\nversion='0.0.0'\nedition='2024'\n\n[dependencies]\ntemper-wasm-sdk={{path={sdk_path:?},features=['test-helpers']}}\nserde={{version='1',features=['derive']}}\nserde_json='1'\n"
        ),
    )
    .unwrap();
    let usage = r#"
pub fn typecheck_surface() {
    let id = FileId("file-1".into());
    let owner = TemperGeneratedUserId("user-1".into());
    let digest = String::from("sha256:abc");
    let note = String::from("done");
    let _status = TemperGeneratedTaskStatus::Open;
    let filter = FileFilter::status_eq(TemperGeneratedTaskStatus::Open);
    let order = FileOrder::created_at(OrderDirectionV1::Asc);
    let sequence_order = FileOrder::commit_sequence(OrderDirectionV1::Desc);
    let _query: fn(&mut FileClient, Option<FileFilter>, Vec<FileOrder>, PageV1) -> Result<TypedPage<File>, ModuleDataError> = FileClient::query;
    let create = FileCreate::new(FileIdRef::from(&id), TemperGeneratedUserIdRef::from(&owner))
        .with_estimate_null();
    let patch = FilePatch::new()
        .with_owner(TemperGeneratedUserIdRef::from(&owner))
        .with_estimate_null();
    let snapshot = FileSnapshotInput::new();
    let complete = FileCompleteInput::new().with_note(&note);
    let maybe_snapshot = FileMaybeSnapshotInput::new(
        TemperGeneratedUserIdRef::from(&owner),
        &digest,
    );
    fn failure_input(failure: &FailureEnvelopeV1) -> FileHandleFailureInput<'_> { FileHandleFailureInput::new(failure) }
    fn create_signature(client: &mut FileClient, value: &FileCreate<'_>) -> Result<TypedWrite<File>, ModuleDataError> { client.create(value) }
    fn patch_signature(client: &mut FileClient, id: &str, value: &FilePatch<'_>) -> Result<TypedWrite<File>, ModuleDataError> { client.patch(id, None, value) }
    fn snapshot_signature(client: &mut FileClient, id: &str, value: &FileSnapshotInput<'_>) -> Result<TypedAction<File>, ModuleDataError> { client.snapshot(id, None, value) }
    fn maybe_snapshot_signature(client: &mut FileClient, id: &str, value: &FileMaybeSnapshotInput<'_>) -> Result<TypedAction<Option<File>>, ModuleDataError> { client.maybe_snapshot(id, None, value) }
    fn failure_signature(client: &mut FileClient, id: &str, value: &FileHandleFailureInput<'_>) -> Result<TypedAction<()>, ModuleDataError> { client.handle_failure(id, None, value) }
    let _ = (create_signature, patch_signature, snapshot_signature, maybe_snapshot_signature, failure_signature);
    let _ = (failure_input, &create, &patch, &snapshot, &complete, &maybe_snapshot);
    assert_eq!(id.as_ref(), "file-1");
    assert_eq!(owner.as_ref(), "user-1");
    assert_eq!(digest, "sha256:abc");
    assert_eq!(note, "done");
    let _ = (filter, order, sequence_order);
    let _batch: fn(&mut DataClient, Vec<BatchItemV1>) -> Result<DataResultV1, ModuleDataError> = execute_batch;
    let _read: fn(&mut FileClient, String) -> Result<OpenedFileRead, ModuleDataError> = FileClient::open_file_read;
    let _version_read: fn(&mut FileClient, String, String) -> Result<OpenedFileRead, ModuleDataError> = FileClient::open_file_version_read;
}

#[test]
fn generated_entity_action_decodes_and_advances_the_next_read() {
    let value = serde_json::json!({
        "Id": "file-1",
        "Status": "Done",
        "Owner": "user-1",
        "Estimate": null,
        "CreatedAt": "2026-08-24T12:00:00Z",
        "FailureReason": ""
    });
    let id = FileId("file-1".into());
    let owner = TemperGeneratedUserId("user-1".into());
    let create_commit = CommitToken {
        entity_type: FileClient::ENTITY_TYPE.into(),
        entity_id: id.as_ref().into(),
        sequence: 1,
    };
    let patch_commit = CommitToken {
        sequence: 2,
        ..create_commit.clone()
    };
    install_native_data_host_for_test(vec![
        DataResponseV1::ok(DataResultV1::Write {
            commit: create_commit.clone(),
            value: Some(value.as_object().cloned().expect("fixture entity is an object")),
            value_omitted: false,
        }),
        DataResponseV1::ok(DataResultV1::Write {
            commit: patch_commit.clone(),
            value: Some(value.as_object().cloned().expect("fixture entity is an object")),
            value_omitted: false,
        }),
    ]);

    let mut client = FileClient::new();
    let create = FileCreate::new(FileIdRef::from(&id), TemperGeneratedUserIdRef::from(&owner))
        .with_estimate_null();
    let created = client.create(&create).expect("borrowed create command executes");
    assert_eq!(created.commit, create_commit);
    assert_eq!(created.value.expect("created entity").id, "file-1");
    let patch = FilePatch::new()
        .with_owner(TemperGeneratedUserIdRef::from(&owner))
        .with_estimate_null();
    let patched = client
        .patch(id.as_ref(), Some(create_commit.sequence), &patch)
        .expect("borrowed patch command executes");
    assert_eq!(patched.commit, patch_commit);
    assert_eq!(patched.value.expect("patched entity").owner.as_ref(), "user-1");
    assert_eq!(id.as_ref(), "file-1", "create and patch only borrow the id");
    assert_eq!(owner.as_ref(), "user-1", "commands only borrow the owner");

    let write_requests = take_native_data_requests_for_test();
    let DataOperationV1::EntityCreate {
        value: create_value,
        ..
    } = &write_requests[0].operation
    else {
        panic!("first generated write must be create");
    };
    assert!(create_value.contains_key("Id"));
    assert!(create_value.contains_key("Owner"));
    assert!(create_value.contains_key("Estimate"));
    assert!(!create_value.contains_key("Status"));
    assert!(!create_value.contains_key("CreatedAt"));
    assert!(!create_value.contains_key("FailureReason"));
    let DataOperationV1::EntityPatch {
        expected_sequence,
        value: patch_value,
        ..
    } = &write_requests[1].operation else {
        panic!("second generated write must be patch");
    };
    assert_eq!(*expected_sequence, Some(create_commit.sequence));
    assert!(patch_value.contains_key("Owner"));
    assert!(patch_value.contains_key("Estimate"));
    assert!(!patch_value.contains_key("Id"));
    assert!(!patch_value.contains_key("Status"));
    assert!(!patch_value.contains_key("CreatedAt"));

    let commit = CommitToken {
        entity_type: FileClient::ENTITY_TYPE.into(),
        entity_id: "file-1".into(),
        sequence: 9,
    };
    install_native_data_host_for_test(vec![
        DataResponseV1::ok(DataResultV1::Action {
            commit: commit.clone(),
            result: Some(value.clone()),
            result_omitted: false,
        }),
        DataResponseV1::ok(DataResultV1::Entity {
            value: value.as_object().cloned().expect("fixture entity is an object"),
            sequence: commit.sequence,
        }),
    ]);

    let mut client = FileClient::new();
    let input = FileSnapshotInput::new();
    let action = client
        .snapshot("file-1", None, &input)
        .expect("generated entity action decodes");
    assert_eq!(action.commit, commit);
    assert_eq!(action.result.expect("entity result").id, "file-1");
    let read = client.get("file-1").expect("sequence-aware keyed read");
    assert_eq!(read.sequence, commit.sequence);
    assert_eq!(read.value.status, TemperGeneratedTaskStatus::Done);
    assert_eq!(read.value.failure_reason, "");

    let requests = take_native_data_requests_for_test();
    assert_eq!(
        requests
            .iter()
            .filter(|request| matches!(request.operation, DataOperationV1::ActionInvoke { .. }))
            .count(),
        1,
        "the generated client must invoke the committed action exactly once"
    );
    assert!(matches!(
        &requests[1].operation,
        DataOperationV1::EntityGet {
            at_least_sequence: Some(9),
            ..
        }
    ));

    // The native test host is process-global, so keep the second response
    // sequence in this same test after the first sequence is fully consumed.
    {
    let value = serde_json::json!({
        "Id": "file-1",
        "Status": "Done",
        "Owner": "user-1",
        "Estimate": null,
        "CreatedAt": "2026-08-24T12:00:00Z",
        "FailureReason": ""
    });
    let commit = CommitToken {
        entity_type: FileClient::ENTITY_TYPE.into(),
        entity_id: "file-1".into(),
        sequence: 11,
    };
    install_native_data_host_for_test(vec![
        DataResponseV1::ok(DataResultV1::Action {
            commit: commit.clone(),
            result: None,
            result_omitted: true,
        }),
        DataResponseV1::ok(DataResultV1::Entity {
            value: value.as_object().cloned().expect("fixture entity is an object"),
            sequence: commit.sequence,
        }),
    ]);

    let mut client = FileClient::new();
    let input = FileSnapshotInput::new();
    let absence = client
        .snapshot("file-1", None, &input)
        .expect("committed action response")
        .required_result()
        .expect_err("the response budget deliberately omitted the entity result");
    assert_eq!(absence.reason, CommittedAbsenceReason::DeliberatelyOmitted);
    assert_eq!(absence.commit, commit);
    let read = client
        .read_committed(&absence.commit)
        .expect("entity-valued result has an authoritative keyed readback");
    assert_eq!(read.sequence, 11);

    let scalar_absence = TypedAction::<TemperGeneratedTaskStatus> {
        commit: commit.clone(),
        result: None,
        result_omitted: true,
    }
    .required_result()
    .expect_err("an omitted scalar result remains explicitly unrecoverable");
    assert_eq!(scalar_absence.commit, commit);
    assert_eq!(scalar_absence.reason, CommittedAbsenceReason::DeliberatelyOmitted);

    let nullable = TypedAction::<Option<File>> {
        commit: commit.clone(),
        result: Some(None),
        result_omitted: false,
    }
    .nullable_result();
    assert!(matches!(nullable, CommittedNullable::Null { .. }));

    let void_commit = TypedAction::<()> {
        commit: commit.clone(),
        result: None,
        result_omitted: false,
    }
    .void_result()
    .expect("void actions retain their commit token");
    assert_eq!(void_commit, commit);
    }
}
"#;
    fs::write(
        temp.path().join("src/lib.rs"),
        format!("{}\n{}", generated.source, usage),
    )
    .unwrap();
    let output = Command::new(env!("CARGO"))
        .args(["test", "--offline", "--quiet"])
        .current_dir(temp.path())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "generated SDK failed to compile:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
