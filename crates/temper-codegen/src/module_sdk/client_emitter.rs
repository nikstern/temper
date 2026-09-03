//! Emission of capability-scoped entity clients.

use std::collections::BTreeMap;

use temper_wasm_sdk::data::{
    DataOperationKind, EntityDataGrant, FileOperationKind, ManifestActionResultCardinalityV1,
    ManifestActionV1, ManifestCreateRoleV1, ManifestPatchRoleV1, ManifestPropertyV1,
    ModuleDataGrant,
};

use super::names::rust_type_name;
use super::source_types::{emit_query_types, generated_command_type, generated_type_name};

pub(super) struct EntityClientSpec<'a> {
    pub(super) generated: &'a str,
    pub(super) canonical: &'a str,
    pub(super) properties: &'a [ManifestPropertyV1],
    pub(super) actions: &'a [ManifestActionV1],
    pub(super) grant: &'a ModuleDataGrant,
    pub(super) entity_grant: &'a EntityDataGrant,
    pub(super) generated_entity_names: &'a BTreeMap<String, String>,
    pub(super) string_lifecycle_type: Option<&'a str>,
}

fn emit_create_command(source: &mut String, generated: &str, properties: &[ManifestPropertyV1]) {
    let admitted = properties
        .iter()
        .filter(|property| {
            property
                .write_policy
                .expect("current generated property must carry write policy")
                .create
                != ManifestCreateRoleV1::Forbidden
        })
        .collect::<Vec<_>>();
    source.push_str(&format!(
        "#[derive(Debug, Clone, serde::Serialize)]\npub struct {generated}Create<'a> {{\n"
    ));
    for property in &admitted {
        let command_type = generated_command_type(property, Some(generated));
        let policy = property.write_policy.expect("checked generated policy");
        let (field_type, serde) = if policy.create == ManifestCreateRoleV1::Required {
            (
                command_type,
                format!("#[serde(rename = {:?})]", property.canonical_name),
            )
        } else if property.nullable {
            (
                format!("Option<Option<{command_type}>>"),
                format!(
                    "#[serde(rename = {:?}, skip_serializing_if = \"Option::is_none\")]",
                    property.canonical_name
                ),
            )
        } else {
            (
                format!("Option<{command_type}>"),
                format!(
                    "#[serde(rename = {:?}, skip_serializing_if = \"Option::is_none\")]",
                    property.canonical_name
                ),
            )
        };
        source.push_str(&format!(
            "    {serde}\n    {}: {field_type},\n",
            property.generated_name
        ));
    }
    source.push_str("    #[serde(skip)]\n    _borrow: std::marker::PhantomData<&'a ()>,\n}\n");
    let required = admitted
        .iter()
        .filter(|property| {
            property
                .write_policy
                .expect("checked generated policy")
                .create
                == ManifestCreateRoleV1::Required
        })
        .collect::<Vec<_>>();
    let params = required
        .iter()
        .map(|property| {
            format!(
                "{}: {}",
                property.generated_name,
                generated_command_type(property, Some(generated))
            )
        })
        .collect::<Vec<_>>()
        .join(", ");
    source.push_str(&format!(
        "impl<'a> {generated}Create<'a> {{\n    pub fn new({params}) -> Self {{ Self {{\n"
    ));
    for property in &admitted {
        if property
            .write_policy
            .expect("checked generated policy")
            .create
            == ManifestCreateRoleV1::Required
        {
            source.push_str(&format!(
                "        {}: {},\n",
                property.generated_name, property.generated_name
            ));
        } else {
            source.push_str(&format!("        {}: None,\n", property.generated_name));
        }
    }
    source.push_str("        _borrow: std::marker::PhantomData,\n    } }\n");
    for property in admitted.iter().filter(|property| {
        property
            .write_policy
            .expect("checked generated policy")
            .create
            == ManifestCreateRoleV1::Optional
    }) {
        let field = &property.generated_name;
        let command_type = generated_command_type(property, Some(generated));
        if property.nullable {
            source.push_str(&format!(
                "    pub fn with_{field}(mut self, value: {command_type}) -> Self {{ self.{field} = Some(Some(value)); self }}\n    pub fn with_{field}_null(mut self) -> Self {{ self.{field} = Some(None); self }}\n"
            ));
        } else {
            source.push_str(&format!(
                "    pub fn with_{field}(mut self, value: {command_type}) -> Self {{ self.{field} = Some(value); self }}\n"
            ));
        }
    }
    source.push_str("}\n\n");
}

