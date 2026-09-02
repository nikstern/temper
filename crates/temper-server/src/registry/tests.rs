use super::*;
use temper_runtime::persistence::schema_deployment::{SchemaScope, SchemaScopeKind};
use temper_spec::csdl::parse_csdl;

const CSDL_XML: &str = include_str!("../../../../test-fixtures/specs/model.csdl.xml");
const ORDER_IOA: &str = include_str!("../../../../test-fixtures/specs/order.ioa.toml");

fn minimal_csdl() -> (CsdlDocument, String) {
    let doc = parse_csdl(CSDL_XML).expect("CSDL should parse");
    (doc, CSDL_XML.to_string())
}

#[test]
fn register_and_lookup_tenant() {
    let mut registry = SpecRegistry::new();
    let (csdl, csdl_xml) = minimal_csdl();

    registry.register_tenant("alpha", csdl, csdl_xml, &[("Order", ORDER_IOA)]);

    let tenant = TenantId::new("alpha");
    assert!(registry.get_tenant(&tenant).is_some());
    assert!(registry.get_table(&tenant, "Order").is_some());
    assert!(registry.get_table(&tenant, "NonExistent").is_none());
}

#[test]
fn unknown_tenant_returns_none() {
    let registry = SpecRegistry::new();
    let tenant = TenantId::new("unknown");
    assert!(registry.get_tenant(&tenant).is_none());
    assert!(registry.get_table(&tenant, "Order").is_none());
}

#[test]
fn multiple_tenants_isolated() {
    let mut registry = SpecRegistry::new();
    let (csdl1, csdl_xml1) = minimal_csdl();
    let (csdl2, csdl_xml2) = minimal_csdl();

    registry.register_tenant("alpha", csdl1, csdl_xml1, &[("Order", ORDER_IOA)]);
    registry.register_tenant("beta", csdl2, csdl_xml2, &[("Task", ORDER_IOA)]);

    let a = TenantId::new("alpha");
    let b = TenantId::new("beta");

    // Each tenant sees only its own entities
    assert!(registry.get_table(&a, "Order").is_some());
    assert!(registry.get_table(&a, "Task").is_none());
    assert!(registry.get_table(&b, "Task").is_some());
    assert!(registry.get_table(&b, "Order").is_none());
}

#[test]
fn tenant_ids_listed() {
    let mut registry = SpecRegistry::new();
    let (csdl1, xml1) = minimal_csdl();
    let (csdl2, xml2) = minimal_csdl();

    registry.register_tenant("alpha", csdl1, xml1, &[]);
    registry.register_tenant("beta", csdl2, xml2, &[]);

    let ids: Vec<&str> = registry.tenant_ids().iter().map(|t| t.as_str()).collect();
    assert!(ids.contains(&"alpha"));
    assert!(ids.contains(&"beta"));
}

#[test]
fn entity_types_for_tenant() {
    let mut registry = SpecRegistry::new();
    let (csdl, xml) = minimal_csdl();

    registry.register_tenant("alpha", csdl, xml, &[("Order", ORDER_IOA)]);

    let types = registry.entity_types(&TenantId::new("alpha"));
    assert_eq!(types, vec!["Order"]);
}

#[test]
fn transition_table_is_functional() {
    let mut registry = SpecRegistry::new();
    let (csdl, xml) = minimal_csdl();

    registry.register_tenant("alpha", csdl, xml, &[("Order", ORDER_IOA)]);

    let table = registry
        .get_table(&TenantId::new("alpha"), "Order")
        .unwrap();
    assert_eq!(table.entity_name, "Order");
    assert_eq!(table.initial_state, "Draft");
    assert!(!table.rules.is_empty());

    // Verify it evaluates correctly
    let result = table.evaluate("Draft", 1, "SubmitOrder");
    assert!(result.is_some());
    assert!(result.unwrap().success);
}

