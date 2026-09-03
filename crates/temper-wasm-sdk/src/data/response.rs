use serde::{Deserialize, Serialize};

use super::{
    CommitToken, CreateOrVerifyResultV1, DATA_ABI_VERSION_V1, DATA_ABI_VERSION_V2, DataObject,
    DataResultV1, FileMetadataV1, ModuleDataError, ModuleDataErrorV1, SequencedValueV1,
};

/// One versioned response from the governed application-data host.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DataResponseV1 {
    /// ABI version. Always [`DATA_ABI_VERSION_V1`].
    pub abi: u32,
    /// Successful result or structured error.
    pub outcome: DataOutcomeV1,
}

impl DataResponseV1 {
    /// Construct a successful v1 response.
    pub const fn ok(result: DataResultV1) -> Self {
        Self {
            abi: DATA_ABI_VERSION_V1,
            outcome: DataOutcomeV1::Ok { result },
        }
    }

    /// Construct an error v1 response.
    pub const fn error(error: ModuleDataError) -> Self {
        Self {
            abi: DATA_ABI_VERSION_V1,
            outcome: DataOutcomeV1::Error { error },
        }
    }
}

/// Success or structured domain error.
#[derive(Debug, Clone, PartialEq)]
pub enum DataOutcomeV1 {
    /// Successful operation outcome.
    Ok { result: DataResultV1 },
    /// Rejected or failed operation outcome.
    Error { error: ModuleDataError },
}

impl Serialize for DataOutcomeV1 {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        #[derive(Serialize)]
        #[serde(tag = "kind", rename_all = "snake_case")]
        enum Wire<'a> {
            Ok { result: &'a DataResultV1 },
            Error { error: ModuleDataErrorV1 },
        }
        match self {
            Self::Ok { result } => Wire::Ok { result }.serialize(serializer),
            Self::Error { error } => Wire::Error {
                error: ModuleDataErrorV1::from(error),
            }
            .serialize(serializer),
        }
    }
}

impl<'de> Deserialize<'de> for DataOutcomeV1 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
        enum Wire {
            Ok { result: DataResultV1 },
            Error { error: ModuleDataErrorV1 },
        }
        match Wire::deserialize(deserializer)? {
            Wire::Ok { result } => Ok(Self::Ok { result }),
            Wire::Error { error } => ModuleDataError::try_from(error)
                .map(|error| Self::Error { error })
                .map_err(serde::de::Error::custom),
        }
    }
}

/// One response from application-data ABI v2.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DataResponseV2 {
    /// ABI version. Always [`DATA_ABI_VERSION_V2`].
    pub abi: u32,
    /// Successful result or canonical structured error.
    pub outcome: DataOutcomeV2,
}

impl DataResponseV2 {
    /// Construct a successful v2 response.
    pub fn ok(result: impl Into<DataResultV2>) -> Self {
        Self {
            abi: DATA_ABI_VERSION_V2,
            outcome: DataOutcomeV2::Ok {
                result: result.into(),
            },
        }
    }

    /// Construct an error v2 response.
    pub const fn error(error: ModuleDataError) -> Self {
        Self {
            abi: DATA_ABI_VERSION_V2,
            outcome: DataOutcomeV2::Error { error },
        }
    }
}

/// Success or canonical structured error in application-data ABI v2.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum DataOutcomeV2 {
    /// Successful operation outcome.
    Ok { result: DataResultV2 },
    /// Rejected or failed operation outcome.
    Error { error: ModuleDataError },
}

/// Successful application-data ABI-v2 operation results.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum DataResultV2 {
    /// One authoritative entity value.
    Entity {
        /// Canonical entity value.
        value: DataObject,
        /// Entity stream sequence represented by the value.
        sequence: u64,
    },
    /// One bounded ordered collection page.
    Page {
        /// Values and their per-entity sequences.
        values: Vec<SequencedValueV1>,
        /// Opaque continuation cursor when more candidates remain.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        next_cursor: Option<String>,
    },
    /// A committed create or patch.
    Write {
        /// Durable per-entity commit token.
        commit: CommitToken,
        /// Returned entity value when it fits the response budget.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        value: Option<DataObject>,
        /// Whether the value was omitted after the write committed.
        value_omitted: bool,
    },
    /// A closed atomic create-or-verify result.
    CreateOrVerify {
        /// Creation, canonical match, or bounded conflict classification.
        outcome: CreateOrVerifyResultV1,
    },
    /// A committed action.
    Action {
        /// Durable per-entity commit token.
        commit: CommitToken,
        /// Action result when present and within budget.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        result: Option<serde_json::Value>,
        /// Whether the result was omitted after the action committed.
        result_omitted: bool,
    },
    /// Request-ordered outcomes from a non-atomic batch.
    Batch {
        /// One independent outcome per input item.
        outcomes: Vec<DataOutcomeV2>,
    },
    /// An open bounded File read stream.
    FileRead {
        /// Invocation-scoped read-stream handle.
        stream_handle: u32,
        /// File metadata resolved at open time.
        metadata: FileMetadataV1,
        /// File entity sequence resolved at open time.
        sequence: u64,
        /// Content length when known.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        content_length: Option<u64>,
    },
    /// An open bounded File write stream.
    FileWrite {
        /// Invocation-scoped write-stream handle.
        stream_handle: u32,
    },
    /// A durably committed File content write.
    FileCommitted {
        /// Durable File commit token.
        commit: CommitToken,
        /// Updated File metadata when available.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        metadata: Option<FileMetadataV1>,
    },
    /// An explicitly aborted File stream.
    Aborted,
}

