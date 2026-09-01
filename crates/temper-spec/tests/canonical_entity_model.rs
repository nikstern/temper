use temper_spec::bundle::{
    IoaSourceInput, SCOPED_SPEC_BUNDLE_CONTRACT_V1, SCOPED_SPEC_BUNDLE_CONTRACT_V2,
    ScopedBundleBudgets, ScopedSpecBundle, ScopedSpecBundleInput,
};

const IOA: &str = r#"
[automaton]
name = "Task"
states = ["Draft", "Ready", "Done"]
initial = "Draft"
lifecycle_property = "State"

[[action]]
name = "Advance"
kind = "input"
from = ["Draft", "Ready"]
to = "Done"
guard = [{ type = "state_in", values = ["Ready", "Done"] }]
params = [{ name = "Count", type = "counter" }]
"#;

const STRUCTURAL_CSDL: &str = r#"
<edmx:Edmx Version="4.0" xmlns:edmx="http://docs.oasis-open.org/odata/ns/edmx">
  <edmx:DataServices>
    <Schema Namespace="Example" xmlns="http://docs.oasis-open.org/odata/ns/edm">
      <EnumType Name="TaskState"/>
      <EntityType Name="Task">
        <Key><PropertyRef Name="Id"/></Key>
        <Property Name="Id" Type="Edm.Guid" Nullable="false"/>
        <Property Name="State" Type="Example.TaskState" Nullable="false"/>
      </EntityType>
      <EntityType Name="Audit">
        <Key><PropertyRef Name="Id"/></Key>
        <Property Name="Id" Type="Edm.Guid" Nullable="false"/>
      </EntityType>
      <Action Name="Advance" IsBound="true">
        <Parameter Name="binding" Type="Example.Task" Nullable="false"/>
        <Parameter Name="count" Type="Edm.Int64" Nullable="false"/>
      </Action>
      <EntityContainer Name="Service">
        <EntitySet Name="Tasks" EntityType="Example.Task"/>
        <EntitySet Name="Audits" EntityType="Example.Audit"/>
      </EntityContainer>
    </Schema>
  </edmx:DataServices>
</edmx:Edmx>
"#;

fn input(csdl: &str, ioa: &str) -> ScopedSpecBundleInput {
    ScopedSpecBundleInput {
        scope_id: "task-91".into(),
        predecessor_digest: None,
        csdl_xml: csdl.into(),
        ioa_sources: vec![IoaSourceInput {
            entity_type: "Example.Task".into(),
            source: ioa.into(),
        }],
        cedar_policies: vec![],
        wasm_modules: vec![],
        migration: None,
        budgets: ScopedBundleBudgets::default(),
    }
}

#[test]
fn v2_builds_one_ioa_ordered_model_and_preserves_data_only_entities() {
    let bundle = ScopedSpecBundle::compile(input(STRUCTURAL_CSDL, IOA)).unwrap();
    assert_eq!(
        bundle.canonicalization_version(),
        SCOPED_SPEC_BUNDLE_CONTRACT_V2
    );
    let model = bundle.canonical_model().unwrap();
    let task = model.behavioral_entity("Example.Task").unwrap();
    assert_eq!(task.lifecycle_property(), Some("State"));
    assert_eq!(task.lifecycle_states(), ["Draft", "Ready", "Done"]);
    assert_eq!(
        task.actions()["Advance"].valid_from_states(),
        ["Ready"],
        "from and state_in intersect in IOA declaration order"
    );
    assert!(model.entities()["Example.Audit"].automaton().is_none());
    assert!(bundle.canonical_csdl().contains("DefaultValue=\"Draft\""));
    assert!(
        bundle
            .canonical_csdl()
            .contains("<Member Name=\"Draft\" Value=\"0\"/>")
    );
    assert!(
        bundle
            .canonical_csdl()
            .contains("<Member Name=\"Ready\" Value=\"1\"/>")
    );
    assert!(
        bundle
            .canonical_csdl()
            .contains("<Member Name=\"Done\" Value=\"2\"/>")
    );
}

#[test]
fn generated_v2_csdl_is_idempotent() {
    let first = ScopedSpecBundle::compile(input(STRUCTURAL_CSDL, IOA)).unwrap();
    let second = ScopedSpecBundle::compile(input(first.canonical_csdl(), IOA)).unwrap();
    assert_eq!(first.canonical_csdl(), second.canonical_csdl());
    assert_eq!(first.digest(), second.digest());
}

