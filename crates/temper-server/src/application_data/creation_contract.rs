//! Canonical immutable sequence-1 creation contracts (ADR-0196).

use sha2::{Digest, Sha256};
use temper_jit::table::types::DeclaredKey;
use temper_runtime::persistence::{
    CREATION_CONTRACT_VERSION_V1, CreationContract, CreationContractField,
};
use temper_wasm_sdk::data::{
    ManifestCreateRoleV1, ManifestEntityV1, ManifestPropertyV1, ManifestValueSourceV1,
    ModuleDataError, ModuleDataErrorKind,
};

use super::{not_applied_error, schema::effective_write_policy};

/// Materialize every caller-admitted optional create field before policy
/// evaluation, contract hashing, key derivation, and event construction.
pub(crate) fn materialize_creation_fields(
    entity: &ManifestEntityV1,
    value: &mut serde_json::Map<String, serde_json::Value>,
) {
    for property in &entity.properties {
        if effective_write_policy(property).create == ManifestCreateRoleV1::Forbidden
            || value.contains_key(&property.canonical_name)
        {
            continue;
        }
        if let Some(default) = property.default_value.clone() {
            value.insert(property.canonical_name.clone(), default);
        } else if property.nullable {
            value.insert(property.canonical_name.clone(), serde_json::Value::Null);
        }
    }
}

/// Materialize the exact schema-wide field set for legacy actor creation.
///
/// Ordinary actor/OData creation predates generated create admission and can
/// therefore omit a CSDL-required stored property. Preserve that working
/// capability while still reserving the property's place in the immutable
/// contract: explicit null records historical absence and cannot compare equal
/// to a later retry that supplies a value. Typed application-data creation
/// continues to reject the omission before reaching this helper.
pub(crate) fn materialize_actor_creation_fields(
    entity: &ManifestEntityV1,
    value: &mut serde_json::Map<String, serde_json::Value>,
) {
    materialize_creation_fields(entity, value);
    for property in &entity.properties {
        if effective_write_policy(property).create != ManifestCreateRoleV1::Forbidden {
            value
                .entry(property.canonical_name.clone())
                .or_insert(serde_json::Value::Null);
        }
    }
}

pub(crate) fn compile_creation_contract(
    entity: &ManifestEntityV1,
    schema_digest: &str,
    value: &serde_json::Map<String, serde_json::Value>,
) -> Result<CreationContract, ModuleDataError> {
    let mut fields = entity
        .properties
        .iter()
        .filter(|property| {
            effective_write_policy(property).create != ManifestCreateRoleV1::Forbidden
        })
        .map(|property| compile_field(property, value))
        .collect::<Result<Vec<_>, _>>()?;
    fields.sort_by(|left, right| left.name.cmp(&right.name));

    let mut digest = Sha256::new();
    framed(&mut digest, b"temper.creation-contract.v1");
    framed(&mut digest, schema_digest.as_bytes());
    for field in &fields {
        framed(&mut digest, field.name.as_bytes());
        framed(&mut digest, field.type_descriptor.as_bytes());
        framed(&mut digest, field.value_source.as_bytes());
        framed(&mut digest, &[u8::from(field.nullable)]);
        framed(
            &mut digest,
            &[u8::from(field.create_required.expect(
                "new creation contracts always encode requiredness",
            ))],
        );
        framed(&mut digest, field.default_digest.as_bytes());
        framed(&mut digest, field.value_digest.as_bytes());
    }
    Ok(CreationContract {
        version: CREATION_CONTRACT_VERSION_V1,
        schema_digest: schema_digest.to_string(),
        fields,
        digest: format!("sha256:{:x}", digest.finalize()),
    })
}

