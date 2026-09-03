use super::*;

#[test]
fn merge_with_no_cross_invariants_preserves_existing_ones() {
    // Regression: a follow-up merge that does not declare cross-invariants
    // (e.g. the Agent OS bootstrap running after a user app load) must not
    // wipe the ones already registered for the tenant. Observed live when
    // child entities on a Local parent returned 201 instead of 409 in the
    // Crucible walkthrough — the app load registered the rules, then the
    // agent-spec merge immediately erased them.
    const CROSS_INVARIANTS_TOML: &str = r#"
version = 1
default_delete_policy = "restrict"

[[invariant]]
name = "OrderStatusSanity"
kind = "hard"
on = "Order.*"
assert = 'related(Order, OrderId).status in ["Active"]'
"#;

    let mut registry = SpecRegistry::new();
    let (csdl, xml) = minimal_csdl();
    registry
        .try_register_tenant_with_reactions_and_constraints(
            "alpha",
            csdl,
            xml,
            &[("Order", ORDER_IOA)],
            Vec::new(),
            Some(CROSS_INVARIANTS_TOML.to_string()),
            false,
        )
        .expect("initial load should succeed");

    let tenant = TenantId::new("alpha");
    let initial_count = registry
        .get_tenant(&tenant)
        .unwrap()
        .cross_invariants
        .as_ref()
        .map(|c| c.invariants.len())
        .unwrap_or(0);
    assert_eq!(initial_count, 1, "sanity: cross-invariant registered");

    // Merge with cross_invariants_source = None (mimics agent OS bootstrap).
    let (new_csdl, new_xml) = task_csdl();
    registry
        .try_register_tenant_with_reactions_and_constraints(
            "alpha",
            new_csdl,
            new_xml,
            &[("Task", ORDER_IOA)],
            Vec::new(),
            None,
            true,
        )
        .expect("merge should succeed");

    let after_merge = registry
        .get_tenant(&tenant)
        .unwrap()
        .cross_invariants
        .as_ref()
        .map(|c| c.invariants.len())
        .unwrap_or(0);
    assert_eq!(
        after_merge, 1,
        "merge without cross-invariants must preserve existing ones"
    );
}

#[test]
fn replace_without_cross_invariants_clears_existing_ones() {
    // Replace mode is the opposite of merge: the caller is the full source
    // of truth, so a replace with `cross_invariants_source = None` must
    // clear any previously loaded rules.
    const CROSS_INVARIANTS_TOML: &str = r#"
version = 1
default_delete_policy = "restrict"

[[invariant]]
name = "OrderStatusSanity"
kind = "hard"
on = "Order.*"
assert = 'related(Order, OrderId).status in ["Active"]'
"#;

    let mut registry = SpecRegistry::new();
    let (csdl, xml) = minimal_csdl();
    registry
        .try_register_tenant_with_reactions_and_constraints(
            "alpha",
            csdl,
            xml,
            &[("Order", ORDER_IOA)],
            Vec::new(),
            Some(CROSS_INVARIANTS_TOML.to_string()),
            false,
        )
        .expect("initial load should succeed");

    let (csdl2, xml2) = minimal_csdl();
    registry
        .try_register_tenant_with_reactions_and_constraints(
            "alpha",
            csdl2,
            xml2,
            &[("Order", ORDER_IOA)],
            Vec::new(),
            None,
            false,
        )
        .expect("replace should succeed");

    let tenant = TenantId::new("alpha");
    assert!(
        registry
            .get_tenant(&tenant)
            .unwrap()
            .cross_invariants
            .is_none(),
        "replace mode must clear cross-invariants when the new payload has none"
    );
}

#[test]
fn replace_removes_entities_not_in_new_spec_set() {
    let mut registry = SpecRegistry::new();
    let (csdl, xml) = minimal_csdl();
    registry.register_tenant("alpha", csdl, xml, &[("Order", ORDER_IOA)]);
    let tenant = TenantId::new("alpha");

    let (csdl2, xml2) = task_csdl();
    registry
        .try_register_tenant_with_reactions_and_constraints(
            "alpha",
            csdl2,
            xml2,
            &[("Task", ORDER_IOA)],
            Vec::new(),
            None,
            false,
        )
        .expect("replace should succeed");

    assert!(
        registry.get_table(&tenant, "Order").is_none(),
        "Order removed in replace"
    );
    assert!(
        registry.get_table(&tenant, "Task").is_some(),
        "Task exists after replace"
    );
}

#[test]
fn poisoned_table_aborts_hot_reload_before_registry_mutation() {
    let mut registry = SpecRegistry::new();
    let (csdl, xml) = minimal_csdl();
    registry.register_tenant("alpha", csdl, xml, &[("Order", ORDER_IOA)]);
    let tenant = TenantId::new("alpha");
    let original_metadata = registry
        .get_tenant(&tenant)
        .unwrap()
        .csdl_xml
        .as_str()
        .to_string();
    let table = registry.get_table_live(&tenant, "Order").unwrap();
    let poison = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _guard = table.write().unwrap();
        panic!("poison transition table for regression coverage");
    }));
    assert!(poison.is_err());

    let (replacement_csdl, replacement_xml) = minimal_csdl();
    let error = registry
        .try_register_tenant(
            "alpha",
            replacement_csdl,
            replacement_xml,
            &[("Order", ORDER_IOA)],
        )
        .unwrap_err();
    assert_eq!(
        error,
        RegistryError::TableLockPoisoned {
            tenant: "alpha".into(),
            entity_type: "Order".into(),
        }
    );
    assert_eq!(
        registry.get_tenant(&tenant).unwrap().csdl_xml.as_str(),
        original_metadata
    );
}
