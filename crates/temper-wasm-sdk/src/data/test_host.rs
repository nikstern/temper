//! Native scripted host used only by generated SDK integration tests.

use std::collections::VecDeque;
use std::sync::{LazyLock, Mutex};

use super::{
    DataRequestV2, DataResponseV1, DataResponseV2, FailureOutcome, FailureRetryability,
    ModuleDataError, ModuleDataErrorKind,
};

#[derive(Default)]
struct NativeDataHost {
    responses: VecDeque<DataResponseV2>,
    requests: Vec<DataRequestV2>,
}

static HOST: LazyLock<Mutex<NativeDataHost>> =
    LazyLock::new(|| Mutex::new(NativeDataHost::default()));

/// Install the exact response sequence returned by the native test host.
pub fn install_native_data_host_for_test(responses: Vec<DataResponseV1>) {
    let mut host = HOST.lock().expect("native data test host lock poisoned");
    assert!(
        host.responses.is_empty(),
        "native data test host still has unconsumed responses"
    );
    host.responses = responses.into_iter().map(DataResponseV2::from).collect();
    host.requests.clear();
}

/// Remove and return every request observed by the native test host.
pub fn take_native_data_requests_for_test() -> Vec<DataRequestV2> {
    let mut host = HOST.lock().expect("native data test host lock poisoned");
    assert!(
        host.responses.is_empty(),
        "native data test host responses must all be consumed"
    );
    std::mem::take(&mut host.requests)
}

pub(super) fn call(request: &[u8]) -> Result<DataResponseV2, ModuleDataError> {
    let request = serde_json::from_slice(request).map_err(|error| {
        ModuleDataError::new(
            ModuleDataErrorKind::InvalidRequest,
            "NativeTestRequestMismatch",
            error.to_string(),
            FailureRetryability::Never,
            FailureOutcome::NotApplied,
        )
        .expect("static native-test failure contract must be valid")
    })?;
    let mut host = HOST.lock().expect("native data test host lock poisoned");
    host.requests.push(request);
    host.responses.pop_front().ok_or_else(|| {
        ModuleDataError::new(
            ModuleDataErrorKind::Internal,
            "NativeTestResponseExhausted",
            "native data test host has no scripted response",
            FailureRetryability::Never,
            FailureOutcome::NotApplied,
        )
        .expect("static native-test failure contract must be valid")
    })
}
