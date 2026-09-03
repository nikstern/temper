//! Stable typed SDK error mapping for descriptor admission.

use temper_wasm_sdk::data::{ModuleDataError, ModuleDataErrorKind};

use crate::application_data::not_applied_error;
use crate::state::StreamDescriptorResolutionError;

pub(super) fn invalid_stream() -> ModuleDataError {
    not_applied_error(
        ModuleDataErrorKind::InvalidRequest,
        "InvalidFileStream",
        "File stream handle is invalid or has the wrong direction",
    )
}

pub(super) fn stream_registry_unavailable() -> ModuleDataError {
    not_applied_error(
        ModuleDataErrorKind::Internal,
        "InvocationStatePoisoned",
        "File stream registry unavailable",
    )
}

pub(super) fn stream_descriptor_error(error: StreamDescriptorResolutionError) -> ModuleDataError {
    let stable_code = error.stable_code();
    match error {
        StreamDescriptorResolutionError::BudgetExceeded => not_applied_error(
            ModuleDataErrorKind::BudgetExceeded,
            stable_code,
            "File content exceeds the stream byte budget",
        ),
        StreamDescriptorResolutionError::Missing => not_applied_error(
            ModuleDataErrorKind::ConsistencyUnavailable,
            stable_code,
            "Authoritative stream descriptor is unavailable",
        ),
        StreamDescriptorResolutionError::Integrity(_) => not_applied_error(
            ModuleDataErrorKind::ConsistencyUnavailable,
            stable_code,
            "Committed stream content failed integrity verification",
        ),
        StreamDescriptorResolutionError::ReplayBudgetExceeded
        | StreamDescriptorResolutionError::Consistency(_) => not_applied_error(
            ModuleDataErrorKind::ConsistencyUnavailable,
            stable_code,
            "Authoritative stream descriptor is inconsistent",
        ),
        StreamDescriptorResolutionError::JournalUnavailable
        | StreamDescriptorResolutionError::Storage(_) => not_applied_error(
            ModuleDataErrorKind::TransientUnavailable,
            stable_code,
            "Authoritative stream descriptor storage is unavailable",
        ),
    }
}
