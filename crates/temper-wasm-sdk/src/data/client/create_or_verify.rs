use super::super::{CreateOrVerifyOutcome, CreateOrVerifyResultV1, DataResultV1, ModuleDataError};

use super::{decode_object, result_shape_error};

/// Decode a create-or-verify result into a generated entity type.
pub fn decode_create_or_verify<T: serde::de::DeserializeOwned>(
    result: DataResultV1,
) -> Result<CreateOrVerifyOutcome<T>, ModuleDataError> {
    let DataResultV1::CreateOrVerify { outcome } = result else {
        return Err(result_shape_error("CreateOrVerify"));
    };
    match outcome {
        CreateOrVerifyResultV1::Created { commit, value } => Ok(CreateOrVerifyOutcome::Created {
            commit,
            value: decode_object(value)?,
        }),
        CreateOrVerifyResultV1::AlreadyMatches { commit, value } => {
            Ok(CreateOrVerifyOutcome::AlreadyMatches {
                commit,
                value: decode_object(value)?,
            })
        }
        CreateOrVerifyResultV1::Conflict { fields, truncated } => {
            Ok(CreateOrVerifyOutcome::Conflict { fields, truncated })
        }
    }
}
