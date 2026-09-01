use super::*;
use temper_runtime::persistence::schema_deployment::{SchemaScope, SchemaScopeKind};
use temper_spec::csdl::parse_csdl;

const IOA: &str = r#"[automaton]
name = "Task"
states = ["Open", "Done"]
initial = "Open"
lifecycle_property = "State"
[[action]]
name = "Advance"
kind = "input"
from = ["Open"]
to = "Done"
"#;

fn scope() -> SchemaScope {
    SchemaScope {
        kind: SchemaScopeKind::Task,
        id: "task-42".into(),
    }
}

#[test]
fn scoped_v2_preserves_equal_short_names_across_namespaces() {
    let xml = r#"<edmx:Edmx Version="4.0" xmlns:edmx="http://docs.oasis-open.org/odata/ns/edmx"><edmx:DataServices><Schema Namespace="Example" xmlns="http://docs.oasis-open.org/odata/ns/edm"><EntityType Name="Task"><Key><PropertyRef Name="Id"/></Key><Property Name="Id" Type="Edm.String" Nullable="false"/><Property Name="State" Type="Edm.String" Nullable="false"/></EntityType><Action Name="Advance" IsBound="true"><Parameter Name="binding" Type="Example.Task" Nullable="false"/></Action><EntityContainer Name="One"><EntitySet Name="ExampleTasks" EntityType="Example.Task"/></EntityContainer></Schema><Schema Namespace="Other" xmlns="http://docs.oasis-open.org/odata/ns/edm"><EntityType Name="Task"><Key><PropertyRef Name="Id"/></Key><Property Name="Id" Type="Edm.String" Nullable="false"/><Property Name="State" Type="Edm.String" Nullable="false"/></EntityType><Action Name="Advance" IsBound="true"><Parameter Name="binding" Type="Other.Task" Nullable="false"/></Action><EntityContainer Name="Two"><EntitySet Name="OtherTasks" EntityType="Other.Task"/></EntityContainer></Schema></edmx:DataServices></edmx:Edmx>"#;
    let mut registry = SpecRegistry::new();
    let tenant = TenantId::new("alpha");
    registry
        .stage_scoped_bundle_v2(
            tenant.clone(),
            scope(),
            "sha256:namespaces".into(),
            parse_csdl(xml).unwrap(),
            xml.into(),
            &[("Example.Task", IOA), ("Other.Task", IOA)],
        )
        .unwrap();
    registry
        .activate_scoped_bundle(&tenant, &scope(), "sha256:namespaces", None)
        .unwrap();

    assert_eq!(
        registry.resolve_scoped_entity_type(&tenant, &scope(), "ExampleTasks"),
        Some("Example.Task".into())
    );
    assert_eq!(
        registry.resolve_scoped_entity_type(&tenant, &scope(), "OtherTasks"),
        Some("Other.Task".into())
    );
    assert!(
        registry
            .get_scoped_table(&tenant, &scope(), "Example.Task")
            .is_some()
    );
    assert!(
        registry
            .get_scoped_table(&tenant, &scope(), "Other.Task")
            .is_some()
    );
    assert!(
        registry
            .get_scoped_table(&tenant, &scope(), "Task")
            .is_none()
    );
}

#[test]
fn persisted_v1_staging_retains_exact_metadata_bytes() {
    let xml = r#"<edmx:Edmx Version="4.0" xmlns:edmx="http://docs.oasis-open.org/odata/ns/edmx"><edmx:DataServices><Schema Namespace="Example" xmlns="http://docs.oasis-open.org/odata/ns/edm"><EnumType Name="Reserved"/><EntityType Name="Task"><Key><PropertyRef Name="Id"/></Key><Property Name="Id" Type="Edm.String" Nullable="false"/><Property Name="State" Type="Edm.String" Nullable="false"/><Annotation Term="Example.Empty"><Collection/></Annotation></EntityType><Action Name="Advance" IsBound="true"><Parameter Name="binding" Type="Example.Task" Nullable="false"/></Action><EntityContainer Name="One"><EntitySet Name="Tasks" EntityType="Example.Task"/></EntityContainer></Schema></edmx:DataServices></edmx:Edmx>"#;
    let mut registry = SpecRegistry::new();
    let tenant = TenantId::new("alpha");
    registry
        .stage_scoped_bundle_persisted_v1_with_modules(
            tenant.clone(),
            scope(),
            "sha256:v1".into(),
            parse_csdl(xml).unwrap(),
            xml.into(),
            &[("Task", IOA)],
            BTreeMap::new(),
        )
        .unwrap();
    registry
        .activate_scoped_bundle(&tenant, &scope(), "sha256:v1", None)
        .unwrap();

    assert_eq!(
        registry
            .get_scoped_config(&tenant, &scope())
            .unwrap()
            .csdl_xml
            .as_str(),
        xml
    );
}
