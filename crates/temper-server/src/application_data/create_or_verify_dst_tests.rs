use std::collections::BTreeSet;

use temper_authz::SecurityContext;
use temper_runtime::tenant::TenantId;
use temper_store_sim::{DeterministicRng, SimEventStore, SimFaultConfig};
use temper_wasm_sdk::data::{DataOperationKind, DataOutcomeV1};

use super::create_or_verify_tests::{durable_invocation_with_store, operation};
use super::tests::call;

const IDS: [&str; 3] = [
    "018f1f80-7b2d-7000-8000-0000000000a1",
    "018f1f80-7b2d-7000-8000-0000000000a2",
    "018f1f80-7b2d-7000-8000-0000000000a3",
];

async fn run_generated_history(seed: u64) -> Vec<String> {
    let store = SimEventStore::new(
        seed,
        SimFaultConfig {
            write_failure_prob: 0.2,
            create_or_verify_reply_loss_prob: 0.3,
            ..SimFaultConfig::none()
        },
    );
    let operations = BTreeSet::from([DataOperationKind::EntityCreateOrVerify]);
    let mut invocation =
        durable_invocation_with_store(operations.clone(), SecurityContext::system(), store.clone());
    let mut generator = DeterministicRng::new(seed ^ 0x82_c0_fe);
    let mut trace = Vec::new();

    for step in 0..24 {
        match generator.next_u64() % 7 {
            0 | 1 => {
                let slot = (generator.next_u64() as usize) % IDS.len();
                let name = if generator.next_u64().is_multiple_of(3) {
                    "Grace"
                } else {
                    "Ada"
                };
                let response = call(
                    &invocation,
                    operation(IDS[slot], &format!("seed-{seed}-slot-{slot}"), name),
                )
                .await;
                let outcome = outcome_tag(response.outcome);
                trace.push(format!("{step}:create:{slot}:{outcome}"));
                if outcome == "err" {
                    let persistence_id = format!("default:Customer:{}", IDS[slot]);
                    let boundary = if store.dump_journal(&persistence_id).is_empty() {
                        "precommit-write-failure"
                    } else {
                        "postcommit-reply-loss"
                    };
                    invocation = durable_invocation_with_store(
                        operations.clone(),
                        SecurityContext::system(),
                        store.clone(),
                    );
                    trace.push(format!("{step}:crash-restart:{boundary}"));
                }
            }
            2 => {
                invocation = durable_invocation_with_store(
                    operations.clone(),
                    SecurityContext::system(),
                    store.clone(),
                );
                trace.push(format!("{step}:restart"));
            }
            3 => {
                let projections = store.list_query_projections("default", "Customer");
                let snapshot = projections
                    .iter()
                    .map(|(id, projection)| {
                        format!("{id}:{}:{}", projection.sequence_nr, projection.status)
                    })
                    .collect::<Vec<_>>()
                    .join(",");
                trace.push(format!("{step}:query:{snapshot}"));
            }
            4 => {
                let left_slot = (generator.next_u64() as usize) % IDS.len();
                let right_slot = (left_slot + 1) % IDS.len();
                let (left, right) = tokio::join!(
                    call(
                        &invocation,
                        operation(
                            IDS[left_slot],
                            &format!("seed-{seed}-race-{step}-left"),
                            "Ada",
                        ),
                    ),
                    call(
                        &invocation,
                        operation(
                            IDS[right_slot],
                            &format!("seed-{seed}-race-{step}-right"),
                            "Ada",
                        ),
                    )
                );
                trace.push(format!(
                    "{step}:concurrent:{left_slot}:{right_slot}:{}:{}",
                    outcome_tag(left.outcome),
                    outcome_tag(right.outcome)
                ));
            }
            5 => {
                let slot = (generator.next_u64() as usize) % IDS.len();
                let persistence_id = format!("default:Customer:{}", IDS[slot]);
                if let Some(projection) = store.dump_first_event_projection(&persistence_id) {
                    store.remove_query_projection("default", "Customer", IDS[slot]);
                    let response = call(
                        &invocation,
                        operation(
                            IDS[slot],
                            &format!("seed-{seed}-projection-lag-{slot}"),
                            "Ada",
                        ),
                    )
                    .await;
                    trace.push(format!(
                        "{step}:projection-lag:{slot}:{}",
                        outcome_tag(response.outcome)
                    ));
                    store.upsert_query_projection("default", "Customer", IDS[slot], projection);
                } else {
                    trace.push(format!("{step}:projection-lag:{slot}:absent"));
                }
            }
            _ => {
                let slot = (generator.next_u64() as usize) % IDS.len();
                let tenant = TenantId::default();
                let (_, response) = tokio::join!(
                    invocation.state.populate_creation_contracts(&tenant),
                    call(
                        &invocation,
                        operation(
                            IDS[slot],
                            &format!("seed-{seed}-backfill-race-{step}"),
                            "Ada",
                        ),
                    )
                );
                trace.push(format!(
                    "{step}:backfill-race:{slot}:{}",
                    outcome_tag(response.outcome)
                ));
            }
        }
    }

    store.disable_faults();
    invocation =
        durable_invocation_with_store(operations, SecurityContext::system(), store.clone());
    for (slot, id) in IDS.iter().enumerate() {
        let response = call(
            &invocation,
            operation(id, &format!("seed-{seed}-slot-{slot}"), "Ada"),
        )
        .await;
        trace.push(format!("final:{slot}:{}", outcome_tag(response.outcome)));
        let journal = store.dump_journal(&format!("default:Customer:{id}"));
        assert!(
            journal.len() <= 1,
            "seed {seed}, slot {slot}: duplicate Created"
        );
        if let Some(event) = journal.first() {
            assert_eq!(event.sequence_nr, 1, "seed {seed}, slot {slot}");
        }
    }
    for (_, projection) in store.list_query_projections("default", "Customer") {
        assert_eq!(projection.sequence_nr, 1, "seed {seed}: stale projection");
    }
    trace
}

fn outcome_tag(outcome: DataOutcomeV1) -> &'static str {
    match outcome {
        DataOutcomeV1::Ok { .. } => "ok",
        DataOutcomeV1::Error { .. } => "err",
    }
}

#[tokio::test]
async fn generated_server_restart_fault_histories_are_deterministic_and_convergent() {
    for seed in 1..=16 {
        let first = run_generated_history(seed).await;
        let second = run_generated_history(seed).await;
        assert_eq!(first, second, "seed {seed}: nondeterministic trace");
    }
}