fn emit_patch_command(source: &mut String, generated: &str, properties: &[ManifestPropertyV1]) {
    let admitted = properties
        .iter()
        .filter(|property| {
            property
                .write_policy
                .expect("current generated property must carry write policy")
                .patch
                == ManifestPatchRoleV1::Writable
        })
        .collect::<Vec<_>>();
    source.push_str(&format!(
        "#[derive(Debug, Clone, Default, serde::Serialize)]\npub struct {generated}Patch<'a> {{\n"
    ));
    for property in &admitted {
        let command_type = generated_command_type(property, Some(generated));
        let field_type = if property.nullable {
            format!("NullablePatch<{command_type}>")
        } else {
            format!("Option<{command_type}>")
        };
        let skip = if property.nullable {
            "NullablePatch::is_unchanged"
        } else {
            "Option::is_none"
        };
        source.push_str(&format!(
            "    #[serde(rename = {:?}, skip_serializing_if = \"{skip}\")]\n    {}: {field_type},\n",
            property.canonical_name, property.generated_name
        ));
    }
    source.push_str("    #[serde(skip)]\n    _borrow: std::marker::PhantomData<&'a ()>,\n}\n");
    source.push_str(&format!(
        "impl<'a> {generated}Patch<'a> {{\n    pub fn new() -> Self {{ Self::default() }}\n"
    ));
    for property in &admitted {
        let field = &property.generated_name;
        let command_type = generated_command_type(property, Some(generated));
        if property.nullable {
            source.push_str(&format!(
                "    pub fn with_{field}(mut self, value: {command_type}) -> Self {{ self.{field} = NullablePatch::Value(value); self }}\n    pub fn with_{field}_null(mut self) -> Self {{ self.{field} = NullablePatch::Null; self }}\n"
            ));
        } else {
            source.push_str(&format!(
                "    pub fn with_{field}(mut self, value: {command_type}) -> Self {{ self.{field} = Some(value); self }}\n"
            ));
        }
    }
    source.push_str("}\n\n");
}

fn emit_action_command(source: &mut String, generated: &str, action: &ManifestActionV1) {
    let action_type = format!(
        "{}{}Input",
        generated,
        rust_type_name(&action.canonical_name)
    );
    source.push_str(&format!(
        "#[derive(Debug, Clone, serde::Serialize)]\npub struct {action_type}<'a> {{\n"
    ));
    for parameter in &action.parameters {
        let command_type = generated_command_type(parameter, None);
        let optional = parameter.nullable || parameter.default_value.is_some();
        let field_type = if optional && parameter.nullable {
            format!("Option<Option<{command_type}>>")
        } else if optional {
            format!("Option<{command_type}>")
        } else {
            command_type
        };
        let skip = if optional {
            ", skip_serializing_if = \"Option::is_none\""
        } else {
            ""
        };
        source.push_str(&format!(
            "    #[serde(rename = {:?}{skip})]\n    {}: {field_type},\n",
            parameter.canonical_name, parameter.generated_name
        ));
    }
    source.push_str("    #[serde(skip)]\n    _borrow: std::marker::PhantomData<&'a ()>,\n}\n");
    let required = action
        .parameters
        .iter()
        .filter(|parameter| !parameter.nullable && parameter.default_value.is_none())
        .collect::<Vec<_>>();
    let params = required
        .iter()
        .map(|parameter| {
            format!(
                "{}: {}",
                parameter.generated_name,
                generated_command_type(parameter, None)
            )
        })
        .collect::<Vec<_>>()
        .join(", ");
    source.push_str(&format!(
        "impl<'a> {action_type}<'a> {{\n    pub fn new({params}) -> Self {{ Self {{\n"
    ));
    for parameter in &action.parameters {
        if !parameter.nullable && parameter.default_value.is_none() {
            source.push_str(&format!(
                "        {}: {},\n",
                parameter.generated_name, parameter.generated_name
            ));
        } else {
            source.push_str(&format!("        {}: None,\n", parameter.generated_name));
        }
    }
    source.push_str("        _borrow: std::marker::PhantomData,\n    } }\n");
    for parameter in action
        .parameters
        .iter()
        .filter(|parameter| parameter.nullable || parameter.default_value.is_some())
    {
        let field = &parameter.generated_name;
        let command_type = generated_command_type(parameter, None);
        if parameter.nullable {
            source.push_str(&format!(
                "    pub fn with_{field}(mut self, value: {command_type}) -> Self {{ self.{field} = Some(Some(value)); self }}\n    pub fn with_{field}_null(mut self) -> Self {{ self.{field} = Some(None); self }}\n"
            ));
        } else {
            source.push_str(&format!(
                "    pub fn with_{field}(mut self, value: {command_type}) -> Self {{ self.{field} = Some(value); self }}\n"
            ));
        }
    }
    source.push_str("}\n\n");
}

