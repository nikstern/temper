use super::*;

#[tokio::test]
async fn typed_module_data_is_isolated_by_exact_scope_and_bundle() {
    let operations = BTreeSet::from([
        DataOperationKind::EntityCreate,
        DataOperationKind::EntityGet,
        DataOperationKind::EntityPatch,
        DataOperationKind::EntityQuery,
        DataOperationKind::ActionInvoke,
    ]);
    let template = invocation(operations, SecurityContext::system());
    let mut state = template.state.clone();
    state.set_storage_stack(crate::storage::StorageStack::from_sim(
        temper_store_sim::SimEventStore::no_faults(7601),
        None,
    ));
    let pin_a = install_scope(&state, "task-a").await;
    let pin_b = install_scope(&state, "task-b").await;
    let scope_a = scoped_invocation(state.clone(), &template.authority, pin_a.clone());
    let scope_b = scoped_invocation(state.clone(), &template.authority, pin_b.clone());
    let global = ApplicationDataInvocation::new(state.clone(), template.authority.clone());
    let id = "018f1f80-7b2d-7000-8000-000000000076";

    for (invocation, name) in [(&scope_a, "scope-a"), (&scope_b, "scope-b")] {
        let created = call(
            invocation,
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
            matches!(
                created.outcome,
                DataOutcomeV1::Ok {
                    result: DataResultV1::Write { .. }
                }
            ),
            "scoped create failed: {created:?}"
        );
    }

    let read_a = call(
        &scope_a,
        DataOperationV1::EntityGet {
            entity_type: "Temper.Example.Customer".into(),
            entity_id: id.into(),
            at_least_sequence: None,
        },
    )
    .await;
    let read_b = call(
        &scope_b,
        DataOperationV1::EntityGet {
            entity_type: "Temper.Example.Customer".into(),
            entity_id: id.into(),
            at_least_sequence: None,
        },
    )
    .await;
    assert_eq!(entity_value(&read_a)["Name"], "scope-a");
    assert_eq!(entity_value(&read_b)["Name"], "scope-b");

    let patched = call(
        &scope_a,
        DataOperationV1::EntityPatch {
            entity_type: "Temper.Example.Customer".into(),
            entity_id: id.into(),
            expected_sequence: None,
            value: serde_json::json!({"Name": "scope-a-patched"})
                .as_object()
                .cloned()
                .expect("test patch payload must be an object"),
        },
    )
    .await;
    assert!(matches!(
        patched.outcome,
        DataOutcomeV1::Ok {
            result: DataResultV1::Write { .. }
        }
    ));
    let renamed = call(
        &scope_b,
        DataOperationV1::ActionInvoke {
            entity_type: "Temper.Example.Customer".into(),
            entity_id: id.into(),
            action: "Rename".into(),
            expected_sequence: None,
            params: serde_json::json!({"Name": "scope-b-renamed"})
                .as_object()
                .cloned()
                .expect("test action parameters must be an object"),
        },
    )
    .await;
    assert!(matches!(
        renamed.outcome,
        DataOutcomeV1::Ok {
            result: DataResultV1::Action { .. }
        }
    ));

    for current in [&pin_a, &pin_b] {
        state.stop_and_remove_scoped_entity(&template.authority.tenant, "Customer", id, current);
    }

    for (invocation, expected_name) in
        [(&scope_a, "scope-a-patched"), (&scope_b, "scope-b-renamed")]
    {
        let recovered = call(
            invocation,
            DataOperationV1::EntityGet {
                entity_type: "Temper.Example.Customer".into(),
                entity_id: id.into(),
                at_least_sequence: None,
            },
        )
        .await;
        assert_eq!(entity_value(&recovered)["Name"], expected_name);
        let page = call(
            invocation,
            DataOperationV1::EntityQuery {
                entity_type: "Temper.Example.Customer".into(),
                filter: None,
                order_by: Vec::new(),
                page: PageV1 {
                    limit: 10,
                    cursor: None,
                },
            },
        )
        .await;
        let DataOutcomeV1::Ok {
            result: DataResultV1::Page { values, .. },
        } = page.outcome
        else {
            panic!("expected scoped page")
        };
        assert_eq!(values.len(), 1);
        assert_eq!(values[0].value["Name"], expected_name);
    }

    let global_read = call(
        &global,
        DataOperationV1::EntityGet {
            entity_type: "Temper.Example.Customer".into(),
            entity_id: id.into(),
            at_least_sequence: None,
        },
    )
    .await;
    assert_eq!(
        response_error(global_read).kind,
        ModuleDataErrorKind::NotFound,
        "scoped writes must never leak into tenant-global application data"
    );

    let wrong_digest = scoped_invocation(
        state,
        &template.authority,
        pin("task-a", &format!("sha256:{}", "c".repeat(64))),
    );
    let missing = call(
        &wrong_digest,
        DataOperationV1::EntityGet {
            entity_type: "Temper.Example.Customer".into(),
            entity_id: id.into(),
            at_least_sequence: None,
        },
    )
    .await;
    assert_eq!(
        response_error(missing).kind,
        ModuleDataErrorKind::SchemaMismatch,
        "a missing exact bundle must fail instead of falling back"
    );
}

