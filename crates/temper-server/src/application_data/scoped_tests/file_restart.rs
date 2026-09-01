use super::*;

#[tokio::test]
async fn scoped_native_file_write_commits_only_to_the_pinned_journal() {
    let sim = temper_store_sim::SimEventStore::no_faults(7606);
    let mut state = crate::state::ServerState::from_registry(
        temper_runtime::ActorSystem::new("scoped-file-write"),
        crate::registry::SpecRegistry::new(),
    );
    state.set_storage_stack(crate::storage::StorageStack::from_sim(sim, None));
    let data_dir = tempfile::tempdir().expect("scoped File blob directory");
    state.data_dir = data_dir.path().to_path_buf();
    let pin = install_file_scope(&state).await;
    let grant = ModuleDataGrant {
        operations: BTreeSet::from([
            DataOperationKind::EntityCreate,
            DataOperationKind::FileWrite,
        ]),
        entities: vec![EntityDataGrant {
            entity_type: "Temper.ScopedFile.File".into(),
            file_operations: BTreeSet::from([FileOperationKind::ContentWrite]),
            ..EntityDataGrant::default()
        }],
        ..ModuleDataGrant::default()
    };
    let csdl = temper_spec::parse_csdl(FILE_CSDL).expect("File CSDL parses");
    let sources = [IoaSourceInput {
        entity_type: "Temper.ScopedFile.File".into(),
        source: FILE_IOA.into(),
    }];
    let model = temper_spec::CanonicalSpecModel::link_v2_sources(&csdl, &sources)
        .expect("File canonical model links");
    let generated = temper_codegen::generate_module_sdk(
        &model,
        "file-worker",
        "file-closure",
        "file-closure",
        "file-artifact",
        grant,
    )
    .expect("File client binding generates");
    let invocation = ApplicationDataInvocation::new(
        state.clone(),
        ModuleInvocationAuthority::new(
            temper_runtime::tenant::TenantId::default(),
            "file-worker".into(),
            "file-artifact".into(),
            "Write".into(),
            "File".into(),
            SecurityContext::system(),
            generated.manifest,
            ModuleDataTarget::Scoped(pin.clone()),
        ),
    );
    let missing_workspace_file = "scoped-file-missing-workspace";
    let created = call(
        &invocation,
        DataOperationV1::EntityCreate {
            entity_type: "Temper.ScopedFile.File".into(),
            value: serde_json::json!({
                "Id": missing_workspace_file,
                "workspace_id": "missing-workspace"
            })
            .as_object()
            .cloned()
            .expect("test create payload must be an object"),
        },
    )
    .await;
    assert!(
        matches!(created.outcome, DataOutcomeV1::Ok { .. }),
        "scoped missing-Workspace File create failed: {created:?}"
    );
    let blob_entries_before = std::fs::read_dir(data_dir.path())
        .expect("scoped File blob directory remains readable")
        .count();
    let opened = call(
        &invocation,
        DataOperationV1::FileWriteOpen {
            file_id: missing_workspace_file.into(),
            expected_sequence: None,
            content_length: Some(7),
            content_hash: None,
        },
    )
    .await;
    let DataOutcomeV1::Ok {
        result: DataResultV1::FileWrite { stream_handle },
    } = opened.outcome
    else {
        panic!("scoped missing-Workspace File stream should open: {opened:?}")
    };
    assert_eq!(invocation.stream_write(stream_handle, b"blocked"), Ok(7));
    let rejected = call(
        &invocation,
        DataOperationV1::FileWriteCommit { stream_handle },
    )
    .await;
    assert!(
        matches!(rejected.outcome, DataOutcomeV1::Error { .. }),
        "missing scoped Workspace must fail closed: {rejected:?}"
    );
    assert_eq!(
        std::fs::read_dir(data_dir.path())
            .expect("scoped File blob directory remains readable")
            .count(),
        blob_entries_before,
        "a failed scoped Workspace lookup must not persist blob bytes"
    );
    let rejected_file = state
        .get_scoped_entity_state(
            &temper_runtime::tenant::TenantId::default(),
            "File",
            missing_workspace_file,
            pin.clone(),
        )
        .await
        .expect("rejected File remains readable in its exact scope");
    assert_eq!(rejected_file.state.status, "Created");

    let file_id = "scoped-file";
    let created = call(
        &invocation,
        DataOperationV1::EntityCreate {
            entity_type: "Temper.ScopedFile.File".into(),
            value: serde_json::json!({"Id": file_id})
                .as_object()
                .cloned()
                .expect("test create payload must be an object"),
        },
    )
    .await;
    assert!(
        matches!(created.outcome, DataOutcomeV1::Ok { .. }),
        "scoped native File create failed: {created:?}"
    );
    let opened = call(
        &invocation,
        DataOperationV1::FileWriteOpen {
            file_id: file_id.into(),
            expected_sequence: None,
            content_length: Some(6),
            content_hash: None,
        },
    )
    .await;
    let DataOutcomeV1::Ok {
        result: DataResultV1::FileWrite { stream_handle },
    } = opened.outcome
    else {
        panic!("scoped File stream should open: {opened:?}")
    };
    assert_eq!(invocation.stream_write(stream_handle, b"scoped"), Ok(6));
    let committed = call(
        &invocation,
        DataOperationV1::FileWriteCommit { stream_handle },
    )
    .await;
    assert!(matches!(
        committed.outcome,
        DataOutcomeV1::Ok {
            result: DataResultV1::FileCommitted { .. }
        }
    ));
    let file = state
        .get_scoped_entity_state(
            &temper_runtime::tenant::TenantId::default(),
            "File",
            file_id,
            pin,
        )
        .await
        .expect("scoped File should be readable at its exact pin");
    assert_eq!(file.state.status, "Ready");
    assert!(!state.entity_exists(
        &temper_runtime::tenant::TenantId::default(),
        "File",
        file_id
    ));
}

