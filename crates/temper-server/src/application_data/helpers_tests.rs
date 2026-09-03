use super::*;
use crate::application_data::service::ApplicationDataWriteError;

#[test]
fn compacting_committed_action_preserves_token_and_marks_omission() {
    let commit = commit("Temper.Example.Customer", "customer-1", 7);
    let mut response = DataResponseV1::ok(DataResultV1::Action {
        commit: commit.clone(),
        result: Some(serde_json::json!({"Id": "customer-1"})),
        result_omitted: false,
    });
    compact_committed_results(&mut response);
    let temper_wasm_sdk::data::DataOutcomeV1::Ok {
        result:
            DataResultV1::Action {
                commit: actual,
                result: None,
                result_omitted: true,
            },
    } = response.outcome
    else {
        panic!("compacted action must remain a successful commit")
    };
    assert_eq!(actual, commit);
}

#[test]
fn compacting_void_actions_preserves_their_non_omitted_shape_directly_and_in_batches() {
    let direct_commit = commit("Temper.Example.Customer", "customer-1", 7);
    let batch_commit = commit("Temper.Example.Customer", "customer-2", 8);
    let void = |commit| DataResultV1::Action {
        commit,
        result: None,
        result_omitted: false,
    };
    let mut direct = DataResponseV1::ok(void(direct_commit.clone()));
    let mut batch = DataResponseV1::ok(DataResultV1::Batch {
        outcomes: vec![temper_wasm_sdk::data::DataOutcomeV1::Ok {
            result: void(batch_commit.clone()),
        }],
    });
    compact_committed_results(&mut direct);
    compact_committed_results(&mut batch);
    assert!(
        matches!(direct.outcome, temper_wasm_sdk::data::DataOutcomeV1::Ok {
        result: DataResultV1::Action { commit, result: None, result_omitted: false }
    } if commit == direct_commit)
    );
    assert!(
        matches!(batch.outcome, temper_wasm_sdk::data::DataOutcomeV1::Ok {
        result: DataResultV1::Batch { outcomes }
    } if matches!(outcomes.as_slice(), [temper_wasm_sdk::data::DataOutcomeV1::Ok {
        result: DataResultV1::Action { commit, result: None, result_omitted: false }
    }] if commit == &batch_commit))
    );
}

#[test]
fn typed_write_phase_maps_without_diagnostic_classification() {
    let applied = write_service_error(ApplicationDataWriteError::Applied(
        "sequence conflict text is diagnostic only".into(),
    ));
    assert_eq!(applied.outcome(), FailureOutcome::Applied);
    assert_eq!(applied.code().as_str(), "PostCommitDataServiceFailure");
    let unknown = write_service_error(ApplicationDataWriteError::Unknown(
        "not found text is diagnostic only".into(),
    ));
    assert_eq!(unknown.outcome(), FailureOutcome::Unknown);
    assert_eq!(unknown.retryability(), FailureRetryability::Reconcile);
    assert_eq!(unknown.code().as_str(), "DataAcknowledgementUnknown");
}