#[test]
fn remove_tenant_succeeds() {
    let mut registry = SpecRegistry::new();
    let (csdl, xml) = minimal_csdl();

    registry.register_tenant("doomed", csdl, xml, &[("Order", ORDER_IOA)]);
    let tenant = TenantId::new("doomed");
    assert!(registry.get_tenant(&tenant).is_some());

    assert!(registry.remove_tenant(&tenant));
    assert!(registry.get_tenant(&tenant).is_none());
    assert!(registry.get_table(&tenant, "Order").is_none());
}

#[test]
fn remove_nonexistent_tenant_returns_false() {
    let mut registry = SpecRegistry::new();
    let tenant = TenantId::new("nonexistent");
    assert!(!registry.remove_tenant(&tenant));
}

#[test]
fn spec_metadata_accessible() {
    let mut registry = SpecRegistry::new();
    let (csdl, xml) = minimal_csdl();

    registry.register_tenant("alpha", csdl, xml, &[("Order", ORDER_IOA)]);

    let spec = registry.get_spec(&TenantId::new("alpha"), "Order").unwrap();
    assert_eq!(spec.automaton.automaton.name, "Order");
    assert!(!spec.ioa_source.is_empty());
}

/// Minimal CSDL with a single EntityType + EntitySet for merge tests.
fn task_csdl() -> (CsdlDocument, String) {
    let xml = r#"<?xml version="1.0"?>
        <edmx:Edmx Version="4.0" xmlns:edmx="http://docs.oasis-open.org/odata/ns/edmx">
          <edmx:DataServices>
            <Schema Namespace="Temper.Example" xmlns="http://docs.oasis-open.org/odata/ns/edm">
              <EntityType Name="Task">
                <Key><PropertyRef Name="Id"/></Key>
                <Property Name="Id" Type="Edm.Guid" Nullable="false"/>
              </EntityType>
              <EntityContainer Name="ExampleService">
                <EntitySet Name="Tasks" EntityType="Temper.Example.Task"/>
              </EntityContainer>
            </Schema>
          </edmx:DataServices>
        </edmx:Edmx>"#;
    (parse_csdl(xml).unwrap(), xml.to_string())
}

fn task_scope() -> SchemaScope {
    SchemaScope {
        kind: SchemaScopeKind::Task,
        id: "task-42".into(),
    }
}

#[test]
fn scoped_staging_is_invisible_until_atomic_activation() {
    let mut registry = SpecRegistry::new();
    let tenant = TenantId::new("alpha");
    let (csdl, xml) = task_csdl();
    registry
        .stage_scoped_bundle(
            tenant.clone(),
            task_scope(),
            "sha256:one".into(),
            csdl,
            xml,
            &[("Task", ORDER_IOA)],
        )
        .unwrap();

    assert!(registry.get_scoped_config(&tenant, &task_scope()).is_none());
    assert!(registry.get_tenant(&tenant).is_none());
    registry
        .activate_scoped_bundle(&tenant, &task_scope(), "sha256:one", None)
        .unwrap();
    assert_eq!(
        registry.resolve_scoped_entity_type(&tenant, &task_scope(), "Tasks"),
        Some("Task".into())
    );
    assert!(
        registry
            .get_scoped_table(&tenant, &task_scope(), "Task")
            .is_some()
    );
}

#[test]
fn scoped_modules_resolve_only_for_the_exact_tenant_scope_and_digest() {
    let mut registry = SpecRegistry::new();
    let tenant = TenantId::new("alpha");
    let scope = task_scope();
    let digest = "sha256:bundle-one";
    let descriptor = ScopedModuleDescriptor {
        artifact_digest: format!("sha256:{}", "a".repeat(64)),
        data_binding: None,
    };
    let (csdl, xml) = task_csdl();
    registry
        .stage_scoped_bundle_with_modules(
            tenant.clone(),
            scope.clone(),
            digest.into(),
            csdl,
            xml,
            &[("Task", ORDER_IOA)],
            BTreeMap::from([("worker".into(), descriptor.clone())]),
        )
        .unwrap();

    assert_eq!(
        registry.get_scoped_module_at_digest(&tenant, &scope, digest, "worker"),
        Some(&descriptor)
    );
    assert!(
        registry
            .get_scoped_module_at_digest(&TenantId::new("beta"), &scope, digest, "worker")
            .is_none()
    );
    let other_scope = SchemaScope {
        kind: SchemaScopeKind::Task,
        id: "task-43".into(),
    };
    assert!(
        registry
            .get_scoped_module_at_digest(&tenant, &other_scope, digest, "worker")
            .is_none()
    );
    assert!(
        registry
            .get_scoped_module_at_digest(&tenant, &scope, "sha256:other", "worker")
            .is_none()
    );
}