#[tokio::test]
async fn seeded_scoped_restart_and_fault_schedules_preserve_isolation() {
    let operations = BTreeSet::from([
        DataOperationKind::EntityCreate,
        DataOperationKind::EntityGet,
        DataOperationKind::EntityPatch,
    ]);
    let id = "018f1f80-7b2d-7000-8000-000000000081";
    for seed in 7_610..7_618 {
        let sim = temper_store_sim::SimEventStore::no_faults(seed);
        let template = invocation(operations.clone(), SecurityContext::system());
        let mut state = template.state.clone();
        state.set_storage_stack(crate::storage::StorageStack::from_sim(sim.clone(), None));
        let pin_a = install_scope(&state, &format!("seed-{seed}-a")).await;
        let pin_b = install_scope(&state, &format!("seed-{seed}-b")).await;
        let scope_a = scoped_invocation(state.clone(), &template.authority, pin_a.clone());
        let scope_b = scoped_invocation(state.clone(), &template.authority, pin_b.clone());
        for (current, name) in [(&scope_a, "scope-a"), (&scope_b, "scope-b")] {
            let created = call(
                current,
                DataOperationV1::EntityCreate {
                    entity_type: "Temper.Example.Customer".into(),
                    value: serde_json::json!({"Id": id, "Name": name})
                        .as_object()
                        .cloned()
                        .expect("test create payload must be an object"),
                },
            )
            .await;
            assert!(
                matches!(created.outcome, DataOutcomeV1::Ok { .. }),
                "seed {seed} failed initial scoped create: {created:?}"
            );
        }

        drop(scope_a);
        drop(scope_b);
        drop(state);
        let restart_template = invocation(operations.clone(), SecurityContext::system());
        let mut restarted = restart_template.state.clone();
        restarted.set_storage_stack(crate::storage::StorageStack::from_sim(sim.clone(), None));
        let deployment = crate::schema_deployment::GovernedSchemaDeploymentService::new(&restarted);
        for pin in [&pin_a, &pin_b] {
            deployment
                .recover_registry_pointer(
                    temper_runtime::tenant::TenantId::default().as_str(),
                    &pin.scope,
                )
                .await
                .unwrap_or_else(|error| panic!("seed {seed} registry recovery failed: {error:?}"));
        }
        let recovered_a = scoped_invocation(
            restarted.clone(),
            &restart_template.authority,
            pin_a.clone(),
        );
        let recovered_b = scoped_invocation(
            restarted.clone(),
            &restart_template.authority,
            pin_b.clone(),
        );
        let mut rng = temper_store_sim::DeterministicRng::new(seed);
        let target_a = rng.next_u64() & 1 == 0;
        let violation_count = 1 + rng.next_u64() % 2;
        let (target, target_pin, expected_target, other, expected_other) = if target_a {
            (
                &recovered_a,
                &pin_a,
                "scope-a".to_string(),
                &recovered_b,
                "scope-b".to_string(),
            )
        } else {
            (
                &recovered_b,
                &pin_b,
                "scope-b".to_string(),
                &recovered_a,
                "scope-a".to_string(),
            )
        };
        let journal_id = temper_runtime::persistence::schema_deployment::scoped_journal_entity_id(
            id, target_pin,
        );
        let persistence_id = format!("default:Customer:{journal_id}");
        sim.inject_concurrency_violations(&persistence_id, violation_count);
        assert_eq!(
            sim.pending_concurrency_violations(&persistence_id),
            violation_count
        );
        let updated_name = format!("{expected_target}-updated-{violation_count}");
        let mut updated = false;
        for _ in 0..=violation_count {
            let response = call(
                target,
                DataOperationV1::EntityPatch {
                    entity_type: "Temper.Example.Customer".into(),
                    entity_id: id.into(),
                    expected_sequence: None,
                    value: serde_json::json!({"Name": updated_name.clone()})
                        .as_object()
                        .cloned()
                        .expect("test patch payload must be an object"),
                },
            )
            .await;
            if matches!(response.outcome, DataOutcomeV1::Ok { .. }) {
                updated = true;
                break;
            }
        }
        assert!(updated, "seed {seed} exhausted its scoped retry budget");
        assert_eq!(sim.pending_concurrency_violations(&persistence_id), 0);
        for (current, expected) in [
            (target, updated_name.as_str()),
            (other, expected_other.as_str()),
        ] {
            let response = call(
                current,
                DataOperationV1::EntityGet {
                    entity_type: "Temper.Example.Customer".into(),
                    entity_id: id.into(),
                    at_least_sequence: None,
                },
            )
            .await;
            assert_eq!(
                entity_value(&response)["Name"],
                expected,
                "seed {seed} crossed an immutable scope boundary"
            );
        }
    }
}
