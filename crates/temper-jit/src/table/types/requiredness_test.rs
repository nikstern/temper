use std::collections::BTreeMap;

use super::TransitionTable;

#[test]
fn compiled_action_contract_rejects_missing_and_null() {
    let table = TransitionTable::from_ioa_source(
        r#"
[automaton]
name = "Task"
states = ["Open"]
initial = "Open"

[[action]]
name = "Assign"
kind = "input"
from = ["Open"]
params = [
  { name = "agent_id", type = "Edm.String" },
  { name = "note", type = "Edm.String", nullable = true },
]
"#,
    );

    for input in [serde_json::json!({}), serde_json::json!({"AgentId": null})] {
        let error = table
            .validate_required_action_params("Assign", &input)
            .unwrap_err();
        assert_eq!(error.code, "MissingActionParameter");
    }
    for input in [
        serde_json::json!({"AgentId": "a"}),
        serde_json::json!({"AgentId": 3}),
        serde_json::json!({"AgentId": ""}),
        serde_json::json!({"AgentId": "a", "agent_id": "a"}),
        serde_json::json!({"agent_id": "a", "note": null}),
        serde_json::json!({"agent_id": "a", "note": "ok"}),
    ] {
        table
            .validate_required_action_params("Assign", &input)
            .unwrap();
    }
}

#[test]
fn action_parameter_aliases_gain_the_exact_ioa_key() {
    let table = TransitionTable::from_ioa_source(
        r#"
[automaton]
name = "Counter"
states = ["Ready"]
initial = "Ready"

[[action]]
name = "Adjust"
from = ["Ready"]
params = [{ name = "delta_value", type = "Edm.Int64" }]
"#,
    );
    let normalized =
        table.canonicalize_action_params("Adjust", &serde_json::json!({"DeltaValue": 4}));
    assert_eq!(normalized["delta_value"], 4);
    assert_eq!(normalized["DeltaValue"], 4);
}

#[test]
fn older_serialized_table_without_action_metadata_remains_readable() {
    let json = r#"{
      "entity_name":"Order",
      "states":["Draft","Cancelled"],
      "initial_state":"Draft",
      "rules":[{
        "name":"CancelOrder",
        "from_states":["Draft"],
        "to_state":"Cancelled",
        "guard":"Always",
        "effects":[{"SetState":"Cancelled"}]
      }],
      "keys":[],
      "vectors":[],
      "state_var_metadata":{},
      "composite_actions":{}
    }"#;
    let table: TransitionTable = serde_json::from_str(json).expect("old table fixture");
    assert!(table.action_params.is_empty());
    let error = table
        .validate_required_action_params("CancelOrder", &serde_json::json!({}))
        .unwrap_err();
    assert_eq!(error.code, "MissingActionParameter");
}

#[test]
fn current_parameterless_action_has_explicit_empty_metadata() {
    let table = TransitionTable::from_ioa_source(
        r#"
[automaton]
name = "Task"
states = ["Open", "Closed"]
initial = "Open"

[[action]]
name = "Close"
from = ["Open"]
to = "Closed"
"#,
    );
    assert_eq!(table.action_params.get("Close"), Some(&BTreeMap::new()));
    table
        .validate_required_action_params("Close", &serde_json::json!({}))
        .expect("current parameterless actions carry explicit metadata");
}