#[tokio::test]
async fn scoped_composite_uses_the_exact_pinned_actor() {
    let operations = BTreeSet::from([
        DataOperationKind::CompositeInvoke,
        DataOperationKind::EntityCreate,
        DataOperationKind::EntityGet,
    ]);
    let template = invocation(operations, SecurityContext::system());
    let mut state = template.state.clone();
    state.set_storage_stack(crate::storage::StorageStack::from_sim(
        temper_store_sim::SimEventStore::no_faults(7602),
        None,
    ));
    let pin = install_scope(&state, "composite-scope").await;
    let mut binding = template.authority.binding.clone();
    binding.grant.entities[0].actions.remove("Rename");
    binding.grant.entities[0]
        .composite_actions
        .insert("Rename".into());
    binding.grant_digest = binding.grant.digest().expect("composite grant digest");
    let scoped = ApplicationDataInvocation::new(
        state.clone(),
        ModuleInvocationAuthority::new(
            template.authority.tenant.clone(),
            template.authority.module_name.clone(),
            template.authority.artifact_digest.clone(),
            template.authority.trigger.clone(),
            template.authority.triggering_entity_type.clone(),
            template.authority.security.clone(),
            binding,
            ModuleDataTarget::Scoped(pin.clone()),
        ),
    );
    let id = "018f1f80-7b2d-7000-8000-000000000077";
    let created = call(
        &scoped,
        DataOperationV1::EntityCreate {
            entity_type: "Temper.Example.Customer".into(),
            value: serde_json::json!({"Id": id, "Name": "before"})
                .as_object()
                .cloned()
                .expect("test create payload must be an object"),
        },
    )
    .await;
    assert!(
        matches!(created.outcome, DataOutcomeV1::Ok { .. }),
        "scoped composite create failed: {created:?}"
    );
    let composite = call(
        &scoped,
        DataOperationV1::CompositeInvoke {
            entity_type: "Temper.Example.Customer".into(),
            entity_id: id.into(),
            action: "Rename".into(),
            expected_sequence: None,
            params: serde_json::json!({"Name": "after"})
                .as_object()
                .cloned()
                .expect("test action parameters must be an object"),
        },
    )
    .await;
    assert!(matches!(
        composite.outcome,
        DataOutcomeV1::Ok {
            result: DataResultV1::Action { .. }
        }
    ));
    let scoped_state = state
        .get_scoped_entity_state(&template.authority.tenant, "Customer", id, pin)
        .await
        .expect("composite target should remain in the pinned journal");
    assert_eq!(scoped_state.state.fields["Name"], "after");
    assert!(!state.entity_exists(&template.authority.tenant, "Customer", id));
}