#[test]
fn matching_partial_legacy_projection_converges_and_contradictions_fail() {
    let matching = STRUCTURAL_CSDL.replace(
        "<EntityType Name=\"Task\">",
        "<EntityType Name=\"Task\"><Annotation Term=\"Temper.Vocab.StateMachine.States\"><Collection><String>Ready</String></Collection></Annotation>",
    );
    let plain = ScopedSpecBundle::compile(input(STRUCTURAL_CSDL, IOA)).unwrap();
    let legacy = ScopedSpecBundle::compile(input(&matching, IOA)).unwrap();
    assert_eq!(plain.digest(), legacy.digest());

    let contradictory = matching.replace("<String>Ready</String>", "<String>Unknown</String>");
    let error = ScopedSpecBundle::compile(input(&contradictory, IOA)).unwrap_err();
    assert!(error.to_string().contains("contradicts IOA"));
}

#[test]
fn v2_requires_explicit_lifecycle_property_but_v1_remains_readable() {
    let legacy_ioa = IOA.replace("lifecycle_property = \"State\"\n", "");
    let error = ScopedSpecBundle::compile(input(STRUCTURAL_CSDL, &legacy_ioa)).unwrap_err();
    assert!(error.to_string().contains("automaton.lifecycle_property"));
    let legacy = ScopedSpecBundle::compile_v1(input(STRUCTURAL_CSDL, &legacy_ioa)).unwrap();
    assert_eq!(
        legacy.canonicalization_version(),
        SCOPED_SPEC_BUNDLE_CONTRACT_V1
    );
    assert!(legacy.canonical_model().is_none());
}

#[test]
fn exact_bound_action_parity_and_semantic_parameter_types_fail_closed() {
    let missing = STRUCTURAL_CSDL.replace(
        r#"      <Action Name="Advance" IsBound="true">
        <Parameter Name="binding" Type="Example.Task" Nullable="false"/>
        <Parameter Name="count" Type="Edm.Int64" Nullable="false"/>
      </Action>
"#,
        "",
    );
    assert!(
        ScopedSpecBundle::compile(input(&missing, IOA))
            .unwrap_err()
            .to_string()
            .contains("action parity mismatch")
    );

    let wrong_type = STRUCTURAL_CSDL.replace("Type=\"Edm.Int64\"", "Type=\"Edm.Boolean\"");
    assert!(
        ScopedSpecBundle::compile(input(&wrong_type, IOA))
            .unwrap_err()
            .to_string()
            .contains("incompatible with CSDL type")
    );
}

#[test]
fn v2_identity_ignores_source_order_and_formatting() {
    let formatted_csdl = STRUCTURAL_CSDL
        .replace(
            "<EntityType Name=\"Audit\">",
            "\n\n<EntityType Name=\"Audit\">",
        )
        .replace("<Property Name=\"Id\"", "<Property   Name=\"Id\"");
    let formatted_ioa = IOA.replace("states =", "states   =");
    let plain = ScopedSpecBundle::compile(input(STRUCTURAL_CSDL, IOA)).unwrap();
    let formatted = ScopedSpecBundle::compile(input(&formatted_csdl, &formatted_ioa)).unwrap();
    assert_eq!(plain.canonical_csdl(), formatted.canonical_csdl());
    assert_eq!(plain.digest(), formatted.digest());
}

#[test]
fn lifecycle_enum_must_be_dedicated_and_shared_in_identical_ioa_order() {
    let unrelated = STRUCTURAL_CSDL.replace(
        "<Property Name=\"Id\" Type=\"Edm.Guid\" Nullable=\"false\"/>\n      </EntityType>",
        "<Property Name=\"Id\" Type=\"Edm.Guid\" Nullable=\"false\"/>\n        <Property Name=\"RecordedState\" Type=\"Example.TaskState\" Nullable=\"false\"/>\n      </EntityType>",
    );
    assert!(
        ScopedSpecBundle::compile(input(&unrelated, IOA))
            .unwrap_err()
            .to_string()
            .contains("unrelated property")
    );

    let shared_csdl = STRUCTURAL_CSDL.replace(
        "<Property Name=\"Id\" Type=\"Edm.Guid\" Nullable=\"false\"/>\n      </EntityType>",
        "<Property Name=\"Id\" Type=\"Edm.Guid\" Nullable=\"false\"/>\n        <Property Name=\"State\" Type=\"Example.TaskState\" Nullable=\"false\"/>\n      </EntityType>",
    );
    let audit = r#"[automaton]
name = "Audit"
states = ["Draft", "Ready", "Done"]
initial = "Draft"
lifecycle_property = "State"
"#;
    let mut shared_input = input(&shared_csdl, IOA);
    shared_input.ioa_sources.push(IoaSourceInput {
        entity_type: "Example.Audit".into(),
        source: audit.into(),
    });
    ScopedSpecBundle::compile(shared_input.clone()).unwrap();
    shared_input.ioa_sources[1].source = audit.replace(
        "[\"Draft\", \"Ready\", \"Done\"]",
        "[\"Ready\", \"Draft\", \"Done\"]",
    );
    assert!(
        ScopedSpecBundle::compile(shared_input)
            .unwrap_err()
            .to_string()
            .contains("incompatible IOA state order")
    );
}

