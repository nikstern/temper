use super::*;

#[tokio::test]
async fn response_reservation_uses_the_complete_schema_before_owner_lookup() {
    let store = temper_store_sim::SimEventStore::no_faults(82);
    let existing_id = "018f1f80-7b2d-7000-8000-000000000088";
    let creator = durable_invocation_with_store(
        BTreeSet::from([DataOperationKind::EntityCreateOrVerify]),
        SecurityContext::system(),
        store.clone(),
    );
    let created = call(&creator, operation(existing_id, "request-88", "Ada")).await;
    assert!(matches!(created.outcome, DataOutcomeV1::Ok { .. }));

    let constrained = durable_invocation_with_response_budget(
        store.clone(),
        temper_wasm_sdk::data::ModuleDataBudgets::MIN_RESPONSE_BYTES,
    );
    for candidate in [
        operation(existing_id, "request-existing-88", "Ada"),
        operation(
            "018f1f80-7b2d-7000-8000-000000000089",
            "request-absent-89",
            "Ada",
        ),
    ] {
        let error = response_error(call(&constrained, candidate).await);
        assert_eq!(error.kind, ModuleDataErrorKind::BudgetExceeded);
        assert_eq!(error.code, "ResponseReservationExceeded");
    }
    assert_eq!(
        store
            .dump_journal("default:Customer:018f1f80-7b2d-7000-8000-000000000089")
            .len(),
        0
    );
}

#[tokio::test]
async fn response_reservation_includes_large_schema_defaults() {
    let store = temper_store_sim::SimEventStore::no_faults(82);
    let template = invocation(
        BTreeSet::from([DataOperationKind::EntityCreateOrVerify]),
        SecurityContext::system(),
    );
    let mut state = template.state.clone();
    state.set_storage_stack(crate::storage::StorageStack::from_sim(store.clone(), None));
    let mut binding = template.authority.binding.clone();
    binding.grant.budgets.max_response_bytes = 150_000;
    binding.entities[0]
        .properties
        .iter_mut()
        .find(|property| property.canonical_name == "Label")
        .expect("Label property")
        .default_value = Some(serde_json::Value::String("x".repeat(200_000)));
    let constrained = ApplicationDataInvocation::new(
        state,
        ModuleInvocationAuthority::new(
            template.authority.tenant.clone(),
            template.authority.module_name.clone(),
            template.authority.artifact_digest.clone(),
            template.authority.trigger.clone(),
            template.authority.triggering_entity_type.clone(),
            template.authority.security.clone(),
            binding,
            template.authority.target.clone(),
        ),
    );
    let id = "018f1f80-7b2d-7000-8000-000000000090";
    let error = response_error(call(&constrained, operation(id, "request-90", "Ada")).await);
    assert_eq!(error.kind, ModuleDataErrorKind::BudgetExceeded);
    assert_eq!(error.code, "ResponseReservationExceeded");
    assert!(
        store
            .dump_journal(&format!("default:Customer:{id}"))
            .is_empty()
    );
}
