use super::*;

#[tokio::test]
async fn scoped_cedar_denial_preserves_stable_error_fields() {
    let security = SecurityContext::from_resolved_identity("scoped-user", "test-agent", None);
    let template = invocation(BTreeSet::from([DataOperationKind::EntityCreate]), security);
    let mut state = template.state.clone();
    state.set_storage_stack(crate::storage::StorageStack::from_sim(
        temper_store_sim::SimEventStore::no_faults(7603),
        None,
    ));
    let pin = install_scope(&state, "cedar-scope").await;
    state
        .authz
        .reload_tenant_policies_named(
            template.authority.tenant.as_str(),
            &[
                (
                    "allow-scoped-customer".to_string(),
                    r#"permit(principal, action, resource is Customer);"#.to_string(),
                ),
                (
                    "decision:block-scoped-create".to_string(),
                    r#"forbid(principal, action == Action::"create", resource is Customer);"#
                        .to_string(),
                ),
            ],
        )
        .expect("restrictive scoped policy should parse");
    let scoped = scoped_invocation(state, &template.authority, pin);
    let denied = call(
        &scoped,
        DataOperationV1::EntityCreate {
            entity_type: "Temper.Example.Customer".into(),
            value: serde_json::json!({"Id":"018f1f80-7b2d-7000-8000-000000000078"})
                .as_object()
                .cloned()
                .expect("test create payload must be an object"),
        },
    )
    .await;
    let error = response_error(denied);
    assert_eq!(error.kind, ModuleDataErrorKind::AuthorizationDenied);
    assert_eq!(error.code, "AuthorizationDenied");
    assert_eq!(error.message, "caller is not authorized for this operation");
    assert_eq!(error.retryability, Retryability::Never);
    assert!(
        error
            .decision_id
            .as_deref()
            .is_some_and(|id| id.contains("decision:block-scoped-create"))
    );
    let details = error.details.expect("Cedar denial details survive the ABI");
    assert_eq!(details["denial_class"], "policy_denied");
    assert!(
        details["policy_ids"]
            .as_array()
            .is_some_and(|ids| ids.iter().any(|id| {
                id.as_str()
                    .is_some_and(|id| id.contains("decision:block-scoped-create"))
            }))
    );
}

#[tokio::test]
async fn scoped_call_budget_is_enforced_without_losing_error_shape() {
    let template = invocation(
        BTreeSet::from([
            DataOperationKind::EntityCreate,
            DataOperationKind::EntityGet,
        ]),
        SecurityContext::system(),
    );
    let mut state = template.state.clone();
    state.set_storage_stack(crate::storage::StorageStack::from_sim(
        temper_store_sim::SimEventStore::no_faults(7604),
        None,
    ));
    let pin = install_scope(&state, "budget-scope").await;
    let mut binding = template.authority.binding.clone();
    binding.grant.budgets.max_calls = 1;
    binding.grant_digest = binding.grant.digest().expect("budgeted grant digest");
    let scoped = ApplicationDataInvocation::new(
        state,
        ModuleInvocationAuthority::new(
            template.authority.tenant.clone(),
            template.authority.module_name.clone(),
            template.authority.artifact_digest.clone(),
            template.authority.trigger.clone(),
            template.authority.triggering_entity_type.clone(),
            template.authority.security.clone(),
            binding,
            ModuleDataTarget::Scoped(pin),
        ),
    );
    let id = "018f1f80-7b2d-7000-8000-000000000079";
    let first = call(
        &scoped,
        DataOperationV1::EntityCreate {
            entity_type: "Temper.Example.Customer".into(),
            value: serde_json::json!({"Id": id})
                .as_object()
                .cloned()
                .expect("test create payload must be an object"),
        },
    )
    .await;
    assert!(matches!(first.outcome, DataOutcomeV1::Ok { .. }));
    let exhausted = call(
        &scoped,
        DataOperationV1::EntityGet {
            entity_type: "Temper.Example.Customer".into(),
            entity_id: id.into(),
            at_least_sequence: None,
        },
    )
    .await;
    let error = response_error(exhausted);
    assert_eq!(error.kind, ModuleDataErrorKind::BudgetExceeded);
    assert_eq!(error.code, "CallBudgetExceeded");
    assert_eq!(error.retryability, Retryability::Never);
}

