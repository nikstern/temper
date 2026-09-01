use std::collections::{BTreeMap, BTreeSet};

use sha2::{Digest, Sha256};
use temper_wasm_sdk::data::{
    EntityDataGrant, ManifestPropertyV1, ManifestValueSourceV1, ModuleSdkManifest,
};

use super::ModuleSdkCodegenError;
use super::names::{checked_enum_variant, rust_scalar_type, rust_type_name};

pub(super) fn emit_artifact_binding(source: &mut String, manifest: &ModuleSdkManifest) {
    for (name, digest) in [
        ("GRANT", &manifest.grant_digest),
        ("CLOSURE", &manifest.closure_digest),
        ("DEPENDENCY_LOCK", &manifest.dependency_lock_digest),
        ("SCHEMA", &manifest.schema_digest),
        ("USED_SYMBOLS", &manifest.used_symbols_digest),
    ] {
        source.push_str(&format!(
            "#[used]\npub static TEMPER_MODULE_{name}_DIGEST: &str = \"{digest}\";\n"
        ));
    }
}

pub(super) fn hex_sha256(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

pub(super) fn emit_named_property_type(
    source: &mut String,
    property: &ManifestPropertyV1,
    emitted: &mut BTreeMap<String, String>,
) -> Result<(), ModuleSdkCodegenError> {
    if property.type_name.starts_with("Edm.")
        || property.type_name == temper_wasm_sdk::data::FAILURE_ENVELOPE_CSDL_TYPE_V1
    {
        return Ok(());
    }
    let base_name = rust_type_name(&property.type_name);
    let generated = if property.enum_members.is_empty() {
        format!("{base_name}Id")
    } else {
        base_name
    };
    let owner = format!("named CSDL type '{}'", property.type_name);
    if !register_generated_type(emitted, generated.clone(), owner)? {
        return Ok(());
    }
    if property.enum_members.is_empty() {
        emit_id_types(source, &generated);
    } else {
        source.push_str(&format!("#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize)]\npub enum {generated} {{\n"));
        let mut variants = BTreeSet::new();
        for member in &property.enum_members {
            let variant = checked_enum_variant(&generated, member)?;
            if !variants.insert(variant.clone()) {
                return Err(ModuleSdkCodegenError::IdentifierCollision(format!(
                    "{generated}::{variant}"
                )));
            }
            source.push_str(&format!(
                "    #[serde(rename = {})]\n    {variant},\n",
                format_args!("{member:?}")
            ));
        }
        source.push_str("}\n\n");
    }
    Ok(())
}

pub(super) fn emit_id_types(source: &mut String, generated: &str) {
    source.push_str(&format!(
        "#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize)]\n#[serde(transparent)]\npub struct {generated}(pub String);\nimpl AsRef<str> for {generated} {{ fn as_ref(&self) -> &str {{ &self.0 }} }}\n\n#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]\n#[serde(transparent)]\npub struct {generated}Ref<'a>(pub &'a str);\nimpl<'a> {generated}Ref<'a> {{ pub const fn new(value: &'a str) -> Self {{ Self(value) }} }}\nimpl<'a> From<&'a {generated}> for {generated}Ref<'a> {{ fn from(value: &'a {generated}) -> Self {{ Self(&value.0) }} }}\nimpl AsRef<str> for {generated}Ref<'_> {{ fn as_ref(&self) -> &str {{ self.0 }} }}\n\n"
    ));
}

pub(super) fn register_generated_type(
    emitted: &mut BTreeMap<String, String>,
    generated: String,
    owner: String,
) -> Result<bool, ModuleSdkCodegenError> {
    if let Some(existing) = emitted.get(&generated) {
        if existing == &owner {
            return Ok(false);
        }
        return Err(ModuleSdkCodegenError::IdentifierCollision(format!(
            "{generated} from {existing} and {owner}"
        )));
    }
    emitted.insert(generated, owner);
    Ok(true)
}

pub(super) fn generated_rust_type(
    property: &ManifestPropertyV1,
    string_lifecycle_type: Option<&str>,
) -> String {
    if property.source == ManifestValueSourceV1::LifecycleStatus
        && property.type_name == "Edm.String"
    {
        string_lifecycle_type.unwrap_or("String").into()
    } else if property.type_name == temper_wasm_sdk::data::FAILURE_ENVELOPE_CSDL_TYPE_V1 {
        "FailureEnvelopeV1".into()
    } else if property.type_name.starts_with("Edm.") {
        rust_scalar_type(&property.type_name).into()
    } else if property.enum_members.is_empty() {
        format!("{}Id", rust_type_name(&property.type_name))
    } else {
        rust_type_name(&property.type_name)
    }
}

pub(super) fn generated_command_type(
    property: &ManifestPropertyV1,
    generated_entity: Option<&str>,
) -> String {
    if property.source == ManifestValueSourceV1::EntityId {
        return format!(
            "{}IdRef<'a>",
            generated_entity.expect("entity ID command field must name its generated entity")
        );
    }
    if property.type_name == temper_wasm_sdk::data::FAILURE_ENVELOPE_CSDL_TYPE_V1 {
        "&'a FailureEnvelopeV1".into()
    } else if matches!(
        property.type_name.as_str(),
        "Edm.Boolean"
            | "Edm.Byte"
            | "Edm.Int16"
            | "Edm.Int32"
            | "Edm.Int64"
            | "Edm.Single"
            | "Edm.Double"
    ) {
        generated_rust_type(property, None)
    } else if property.type_name.starts_with("Edm.") {
        "&'a str".into()
    } else if property.enum_members.is_empty() {
        format!("{}IdRef<'a>", rust_type_name(&property.type_name))
    } else {
        format!("&'a {}", rust_type_name(&property.type_name))
    }
}

pub(super) fn generated_type_name(type_name: &str, enum_members: &[String]) -> String {
    if type_name == temper_wasm_sdk::data::FAILURE_ENVELOPE_CSDL_TYPE_V1 {
        "FailureEnvelopeV1".into()
    } else if type_name.starts_with("Edm.") {
        rust_scalar_type(type_name).into()
    } else if !enum_members.is_empty() {
        rust_type_name(type_name)
    } else {
        format!("{}Id", rust_type_name(type_name))
    }
}

pub(super) fn emit_query_types(
    source: &mut String,
    generated: &str,
    properties: &[ManifestPropertyV1],
    grant: &EntityDataGrant,
    string_lifecycle_type: Option<&str>,
) {
    source.push_str(&format!("pub struct {generated}Filter(FilterV1);\nimpl {generated}Filter {{\n    pub fn and(operands: Vec<Self>) -> Self {{ Self(FilterV1::And {{ operands: operands.into_iter().map(|value| value.0).collect() }}) }}\n    pub fn or(operands: Vec<Self>) -> Self {{ Self(FilterV1::Or {{ operands: operands.into_iter().map(|value| value.0).collect() }}) }}\n    pub fn not(operand: Self) -> Self {{ Self(FilterV1::Not {{ operand: Box::new(operand.0) }}) }}\n"));
    for property in properties
        .iter()
        .filter(|property| grant.query_filter_fields.contains(&property.canonical_name))
    {
        let field = &property.generated_name;
        let canonical = &property.canonical_name;
        let rust_type = generated_rust_type(property, string_lifecycle_type);
        let scalar = scalar_expression(property, "value", string_lifecycle_type);
        source.push_str(&format!("    pub fn {field}_eq(value: {rust_type}) -> Self {{ Self(FilterV1::Compare {{ field: \"{canonical}\".into(), operator: CompareOperatorV1::Eq, value: {scalar} }}) }}\n    pub fn {field}_ne(value: {rust_type}) -> Self {{ Self(FilterV1::Compare {{ field: \"{canonical}\".into(), operator: CompareOperatorV1::Ne, value: {scalar} }}) }}\n"));
        if property.type_name != "Edm.Boolean" {
            source.push_str(&format!("    pub fn {field}_lt(value: {rust_type}) -> Self {{ Self(FilterV1::Compare {{ field: \"{canonical}\".into(), operator: CompareOperatorV1::Lt, value: {scalar} }}) }}\n    pub fn {field}_le(value: {rust_type}) -> Self {{ Self(FilterV1::Compare {{ field: \"{canonical}\".into(), operator: CompareOperatorV1::Le, value: {scalar} }}) }}\n    pub fn {field}_gt(value: {rust_type}) -> Self {{ Self(FilterV1::Compare {{ field: \"{canonical}\".into(), operator: CompareOperatorV1::Gt, value: {scalar} }}) }}\n    pub fn {field}_ge(value: {rust_type}) -> Self {{ Self(FilterV1::Compare {{ field: \"{canonical}\".into(), operator: CompareOperatorV1::Ge, value: {scalar} }}) }}\n"));
        }
        if property.nullable {
            source.push_str(&format!("    pub fn {field}_is_null(is_null: bool) -> Self {{ Self(FilterV1::IsNull {{ field: \"{canonical}\".into(), is_null }}) }}\n"));
        }
    }
    source.push_str("}\n\n");
    source.push_str(&format!(
        "pub struct {generated}Order(OrderV1);\nimpl {generated}Order {{\n"
    ));
    for property in properties
        .iter()
        .filter(|property| grant.query_order_fields.contains(&property.canonical_name))
    {
        source.push_str(&format!("    pub fn {}(direction: OrderDirectionV1) -> Self {{ Self(OrderV1::property(\"{}\", direction)) }}\n", property.generated_name, property.canonical_name));
    }
    if grant.query_order_by_sequence {
        source.push_str("    pub fn commit_sequence(direction: OrderDirectionV1) -> Self { Self(OrderV1::EntityCommitSequence { direction }) }\n");
    }
    source.push_str("}\n\n");
}

fn scalar_expression(
    property: &ManifestPropertyV1,
    value: &str,
    string_lifecycle_type: Option<&str>,
) -> String {
    if property.source == ManifestValueSourceV1::LifecycleStatus
        && property.type_name == "Edm.String"
    {
        debug_assert!(string_lifecycle_type.is_some());
        return format!(
            "ScalarV1::String(serde_json::to_value({value}).expect(\"generated lifecycle enum serializes\").as_str().expect(\"generated lifecycle enum is a string\").into())"
        );
    }
    match property.type_name.as_str() {
        "Edm.Boolean" => format!("ScalarV1::Boolean({value})"),
        "Edm.Byte" | "Edm.Int16" | "Edm.Int32" | "Edm.Int64" => format!("ScalarV1::Int64({value})"),
        "Edm.Single" | "Edm.Double" => format!("ScalarV1::Double({value})"),
        "Edm.Guid" => format!("ScalarV1::Guid({value})"),
        "Edm.DateTimeOffset" => format!("ScalarV1::DateTimeOffset({value})"),
        "Edm.Decimal" => format!("ScalarV1::Decimal({value})"),
        "Edm.String" => format!("ScalarV1::String({value})"),
        _ if property.enum_members.is_empty() => format!("ScalarV1::String({value}.0)"),
        _ => format!(
            "ScalarV1::Enum(EnumValueV1 {{ type_name: \"{}\".into(), member: serde_json::to_value({value}).expect(\"generated enum serializes\").as_str().expect(\"generated enum is a string\").into() }})",
            property.type_name
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use temper_wasm_sdk::data::{FAILURE_ENVELOPE_CSDL_TYPE_V1, ManifestValueSourceV1};

    fn failure_property() -> ManifestPropertyV1 {
        ManifestPropertyV1 {
            canonical_name: "Failure".into(),
            generated_name: "failure".into(),
            type_name: FAILURE_ENVELOPE_CSDL_TYPE_V1.into(),
            nullable: false,
            source: ManifestValueSourceV1::Input,
            default_value: None,
            enum_members: Vec::new(),
            write_policy: None,
        }
    }

    #[test]
    fn canonical_failure_parameter_uses_shared_envelope_not_reference_id() {
        let property = failure_property();
        assert_eq!(generated_rust_type(&property, None), "FailureEnvelopeV1");
        assert_eq!(
            generated_type_name(&property.type_name, &property.enum_members),
            "FailureEnvelopeV1"
        );

        let mut source = String::new();
        let mut emitted = BTreeMap::new();
        emit_named_property_type(&mut source, &property, &mut emitted).unwrap();
        assert!(
            source.is_empty(),
            "shared type must not generate an ID wrapper"
        );
    }

    #[test]
    fn enum_backed_named_types_fail_on_entity_and_enum_name_collisions() {
        let property = |type_name: &str| ManifestPropertyV1 {
            canonical_name: "State".into(),
            generated_name: "state".into(),
            type_name: type_name.into(),
            nullable: false,
            source: ManifestValueSourceV1::LifecycleStatus,
            default_value: None,
            enum_members: vec!["Open".into()],
            write_policy: None,
        };
        let mut emitted = BTreeMap::from([(
            "ExampleStatus".into(),
            "entity type 'Other.ExampleStatus'".into(),
        )]);
        assert!(matches!(
            emit_named_property_type(
                &mut String::new(),
                &property("Example.Status"),
                &mut emitted
            ),
            Err(ModuleSdkCodegenError::IdentifierCollision(_))
        ));

        let mut emitted = BTreeMap::new();
        emit_named_property_type(
            &mut String::new(),
            &property("Example.Task-State"),
            &mut emitted,
        )
        .unwrap();
        assert!(matches!(
            emit_named_property_type(
                &mut String::new(),
                &property("Example.Task_State"),
                &mut emitted
            ),
            Err(ModuleSdkCodegenError::IdentifierCollision(_))
        ));
    }

    #[test]
    fn entity_value_and_entity_id_types_cannot_share_a_rust_name() {
        let mut emitted = BTreeMap::new();
        register_generated_type(
            &mut emitted,
            "FooId".into(),
            "entity ID type for 'Example.Foo'".into(),
        )
        .unwrap();
        assert!(matches!(
            register_generated_type(
                &mut emitted,
                "FooId".into(),
                "entity type 'Example.FooId'".into()
            ),
            Err(ModuleSdkCodegenError::IdentifierCollision(_))
        ));
    }
}