pub(super) fn emit_entity_client(source: &mut String, spec: EntityClientSpec<'_>) {
    let EntityClientSpec {
        generated,
        canonical,
        properties,
        actions,
        grant,
        entity_grant,
        generated_entity_names,
        string_lifecycle_type,
    } = spec;
    let entity_get = grant.permits(DataOperationKind::EntityGet, canonical, None);
    let entity_query = grant.permits(DataOperationKind::EntityQuery, canonical, None);
    let entity_create = grant.permits(DataOperationKind::EntityCreate, canonical, None);
    let entity_create_or_verify =
        grant.permits(DataOperationKind::EntityCreateOrVerify, canonical, None);
    let entity_patch = grant.permits(DataOperationKind::EntityPatch, canonical, None);

    if entity_create || entity_create_or_verify {
        emit_create_command(source, generated, properties);
    }
    if entity_patch {
        emit_patch_command(source, generated, properties);
    }
    for action in actions {
        let operation_granted = if action.composite {
            grant
                .operations
                .contains(&temper_wasm_sdk::data::DataOperationKind::CompositeInvoke)
        } else {
            grant
                .operations
                .contains(&temper_wasm_sdk::data::DataOperationKind::ActionInvoke)
        };
        if !operation_granted {
            continue;
        }
        emit_action_command(source, generated, action);
    }
    if entity_query {
        emit_query_types(
            source,
            generated,
            properties,
            entity_grant,
            string_lifecycle_type,
        );
    }
    source.push_str(&format!(
        "pub struct {generated}Client {{ data: DataClient }}\nimpl {generated}Client {{\n    pub const ENTITY_TYPE: &'static str = \"{canonical}\";\n    pub fn new() -> Self {{ Self {{ data: DataClient::default() }} }}\n"
    ));
    if entity_get {
        source.push_str(&format!("    pub fn get(&mut self, id: impl AsRef<str>) -> Result<TypedEntity<{generated}>, ModuleDataError> {{ decode_entity(self.data.call(DataOperationV1::EntityGet {{ entity_type: Self::ENTITY_TYPE.into(), entity_id: id.as_ref().into(), at_least_sequence: None }})?) }}\n    pub fn get_at_least(&mut self, id: impl AsRef<str>, at_least_sequence: u64) -> Result<TypedEntity<{generated}>, ModuleDataError> {{ decode_entity(self.data.call(DataOperationV1::EntityGet {{ entity_type: Self::ENTITY_TYPE.into(), entity_id: id.as_ref().into(), at_least_sequence: Some(at_least_sequence) }})?) }}\n    pub fn read_committed(&mut self, commit: &CommitToken) -> Result<TypedEntity<{generated}>, ModuleDataError> {{ if commit.entity_type != Self::ENTITY_TYPE {{ return Err(ModuleDataError::new(ModuleDataErrorKind::SchemaMismatch, \"CommitTokenEntityTypeMismatch\", \"commit token belongs to a different entity type\", temper_wasm_sdk::FailureRetryability::Never, temper_wasm_sdk::FailureOutcome::NotApplied).expect(\"static commit-token mismatch contract must be valid\")); }} self.get_at_least(&commit.entity_id, commit.sequence) }}\n"));
    }
    if entity_query {
        source.push_str(&format!("    pub fn query(&mut self, filter: Option<{generated}Filter>, order_by: Vec<{generated}Order>, page: PageV1) -> Result<TypedPage<{generated}>, ModuleDataError> {{ decode_page(self.data.call(DataOperationV1::EntityQuery {{ entity_type: Self::ENTITY_TYPE.into(), filter: filter.map(|value| value.0), order_by: order_by.into_iter().map(|value| value.0).collect(), page }})?) }}\n"));
    }
    if entity_create {
        source.push_str(&format!("    pub fn create(&mut self, value: &{generated}Create<'_>) -> Result<TypedWrite<{generated}>, ModuleDataError> {{ decode_write(self.data.call(DataOperationV1::EntityCreate {{ entity_type: Self::ENTITY_TYPE.into(), value: encode_command_object(value)? }})?) }}\n"));
    }
    if entity_create_or_verify {
        source.push_str(&format!("    pub fn create_or_verify(&mut self, idempotency_key: impl Into<String>, value: &{generated}Create<'_>) -> Result<CreateOrVerifyOutcome<{generated}>, ModuleDataError> {{ decode_create_or_verify(self.data.call(DataOperationV1::EntityCreateOrVerify {{ entity_type: Self::ENTITY_TYPE.into(), idempotency_key: idempotency_key.into(), value: encode_command_object(value)? }})?) }}\n"));
    }
    if entity_patch {
        source.push_str(&format!("    pub fn patch(&mut self, id: impl AsRef<str>, expected_sequence: Option<u64>, value: &{generated}Patch<'_>) -> Result<TypedWrite<{generated}>, ModuleDataError> {{ decode_write(self.data.call(DataOperationV1::EntityPatch {{ entity_type: Self::ENTITY_TYPE.into(), entity_id: id.as_ref().into(), expected_sequence, value: encode_command_object(value)? }})?) }}\n"));
    }
    for action in actions {
        let operation_granted = if action.composite {
            grant
                .operations
                .contains(&temper_wasm_sdk::data::DataOperationKind::CompositeInvoke)
        } else {
            grant
                .operations
                .contains(&temper_wasm_sdk::data::DataOperationKind::ActionInvoke)
        };
        if !operation_granted {
            continue;
        }
        let method = &action.generated_name;
        let action_type = format!(
            "{}{}Input",
            generated,
            rust_type_name(&action.canonical_name)
        );
        let operation = if action.composite {
            "CompositeInvoke"
        } else {
            "ActionInvoke"
        };
        let base_result_type = action.result_type.as_deref().map_or_else(
            || "()".to_string(),
            |type_name| {
                generated_entity_names
                    .get(type_name)
                    .cloned()
                    .unwrap_or_else(|| generated_type_name(type_name, &action.result_enum_members))
            },
        );
        let result_type = match action
            .result_cardinality
            .expect("current generated action must carry result cardinality")
        {
            ManifestActionResultCardinalityV1::Void => "()".to_string(),
            ManifestActionResultCardinalityV1::Required => base_result_type,
            ManifestActionResultCardinalityV1::Nullable => {
                format!("Option<{base_result_type}>")
            }
        };
        source.push_str(&format!("    pub fn {method}(&mut self, id: impl AsRef<str>, expected_sequence: Option<u64>, params: &{action_type}<'_>) -> Result<TypedAction<{result_type}>, ModuleDataError> {{ decode_action(self.data.call(DataOperationV1::{operation} {{ entity_type: Self::ENTITY_TYPE.into(), entity_id: id.as_ref().into(), action: \"{}\".into(), expected_sequence, params: encode_command_object(params)? }})?) }}\n", action.canonical_name));
    }
    if grant.operations.contains(&DataOperationKind::FileRead)
        && entity_grant
            .file_operations
            .contains(&FileOperationKind::ContentRead)
    {
        source.push_str("    pub fn open_file_read(&mut self, file_id: impl Into<String>) -> Result<OpenedFileRead, ModuleDataError> { decode_file_read(self.data.call(DataOperationV1::FileReadOpen { file_id: file_id.into(), version_id: None })?) }\n");
    }
    if grant.operations.contains(&DataOperationKind::FileRead)
        && entity_grant
            .file_operations
            .contains(&FileOperationKind::VersionRead)
    {
        source.push_str("    pub fn open_file_version_read(&mut self, file_id: impl Into<String>, version_id: impl Into<String>) -> Result<OpenedFileRead, ModuleDataError> { decode_file_read(self.data.call(DataOperationV1::FileReadOpen { file_id: file_id.into(), version_id: Some(version_id.into()) })?) }\n");
    }
    if grant.operations.contains(&DataOperationKind::FileWrite)
        && entity_grant
            .file_operations
            .contains(&FileOperationKind::ContentWrite)
    {
        source.push_str("    pub fn open_file_write(&mut self, file_id: impl Into<String>, expected_sequence: Option<u64>, content_length: Option<u64>, content_hash: Option<String>) -> Result<FileWriter, ModuleDataError> { decode_file_write(self.data.call(DataOperationV1::FileWriteOpen { file_id: file_id.into(), expected_sequence, content_length, content_hash })?) }\n");
        source.push_str("    pub fn commit_file_write(&mut self, stream_handle: u32) -> Result<DataResultV1, ModuleDataError> { self.data.call(DataOperationV1::FileWriteCommit { stream_handle }) }\n");
        source.push_str("    pub fn abort_file_stream(&mut self, stream_handle: u32) -> Result<DataResultV1, ModuleDataError> { self.data.call(DataOperationV1::FileStreamAbort { stream_handle }) }\n");
    }
    source.push_str("}\n\n");
}
