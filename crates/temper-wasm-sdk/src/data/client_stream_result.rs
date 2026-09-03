#[cfg(target_arch = "wasm32")]
use super::{sdk_error, stream_error};
#[cfg(target_arch = "wasm32")]
use crate::data::{ModuleDataError, ModuleDataErrorKind};
#[cfg(target_arch = "wasm32")]
use crate::{FailureOutcome, FailureRetryability};

#[cfg(target_arch = "wasm32")]
/// Decode the bounded integer result returned by File stream host calls.
pub(super) fn decode_stream_result(result: i32) -> Result<usize, ModuleDataError> {
    match result {
        value if value >= 0 => Ok(value as usize),
        -1 => Err(ModuleDataError::new(
            ModuleDataErrorKind::TransientUnavailable,
            "WouldBlock",
            "File stream would block",
            FailureRetryability::WithBackoff,
            FailureOutcome::NotApplied,
        )
        .expect("static stream retry contract must be valid")),
        -2 => Err(stream_error("FileStreamClosed", "File stream is closed")),
        -3 => Err(stream_error(
            "InvalidFileStream",
            "File stream handle is invalid",
        )),
        _ => Err(sdk_error(
            "FileStreamHostFailure",
            "File stream host failed".into(),
        )),
    }
}