/// Hash the complete declared-key schema covered by a creation contract.
///
/// This is intentionally independent of the values present on one entity: a
/// nullable or released key remains part of the exact declared-key set even
/// when it produces no ownership row for that entity.
pub(crate) fn declared_key_signature(
    declared_keys: &[DeclaredKey],
    contract: &CreationContract,
) -> String {
    let mut keys = declared_keys.to_vec();
    keys.sort_by(|left, right| {
        (&left.name, &left.properties, left.entity_id).cmp(&(
            &right.name,
            &right.properties,
            right.entity_id,
        ))
    });
    let fields = contract
        .fields
        .iter()
        .map(|field| (field.name.as_str(), field))
        .collect::<std::collections::BTreeMap<_, _>>();
    let mut digest = Sha256::new();
    framed(&mut digest, b"temper.declared-key-signature.v1");
    framed(&mut digest, &contract.version.to_be_bytes());
    for key in keys {
        framed(&mut digest, key.name.as_bytes());
        framed(&mut digest, &[u8::from(key.entity_id)]);
        for property in key.properties {
            framed(&mut digest, property.as_bytes());
            if let Some(field) = fields.get(property.as_str()) {
                framed(&mut digest, field.type_descriptor.as_bytes());
                framed(&mut digest, field.value_source.as_bytes());
                framed(&mut digest, &[u8::from(field.nullable)]);
                framed(
                    &mut digest,
                    &[u8::from(field.create_required.expect(
                        "new creation contracts always encode requiredness",
                    ))],
                );
                framed(&mut digest, field.default_digest.as_bytes());
            } else {
                framed(&mut digest, b"<not-in-creation-contract>");
            }
        }
    }
    format!("sha256:{:x}", digest.finalize())
}

fn compile_field(
    property: &ManifestPropertyV1,
    value: &serde_json::Map<String, serde_json::Value>,
) -> Result<CreationContractField, ModuleDataError> {
    let policy = effective_write_policy(property);
    let canonical = value
        .get(&property.canonical_name)
        .cloned()
        .or_else(|| property.default_value.clone())
        .or_else(|| property.nullable.then_some(serde_json::Value::Null))
        .ok_or_else(|| {
            not_applied_error(
                ModuleDataErrorKind::SchemaMismatch,
                "MissingRequiredProperty",
                "required creation-contract property is absent",
            )
        })?;
    let canonical = match canonical_value(property, canonical.clone()) {
        Ok(canonical) => canonical,
        // Entity IDs are server authority, and legacy OData creation has always
        // allowed opaque IDs even when a CSDL key uses a narrower scalar such
        // as Edm.Guid. The contract records that authoritative identity exactly;
        // schema admission of caller-owned fields remains strict.
        Err(_) if property.source == ManifestValueSourceV1::EntityId => canonical_json(canonical),
        Err(error) => return Err(error),
    };
    let default_digest = if policy.create == ManifestCreateRoleV1::Optional {
        let default = property
            .default_value
            .clone()
            .unwrap_or(serde_json::Value::Null);
        digest_value(property, &canonical_value(property, default)?)?
    } else {
        String::new()
    };
    let value_source = match property.source {
        ManifestValueSourceV1::Input => "input",
        ManifestValueSourceV1::StoredField => "stored_field",
        ManifestValueSourceV1::EntityId => "entity_id",
        ManifestValueSourceV1::LifecycleStatus => "lifecycle_status",
    };
    Ok(CreationContractField {
        name: property.canonical_name.clone(),
        type_descriptor: if property.enum_members.is_empty() {
            property.type_name.clone()
        } else {
            format!(
                "{}[{}]",
                property.type_name,
                property.enum_members.join(",")
            )
        },
        value_source: value_source.to_string(),
        nullable: property.nullable,
        create_required: Some(policy.create == ManifestCreateRoleV1::Required),
        default_digest,
        value_digest: digest_value(property, &canonical)?,
    })
}

