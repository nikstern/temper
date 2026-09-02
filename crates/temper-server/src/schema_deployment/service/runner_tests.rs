use serde_json::json;

use chrono::{TimeZone, Utc};
use temper_runtime::persistence::schema_deployment::{
    SchemaEventPin, SchemaExecutionPin, SchemaScope, SchemaScopeKind,
};

use super::runner::collapse_runtime_alias;
use super::timeout_intents::{MigratedTimeoutContext, attach_migrated_state_timeout_intents};

const MIGRATED_TIMEOUT_IOA: &str = r#"
[automaton]
name = "ArcSynthesisRun"
states = ["Draft", "ResumeReady", "Completed"]
initial = "Draft"

[[action]]
name = "Resume"
kind = "input"
from = ["ResumeReady"]
to = "Completed"

[[state_timeout]]
state = "ResumeReady"
after_seconds = 1
on_timeout = "Resume"
max_occurrences = 1
"#;

#[test]
fn migrated_state_entry_co_commits_its_timeout_intent() {
    let table = temper_jit::table::TransitionTable::from_ioa_source(MIGRATED_TIMEOUT_IOA);
    let pin = SchemaExecutionPin {
        scope: SchemaScope {
            kind: SchemaScopeKind::Task,
            id: "arc-1".into(),
        },
        bundle_digest: format!("sha256:{}", "a".repeat(64)),
    };
    let schema_pin = SchemaEventPin {
        execution: pin,
        action_digest: format!("sha256:{}", "b".repeat(64)),
    };
    let event = crate::entity_actor::EntityEvent {
        action: crate::entity_actor::types::FIELD_UPDATE_EVENT_TYPE.into(),
        from_status: "Draft".into(),
        to_status: "ResumeReady".into(),
        timestamp: Utc.timestamp_opt(1_800_000_000, 0).single().unwrap(),
        params: json!({"replace": true, "migration": true}),
        idempotency_key: Some("migration-1".into()),
    };
    let mut payload = serde_json::to_value(&event).unwrap();

    let source_fields = json!({"id": "run-1", "status": "ResumeReady"});
    attach_migrated_state_timeout_intents(
        &mut payload,
        MigratedTimeoutContext {
            tenant: "tenant-a",
            entity_type: "ArcSynthesisRun",
            entity_id: "run-1",
            source_sequence: 1,
            event: &event,
            source_fields: &source_fields,
            table: &table,
            schema_pin,
        },
    )
    .expect("migration state entry should schedule its declared timeout");

    let intents = crate::trigger::delivery::extract_intents(&payload).unwrap();
    assert_eq!(intents.len(), 1);
    assert_eq!(
        intents[0].kind,
        crate::trigger::delivery::DeliveryKind::StateTimeout
    );
    assert_eq!(intents[0].source_sequence, 1);
    assert_eq!(
        intents[0].state_timeout.as_ref().unwrap().state,
        "ResumeReady"
    );
}

#[test]
fn migration_boundary_retains_only_the_snake_case_runtime_name() {
    let mut fields = json!({
        "Id": "task-1",
        "id": "task-1",
        "Status": "Ready",
        "status": "Ready"
    })
    .as_object()
    .expect("fixture is an object")
    .clone();

    collapse_runtime_alias(&mut fields, "Id", "id").expect("matching identity aliases");
    collapse_runtime_alias(&mut fields, "Status", "status").expect("matching lifecycle aliases");

    assert_eq!(fields.get("id"), Some(&json!("task-1")));
    assert_eq!(fields.get("status"), Some(&json!("Ready")));
    assert!(!fields.contains_key("Id"));
    assert!(!fields.contains_key("Status"));
}

#[test]
fn migration_boundary_renames_a_pascal_only_runtime_field() {
    let mut fields = json!({"Id": "task-1"})
        .as_object()
        .expect("fixture is an object")
        .clone();

    collapse_runtime_alias(&mut fields, "Id", "id").expect("legacy identity is canonicalized");

    assert_eq!(fields.get("id"), Some(&json!("task-1")));
    assert!(!fields.contains_key("Id"));
}

#[test]
fn migration_boundary_rejects_disagreeing_runtime_aliases() {
    let mut fields = json!({"Id": "task-1", "id": "task-2"})
        .as_object()
        .expect("fixture is an object")
        .clone();

    let error = collapse_runtime_alias(&mut fields, "Id", "id")
        .expect_err("disagreeing identity aliases must fail");

    assert_eq!(error.code(), "migration_rejected");
}
