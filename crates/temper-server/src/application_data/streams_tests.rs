use super::{FileStream, FileStreamRegistry};
use temper_wasm_sdk::data::ModuleDataBudgets;

#[test]
fn typed_stream_admission_has_no_application_field_aliases() {
    let source = include_str!("streams.rs");
    for forbidden in [
        "version_belongs_to_file",
        "declared_stream_length",
        "\"Size\"",
        "\"size\"",
        "\"ContentLength\"",
        "\"content_length\"",
        "\"SizeBytes\"",
        "\"size_bytes\"",
        "\"FileId\"",
        "\"file_id\"",
    ] {
        assert!(
            !source.contains(forbidden),
            "typed admission must not infer authority from '{forbidden}'"
        );
    }
}

#[test]
fn final_file_chunk_is_followed_by_eof_then_consumption() {
    let mut registry = FileStreamRegistry::new(&ModuleDataBudgets::default());
    let handle = registry
        .insert(FileStream::Read { offset: 0 }, b"abc".to_vec())
        .unwrap();
    assert_eq!(registry.read(handle, 3).unwrap(), b"abc");
    assert_eq!(registry.read(handle, 3).unwrap(), b"");
    assert_eq!(registry.read(handle, 3), Err(-3));
}

#[test]
fn write_chunks_append_under_stream_budget() {
    let mut registry = FileStreamRegistry::new(&ModuleDataBudgets::default());
    let handle = registry
        .insert(
            FileStream::Write {
                entity_type: "Temper.FileSystem.File".into(),
                file_id: "file-1".into(),
                expected_length: None,
                expected_hash: None,
                expected_sequence: None,
                committing: false,
            },
            Vec::new(),
        )
        .unwrap();
    registry.max_bytes = 3;
    assert_eq!(registry.write(handle, b"ab"), Ok(2));
    assert_eq!(registry.write(handle, b"c"), Ok(1));
    assert_eq!(registry.write(handle, b"d"), Err(-4));
    assert_eq!(registry.take(handle).unwrap().1, b"abc");
}

#[test]
fn commit_validation_and_failed_dispatch_leave_write_retryable() {
    let mut registry = FileStreamRegistry::new(&ModuleDataBudgets::default());
    let handle = registry
        .insert(
            FileStream::Write {
                entity_type: "Temper.FileSystem.File".into(),
                file_id: "file-1".into(),
                expected_length: Some(3),
                expected_hash: None,
                expected_sequence: Some(7),
                committing: false,
            },
            Vec::new(),
        )
        .unwrap();
    registry.write(handle, b"ab").unwrap();
    assert_eq!(
        registry.begin_commit(handle).unwrap_err().code().as_str(),
        "FileLengthMismatch"
    );
    registry.write(handle, b"c").unwrap();
    assert_eq!(registry.begin_commit(handle).unwrap().bytes, b"abc");
    assert_eq!(registry.write(handle, b"d"), Err(-3));
    registry.finish_commit(handle, false).unwrap();
    assert_eq!(registry.begin_commit(handle).unwrap().bytes, b"abc");
    registry.finish_commit(handle, true).unwrap();
    assert!(registry.begin_commit(handle).is_err());
}

#[test]
fn commit_rejects_read_direction_without_consuming_it() {
    let mut registry = FileStreamRegistry::new(&ModuleDataBudgets::default());
    let handle = registry
        .insert(FileStream::Read { offset: 0 }, b"abc".to_vec())
        .unwrap();
    assert!(registry.begin_commit(handle).is_err());
    assert_eq!(registry.read(handle, 3).unwrap(), b"abc");
}

#[test]
fn file_commit_preserves_rejected_and_ambiguous_phases() {
    let rejected = super::file_commit_error(crate::state::FileStreamContentError::ActionRejected(
        "guard rejected".into(),
    ));
    assert_eq!(
        rejected.outcome(),
        temper_wasm_sdk::FailureOutcome::NotApplied
    );
    assert_eq!(rejected.code().as_str(), "FileCommitRejected");
    assert_eq!(
        rejected.retryability(),
        temper_wasm_sdk::FailureRetryability::Never
    );

    let ambiguous = super::file_commit_error(crate::state::FileStreamContentError::BlobStore(
        "reply lost".into(),
    ));
    assert_eq!(
        ambiguous.outcome(),
        temper_wasm_sdk::FailureOutcome::Unknown
    );
    assert_eq!(ambiguous.code().as_str(), "DataAcknowledgementUnknown");
    assert_eq!(
        ambiguous.retryability(),
        temper_wasm_sdk::FailureRetryability::Reconcile
    );

    for (outcome, code, retryability) in [
        (
            temper_wasm_sdk::FailureOutcome::NotApplied,
            "FileCommitRejected",
            temper_wasm_sdk::FailureRetryability::Never,
        ),
        (
            temper_wasm_sdk::FailureOutcome::Applied,
            "PostCommitDataServiceFailure",
            temper_wasm_sdk::FailureRetryability::Never,
        ),
        (
            temper_wasm_sdk::FailureOutcome::Unknown,
            "DataAcknowledgementUnknown",
            temper_wasm_sdk::FailureRetryability::Reconcile,
        ),
    ] {
        let state = serde_json::from_value(serde_json::json!({
            "entity_type": "File",
            "entity_id": "file-1",
            "status": "Ready",
            "item_count": 0,
            "fields": {},
            "sequence_nr": 7
        }))
        .expect("minimal entity state decodes");
        let response = crate::entity_actor::EntityResponse {
            success: false,
            state,
            error: Some("injected File action failure".into()),
            failure_outcome: Some(outcome),
            custom_effects: Vec::new(),
            scheduled_actions: Vec::new(),
            spawn_requests: Vec::new(),
            spec_governed: true,
        };
        let error = super::validate_file_commit_response(response)
            .expect_err("failed action response must not publish FileCommitted");
        assert_eq!(error.outcome(), outcome);
        assert_eq!(error.code().as_str(), code);
        assert_eq!(error.retryability(), retryability);
    }
}