#[tokio::test]
async fn scoped_query_rejects_an_authoritative_set_larger_than_its_scan_budget() {
    let template = invocation(
        BTreeSet::from([
            DataOperationKind::EntityCreate,
            DataOperationKind::EntityQuery,
        ]),
        SecurityContext::system(),
    );
    let mut state = template.state.clone();
    state.set_storage_stack(crate::storage::StorageStack::from_sim(
        temper_store_sim::SimEventStore::no_faults(7607),
        None,
    ));
    let pin = install_scope(&state, "query-budget-scope").await;
    let mut binding = template.authority.binding.clone();
    binding.grant.budgets.max_page_items = 1;
    binding.grant_digest = binding.grant.digest().expect("query grant digest");
    let scoped = ApplicationDataInvocation::new(
        state,
        ModuleInvocationAuthority::new(
            template.authority.tenant.clone(),
            template.authority.module_name.clone(),
            template.authority.artifact_digest.clone(),
            template.authority.trigger.clone(),
            template.authority.triggering_entity_type.clone(),
            template.authority.security.clone(),
            binding,
            ModuleDataTarget::Scoped(pin),
        ),
    );
    for suffix in 1..=9 {
        let created = call(
            &scoped,
            DataOperationV1::EntityCreate {
                entity_type: "Temper.Example.Customer".into(),
                value: serde_json::json!({
                    "Id": format!("018f1f80-7b2d-7000-8000-{suffix:012}"),
                    "Name": format!("customer-{suffix}")
                })
                .as_object()
                .cloned()
                .expect("test create payload must be an object"),
            },
        )
        .await;
        assert!(
            matches!(created.outcome, DataOutcomeV1::Ok { .. }),
            "scoped query fixture create failed: {created:?}"
        );
    }
    let response = call(
        &scoped,
        DataOperationV1::EntityQuery {
            entity_type: "Temper.Example.Customer".into(),
            filter: None,
            order_by: Vec::new(),
            page: PageV1 {
                limit: 1,
                cursor: None,
            },
        },
    )
    .await;
    let error = response_error(response);
    assert_eq!(error.kind, ModuleDataErrorKind::BudgetExceeded);
    assert_eq!(error.code, "QueryFallbackBudgetExceeded");
}

#[tokio::test]
async fn scoped_binding_cannot_cross_tenant_even_with_the_same_pin() {
    let template = invocation(
        BTreeSet::from([DataOperationKind::EntityCreate]),
        SecurityContext::system(),
    );
    let mut state = template.state.clone();
    state.set_storage_stack(crate::storage::StorageStack::from_sim(
        temper_store_sim::SimEventStore::no_faults(7605),
        None,
    ));
    let pin = install_scope(&state, "tenant-scope").await;
    let other = ApplicationDataInvocation::new(
        state,
        ModuleInvocationAuthority::new(
            temper_runtime::tenant::TenantId::new("other-tenant"),
            template.authority.module_name.clone(),
            template.authority.artifact_digest.clone(),
            template.authority.trigger.clone(),
            template.authority.triggering_entity_type.clone(),
            template.authority.security.clone(),
            template.authority.binding.clone(),
            ModuleDataTarget::Scoped(pin),
        ),
    );
    let denied = call(
        &other,
        DataOperationV1::EntityCreate {
            entity_type: "Temper.Example.Customer".into(),
            value: serde_json::json!({"Id":"018f1f80-7b2d-7000-8000-000000000080"})
                .as_object()
                .cloned()
                .expect("test create payload must be an object"),
        },
    )
    .await;
    let error = response_error(denied);
    assert_eq!(error.kind, ModuleDataErrorKind::SchemaMismatch);
    assert_eq!(error.code, "ScopedSchemaUnavailable");
}
