use temper_jit::TransitionTable;
use temper_spec::{CanonicalSpecModel, IoaSourceInput, csdl::parse_csdl};
use temper_verify::VerificationCascade;

const CSDL: &str = r#"
<edmx:Edmx Version="4.0" xmlns:edmx="http://docs.oasis-open.org/odata/ns/edmx">
  <edmx:DataServices>
    <Schema Namespace="Parity" xmlns="http://docs.oasis-open.org/odata/ns/edm">
      <EntityType Name="Task">
        <Key><PropertyRef Name="Id"/></Key>
        <Property Name="Id" Type="Edm.Guid" Nullable="false"/>
        <Property Name="State" Type="Edm.String" Nullable="false"/>
      </EntityType>
      <Action Name="Advance" IsBound="true">
        <Parameter Name="binding" Type="Parity.Task" Nullable="false"/>
      </Action>
    </Schema>
  </edmx:DataServices>
</edmx:Edmx>
"#;

const IOA: &str = r#"
[automaton]
name = "Task"
states = ["Draft", "Done"]
initial = "Draft"
lifecycle_property = "State"

[[action]]
name = "Advance"
kind = "input"
from = ["Draft"]
to = "Done"
"#;

#[test]
fn jit_and_verification_share_the_canonical_parsed_automaton() {
    let document = parse_csdl(CSDL).unwrap();
    let model = CanonicalSpecModel::link_v2_sources(
        &document,
        &[IoaSourceInput {
            entity_type: "Parity.Task".into(),
            source: IOA.into(),
        }],
    )
    .unwrap();
    let automaton = model
        .behavioral_entity("Parity.Task")
        .and_then(|entity| entity.automaton())
        .unwrap();

    let table = TransitionTable::from_automaton(automaton);
    assert_eq!(table.states, automaton.automaton.states);
    assert_eq!(table.initial_state, automaton.automaton.initial);
    assert_eq!(table.rules.len(), 1);
    assert_eq!(table.rules[0].name, automaton.actions[0].name);
    assert_eq!(table.rules[0].from_states, automaton.actions[0].from);
    assert_eq!(table.rules[0].to_state, automaton.actions[0].to);

    let result = VerificationCascade::from_automaton(automaton)
        .with_sim_seeds(2)
        .with_sim_ticks(20)
        .with_prop_test_cases(20)
        .run();
    assert!(
        result.all_passed,
        "canonical verification failed: {result:?}"
    );
}