#[test]
fn scoped_cedar_policies_resolve_only_for_the_exact_immutable_bundle() {
    let mut registry = SpecRegistry::new();
    let tenant = TenantId::new("alpha");
    let scope = task_scope();
    let digest = "sha256:bundle-one";
    registry
        .stage_scoped_cedar_policies(
            tenant.clone(),
            scope.clone(),
            digest.into(),
            BTreeMap::from([(
                "resume".into(),
                "permit(principal, action == Action::\"Resume\", resource);".into(),
            )]),
        )
        .unwrap();

    assert!(
        registry
            .scoped_cedar_policy_at_digest(&tenant, &scope, digest)
            .unwrap()
            .contains("Action::\"Resume\"")
    );
    assert!(
        registry
            .scoped_cedar_policy_at_digest(&tenant, &scope, "sha256:other")
            .is_none()
    );
    assert!(
        registry
            .scoped_cedar_policy_at_digest(
                &tenant,
                &SchemaScope {
                    kind: SchemaScopeKind::Task,
                    id: "task-43".into(),
                },
                digest,
            )
            .is_none()
    );
}

#[test]
fn scoped_activation_rejects_stale_predecessor_without_changing_reader() {
    let mut registry = SpecRegistry::new();
    let tenant = TenantId::new("alpha");
    for digest in ["sha256:one", "sha256:two"] {
        let (csdl, xml) = task_csdl();
        registry
            .stage_scoped_bundle(
                tenant.clone(),
                task_scope(),
                digest.into(),
                csdl,
                xml,
                &[("Task", ORDER_IOA)],
            )
            .unwrap();
    }
    registry
        .activate_scoped_bundle(&tenant, &task_scope(), "sha256:one", None)
        .unwrap();
    assert!(matches!(
        registry.activate_scoped_bundle(&tenant, &task_scope(), "sha256:two", Some("sha256:stale")),
        Err(RegistryError::ScopedPredecessorMismatch { .. })
    ));
    assert_eq!(
        registry.active_scope_digest(&tenant, &task_scope()),
        Some("sha256:one")
    );
}

#[test]
fn scoped_retirement_blocks_new_resolution_but_preserves_exact_pinned_table() {
    let tenant = TenantId::new("tenant-retire");
    let mut registry = SpecRegistry::new();
    let (csdl, csdl_xml) = task_csdl();
    registry
        .stage_scoped_bundle(
            tenant.clone(),
            task_scope(),
            "sha256:retired".into(),
            csdl,
            csdl_xml,
            &[("Task", ORDER_IOA)],
        )
        .unwrap();
    registry
        .activate_scoped_bundle(&tenant, &task_scope(), "sha256:retired", None)
        .unwrap();
    registry
        .retire_scoped_bundle(&tenant, &task_scope(), "sha256:retired")
        .unwrap();

    assert!(registry.get_scoped_config(&tenant, &task_scope()).is_none());
    assert!(
        registry
            .get_scoped_table_at_digest(&tenant, &task_scope(), "sha256:retired", "Task",)
            .is_some(),
        "retirement must retain immutable artifacts for existing pins"
    );
}