#[test]
fn duplicate_short_names_across_namespaces_remain_fully_qualified() {
    let csdl = STRUCTURAL_CSDL.replace(
        "</edmx:DataServices>",
        r#"<Schema Namespace="Other" xmlns="http://docs.oasis-open.org/odata/ns/edm"><EntityType Name="Task"><Key><PropertyRef Name="Id"/></Key><Property Name="Id" Type="Edm.Guid" Nullable="false"/><Property Name="State" Type="Edm.String" Nullable="false"/></EntityType></Schema></edmx:DataServices>"#,
    );
    let mut bundle_input = input(&csdl, IOA);
    bundle_input.ioa_sources.push(IoaSourceInput {
        entity_type: "Other.Task".into(),
        source: r#"[automaton]
name = "Task"
states = ["Draft", "Ready", "Done"]
initial = "Draft"
lifecycle_property = "State"
"#
        .into(),
    });
    let bundle = ScopedSpecBundle::compile(bundle_input).unwrap();
    let model = bundle.canonical_model().unwrap();
    assert!(model.behavioral_entity("Example.Task").is_some());
    assert!(model.behavioral_entity("Other.Task").is_some());
}

#[test]
fn actions_declared_in_another_schema_receive_the_canonical_projection() {
    let action = r#"      <Action Name="Advance" IsBound="true">
        <Parameter Name="binding" Type="Example.Task" Nullable="false"/>
        <Parameter Name="count" Type="Edm.Int64" Nullable="false"/>
      </Action>
"#;
    let csdl = STRUCTURAL_CSDL.replace(action, "").replace(
        "</edmx:DataServices>",
        &format!(
            r#"<Schema Namespace="Operations" xmlns="http://docs.oasis-open.org/odata/ns/edm">
{action}</Schema></edmx:DataServices>"#
        ),
    );

    let first = ScopedSpecBundle::compile(input(&csdl, IOA)).unwrap();
    assert!(
        first
            .canonical_csdl()
            .contains("Temper.Vocab.StateMachine.ValidFromStates")
    );
    assert!(
        first
            .canonical_csdl()
            .contains("Temper.Vocab.StateMachine.TargetState")
    );
    let second = ScopedSpecBundle::compile(input(first.canonical_csdl(), IOA)).unwrap();
    assert_eq!(first.canonical_csdl(), second.canonical_csdl());
    assert_eq!(first.digest(), second.digest());
}

#[test]
fn duplicate_behavior_annotations_and_reordered_implicit_enum_members_fail() {
    let duplicate_initial = STRUCTURAL_CSDL.replace(
        "<EntityType Name=\"Task\">",
        r#"<EntityType Name="Task">
        <Annotation Term="Temper.Vocab.StateMachine.InitialState" String="Draft"/>
        <Annotation Term="Temper.Vocab.StateMachine.InitialState" String="Done"/>"#,
    );
    assert!(
        ScopedSpecBundle::compile(input(&duplicate_initial, IOA))
            .unwrap_err()
            .to_string()
            .contains("duplicate behavioral annotation")
    );

    let reordered = STRUCTURAL_CSDL.replace(
        "<EnumType Name=\"TaskState\"/>",
        r#"<EnumType Name="TaskState"><Member Name="Done"/><Member Name="Draft"/></EnumType>"#,
    );
    assert!(
        ScopedSpecBundle::compile(input(&reordered, IOA))
            .unwrap_err()
            .to_string()
            .contains("contradicts IOA")
    );
}

#[test]
fn unrelated_same_suffix_annotations_are_preserved() {
    let csdl = STRUCTURAL_CSDL.replace(
        "<EntityType Name=\"Task\">",
        r#"<EntityType Name="Task"><Annotation Term="Acme.StateMachine.States" String="External"/>"#,
    );
    let bundle = ScopedSpecBundle::compile(input(&csdl, IOA)).unwrap();

    assert!(
        bundle
            .canonical_csdl()
            .contains("Term=\"Acme.StateMachine.States\" String=\"External\"")
    );
    assert!(
        bundle
            .canonical_csdl()
            .contains("Term=\"Temper.Vocab.StateMachine.States\"")
    );
}