impl From<DataResultV1> for DataResultV2 {
    fn from(result: DataResultV1) -> Self {
        match result {
            DataResultV1::Entity { value, sequence } => Self::Entity { value, sequence },
            DataResultV1::Page {
                values,
                next_cursor,
            } => Self::Page {
                values,
                next_cursor,
            },
            DataResultV1::Write {
                commit,
                value,
                value_omitted,
            } => Self::Write {
                commit,
                value,
                value_omitted,
            },
            DataResultV1::CreateOrVerify { outcome } => Self::CreateOrVerify { outcome },
            DataResultV1::Action {
                commit,
                result,
                result_omitted,
            } => Self::Action {
                commit,
                result,
                result_omitted,
            },
            DataResultV1::Batch { outcomes } => Self::Batch {
                outcomes: outcomes
                    .into_iter()
                    .map(|outcome| match outcome {
                        DataOutcomeV1::Ok { result } => DataOutcomeV2::Ok {
                            result: result.into(),
                        },
                        DataOutcomeV1::Error { error } => DataOutcomeV2::Error { error },
                    })
                    .collect(),
            },
            DataResultV1::FileRead {
                stream_handle,
                metadata,
                sequence,
                content_length,
            } => Self::FileRead {
                stream_handle,
                metadata,
                sequence,
                content_length,
            },
            DataResultV1::FileWrite { stream_handle } => Self::FileWrite { stream_handle },
            DataResultV1::FileCommitted { commit, metadata } => {
                Self::FileCommitted { commit, metadata }
            }
            DataResultV1::Aborted => Self::Aborted,
        }
    }
}

impl From<DataResultV2> for DataResultV1 {
    fn from(result: DataResultV2) -> Self {
        match result {
            DataResultV2::Entity { value, sequence } => Self::Entity { value, sequence },
            DataResultV2::Page {
                values,
                next_cursor,
            } => Self::Page {
                values,
                next_cursor,
            },
            DataResultV2::Write {
                commit,
                value,
                value_omitted,
            } => Self::Write {
                commit,
                value,
                value_omitted,
            },
            DataResultV2::CreateOrVerify { outcome } => Self::CreateOrVerify { outcome },
            DataResultV2::Action {
                commit,
                result,
                result_omitted,
            } => Self::Action {
                commit,
                result,
                result_omitted,
            },
            DataResultV2::Batch { outcomes } => Self::Batch {
                outcomes: outcomes
                    .into_iter()
                    .map(|outcome| match outcome {
                        DataOutcomeV2::Ok { result } => DataOutcomeV1::Ok {
                            result: result.into(),
                        },
                        DataOutcomeV2::Error { error } => DataOutcomeV1::Error { error },
                    })
                    .collect(),
            },
            DataResultV2::FileRead {
                stream_handle,
                metadata,
                sequence,
                content_length,
            } => Self::FileRead {
                stream_handle,
                metadata,
                sequence,
                content_length,
            },
            DataResultV2::FileWrite { stream_handle } => Self::FileWrite { stream_handle },
            DataResultV2::FileCommitted { commit, metadata } => {
                Self::FileCommitted { commit, metadata }
            }
            DataResultV2::Aborted => Self::Aborted,
        }
    }
}

impl From<DataResponseV1> for DataResponseV2 {
    fn from(response: DataResponseV1) -> Self {
        let outcome = match response.outcome {
            DataOutcomeV1::Ok { result } => DataOutcomeV2::Ok {
                result: result.into(),
            },
            DataOutcomeV1::Error { error } => DataOutcomeV2::Error { error },
        };
        Self {
            abi: DATA_ABI_VERSION_V2,
            outcome,
        }
    }
}

impl From<DataResponseV2> for DataResponseV1 {
    fn from(response: DataResponseV2) -> Self {
        let outcome = match response.outcome {
            DataOutcomeV2::Ok { result } => DataOutcomeV1::Ok {
                result: result.into(),
            },
            DataOutcomeV2::Error { error } => DataOutcomeV1::Error { error },
        };
        Self {
            abi: DATA_ABI_VERSION_V1,
            outcome,
        }
    }
}
