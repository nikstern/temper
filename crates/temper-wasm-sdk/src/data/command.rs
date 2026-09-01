//! Shared generated-command encoding and nullable patch contracts.

use serde::Serialize;

use super::{ModuleDataError, ModuleDataErrorKind, Retryability};

/// Three-way nullable patch value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum NullablePatch<T> {
    /// Leave the canonical property unchanged.
    #[default]
    Unchanged,
    /// Set the canonical property to null.
    Null,
    /// Set the canonical property to a non-null value.
    Value(T),
}

impl<T> NullablePatch<T> {
    /// Whether this patch leaves the property unchanged.
    pub const fn is_unchanged(&self) -> bool {
        matches!(self, Self::Unchanged)
    }
}

impl<T: Serialize> Serialize for NullablePatch<T> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        match self {
            Self::Unchanged => Err(serde::ser::Error::custom(
                "unchanged nullable patch must be skipped",
            )),
            Self::Null => serializer.serialize_none(),
            Self::Value(value) => value.serialize(serializer),
        }
    }
}

/// Encode one generated command into the owned application-data ABI object.
pub fn encode_command_object<T: Serialize + ?Sized>(
    command: &T,
) -> Result<serde_json::Map<String, serde_json::Value>, ModuleDataError> {
    let encoded = serde_json::to_value(command).map_err(|error| {
        ModuleDataError::new(
            ModuleDataErrorKind::InvalidRequest,
            "GeneratedCommandEncodingFailed",
            error.to_string(),
            Retryability::Never,
        )
    })?;
    match encoded {
        serde_json::Value::Object(object) => Ok(object),
        _ => Err(ModuleDataError::new(
            ModuleDataErrorKind::SchemaMismatch,
            "GeneratedCommandNotObject",
            "generated command did not encode as a JSON object",
            Retryability::Never,
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Serialize)]
    struct Patch<'a> {
        #[serde(skip_serializing_if = "NullablePatch::is_unchanged")]
        value: NullablePatch<&'a str>,
    }

    #[test]
    fn nullable_patch_preserves_unchanged_null_and_value() {
        let unchanged = encode_command_object(&Patch {
            value: NullablePatch::Unchanged,
        })
        .unwrap();
        assert!(unchanged.is_empty());
        let null = encode_command_object(&Patch {
            value: NullablePatch::Null,
        })
        .unwrap();
        assert_eq!(null["value"], serde_json::Value::Null);
        let value = encode_command_object(&Patch {
            value: NullablePatch::Value("ready"),
        })
        .unwrap();
        assert_eq!(value["value"], "ready");
    }

    #[test]
    fn non_object_encoding_fails_closed() {
        let error = encode_command_object("not-an-object").unwrap_err();
        assert_eq!(error.code, "GeneratedCommandNotObject");
        assert_eq!(error.kind, ModuleDataErrorKind::SchemaMismatch);
    }
}