#[test]
fn scoped_reaction_candidates_come_only_from_exact_bundle_digest() {
    const SCOPED_REACTION_IOA: &str = r#"
[automaton]
name = "Task"
states = ["Open"]
initial = "Open"

[[action]]
name = "Advance"
kind = "input"
from = ["Open"]
to = "Open"

[[action.triggers]]
name = "scoped_followup"
kind = "entity"
target_entity = "Task"
target_action = "Advance"

[action.triggers.resolve_target]
type = "create"
"#;
    let tenant = TenantId::new("tenant-reactions");
    let mut registry = SpecRegistry::new();
    let (csdl, csdl_xml) = task_csdl();
    registry
        .stage_scoped_bundle(
            tenant.clone(),
            task_scope(),
            "sha256:scoped-reactions".into(),
            csdl,
            csdl_xml,
            &[("Task", SCOPED_REACTION_IOA)],
        )
        .unwrap();

    let rules = registry.scoped_reaction_candidates_at_digest(
        &tenant,
        &task_scope(),
        "sha256:scoped-reactions",
        "Task",
        "Advance",
    );
    assert_eq!(rules.len(), 1);
    assert_eq!(rules[0].name, "Task:Advance:scoped_followup");
    assert!(
        registry
            .scoped_reaction_candidates_at_digest(
                &tenant,
                &task_scope(),
                "sha256:other",
                "Task",
                "Advance",
            )
            .is_empty()
    );
}

#[test]
fn global_compatibility_requires_explicit_scope_creation_choice() {
    let mut registry = SpecRegistry::new();
    let tenant = TenantId::new("alpha");
    let (csdl, xml) = task_csdl();
    registry.register_tenant(tenant.clone(), csdl, xml, &[("Task", ORDER_IOA)]);
    assert!(!registry.scope_allows_global_compatibility(&tenant, &task_scope()));
    assert!(
        registry
            .resolve_scoped_entity_type(&tenant, &task_scope(), "Tasks")
            .is_none()
    );
    registry.set_scope_global_compatibility(tenant.clone(), task_scope(), true);
    assert!(registry.scope_allows_global_compatibility(&tenant, &task_scope()));
}

#[test]
fn merge_preserves_existing_entities_and_entity_set_map() {
    let mut registry = SpecRegistry::new();
    let (csdl, xml) = minimal_csdl();
    registry.register_tenant("alpha", csdl, xml, &[("Order", ORDER_IOA)]);
    let tenant = TenantId::new("alpha");

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

    assert!(
        registry.get_table(&tenant, "Order").is_some(),
        "Order survives merge"
    );
    assert!(
        registry.get_table(&tenant, "Task").is_some(),
        "Task added by merge"
    );

    let config = registry.get_tenant(&tenant).unwrap();
    assert!(config.entity_set_map.contains_key("Orders"));
    assert!(config.entity_set_map.contains_key("Tasks"));
    assert!(matches!(
        config.verification.get("Task"),
        Some(VerificationStatus::Pending)
    ));
}

#[test]
fn csdl_formatting_only_reload_preserves_global_schema_digest() {
    let mut registry = SpecRegistry::new();
    let (csdl, xml) = minimal_csdl();
    registry.register_tenant("alpha", csdl, xml.clone(), &[("Order", ORDER_IOA)]);
    let tenant = TenantId::new("alpha");
    let before = registry
        .get_table(&tenant, "Order")
        .and_then(|table| table.schema_digest.clone())
        .expect("registered table must carry full schema identity");

    let (csdl, _) = minimal_csdl();
    registry
        .try_register_tenant_with_reactions_and_constraints(
            "alpha",
            csdl,
            format!("{xml}<!-- compatible CSDL metadata change -->"),
            &[("Order", ORDER_IOA)],
            Vec::new(),
            None,
            false,
        )
        .expect("CSDL-only replacement should succeed");
    let after = registry
        .get_table(&tenant, "Order")
        .and_then(|table| table.schema_digest.clone())
        .expect("reloaded table must carry full schema identity");

    assert_eq!(before, after);
}

mod hot_reload;