fn canonical_value(
    property: &ManifestPropertyV1,
    value: serde_json::Value,
) -> Result<serde_json::Value, ModuleDataError> {
    if value.is_null() {
        return Ok(value);
    }
    let normalized = match property.type_name.as_str() {
        "Edm.Byte" | "Edm.SByte" | "Edm.Int16" | "Edm.Int32" | "Edm.Int64" => value
            .as_i64()
            .map(|number| serde_json::Value::String(number.to_string())),
        "Edm.Decimal" => match value {
            serde_json::Value::String(number) => {
                Some(serde_json::Value::String(normalize_decimal(&number)?))
            }
            serde_json::Value::Number(number) => Some(serde_json::Value::String(
                normalize_decimal(&number.to_string())?,
            )),
            _ => None,
        },
        "Edm.Double" | "Edm.Single" => value
            .as_f64()
            .and_then(|number| serde_json::Number::from_f64(number).map(serde_json::Value::Number)),
        "Edm.Guid" => value.as_str().and_then(|text| {
            uuid::Uuid::parse_str(text)
                .ok()
                .map(|guid| serde_json::Value::String(guid.hyphenated().to_string()))
        }),
        _ => Some(canonical_json(value)),
    };
    normalized.ok_or_else(|| {
        not_applied_error(
            ModuleDataErrorKind::SchemaMismatch,
            "PropertyTypeMismatch",
            "creation-contract value does not match its canonical type",
        )
    })
}

fn normalize_decimal(value: &str) -> Result<String, ModuleDataError> {
    let value = value.trim();
    let (negative, unsigned) = if let Some(rest) = value.strip_prefix('-') {
        (true, rest)
    } else {
        (false, value.strip_prefix('+').unwrap_or(value))
    };
    let mut exponent_parts = unsigned.split(['e', 'E']);
    let mantissa = exponent_parts.next().unwrap_or_default();
    let exponent = exponent_parts
        .next()
        .map(|value| value.parse::<i32>())
        .transpose()
        .map_err(|_| invalid_decimal())?
        .unwrap_or(0);
    if exponent_parts.next().is_some() {
        return Err(invalid_decimal());
    }
    let (integer, fraction) = mantissa.split_once('.').unwrap_or((mantissa, ""));
    if integer.is_empty()
        || !integer.bytes().all(|byte| byte.is_ascii_digit())
        || !fraction.bytes().all(|byte| byte.is_ascii_digit())
    {
        return Err(invalid_decimal());
    }
    let joined = format!("{integer}{fraction}");
    let significant = joined.trim_start_matches('0');
    if significant.is_empty() {
        return Ok("0".to_string());
    }
    let coefficient = significant.trim_end_matches('0');
    let removed_trailing = significant.len() - coefficient.len();
    let power = exponent
        .checked_sub(i32::try_from(fraction.len()).map_err(|_| invalid_decimal())?)
        .and_then(|power| power.checked_add(i32::try_from(removed_trailing).ok()?))
        .ok_or_else(invalid_decimal)?;
    let sign = if negative { "-" } else { "" };
    Ok(format!("{sign}{coefficient}e{power}"))
}

fn invalid_decimal() -> ModuleDataError {
    not_applied_error(
        ModuleDataErrorKind::SchemaMismatch,
        "PropertyTypeMismatch",
        "decimal creation-contract value is invalid",
    )
}

fn canonical_json(value: serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::Array(values) => {
            serde_json::Value::Array(values.into_iter().map(canonical_json).collect())
        }
        serde_json::Value::Object(values) => {
            let ordered = values
                .into_iter()
                .map(|(name, value)| (name, canonical_json(value)))
                .collect::<std::collections::BTreeMap<_, _>>();
            serde_json::Value::Object(ordered.into_iter().collect())
        }
        scalar => scalar,
    }
}

fn digest_value(
    property: &ManifestPropertyV1,
    value: &serde_json::Value,
) -> Result<String, ModuleDataError> {
    let encoded = serde_json::to_vec(value).map_err(|error| {
        not_applied_error(
            ModuleDataErrorKind::Internal,
            "CreationContractEncodingFailed",
            &error.to_string(),
        )
    })?;
    let mut digest = Sha256::new();
    framed(&mut digest, b"temper.creation-contract.field.v1");
    framed(&mut digest, property.canonical_name.as_bytes());
    framed(&mut digest, property.type_name.as_bytes());
    framed(&mut digest, &encoded);
    Ok(format!("sha256:{:x}", digest.finalize()))
}

fn framed(digest: &mut Sha256, value: &[u8]) {
    digest.update((value.len() as u64).to_be_bytes());
    digest.update(value);
}

#[cfg(test)]
mod tests;
