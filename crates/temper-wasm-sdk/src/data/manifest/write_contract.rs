/// Canonical property metadata used for typed generation and host validation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ManifestPropertyV1 {
    /// Case-sensitive CSDL property or parameter name.
    pub canonical_name: String,
    /// Generated Rust field name.
    pub generated_name: String,
    /// Fully qualified CSDL scalar, enum, or reference type.
    pub type_name: String,
    /// Whether the canonical value may be null.
    pub nullable: bool,
    /// Immutable authority that supplies this canonical value.
    pub source: ManifestValueSourceV1,
    /// Generation-validated canonical JSON value for the declared CSDL default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_value: Option<serde_json::Value>,
    /// Closed enum members, empty for non-enum properties.
    #[serde(default)]
    pub enum_members: Vec<String>,
    /// Operation-specific caller write admission for this entity property.
    ///
    /// Absent only on authenticated historical manifests whose canonical JSON
    /// predates module SDK contract version 2.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub write_policy: Option<ManifestPropertyWritePolicyV1>,
}

/// Closed caller write admission for one canonical entity property.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ManifestPropertyWritePolicyV1 {
    /// Admission for entity creation.
    pub create: ManifestCreateRoleV1,
    /// Admission for entity patching.
    pub patch: ManifestPatchRoleV1,
}

/// Closed create-input role for one canonical entity property.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ManifestCreateRoleV1 {
    /// The caller must supply the property.
    Required,
    /// The caller may omit the property for host default or null materialization.
    Optional,
    /// The caller must not supply the property.
    Forbidden,
}

/// Closed patch-input role for one canonical entity property.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ManifestPatchRoleV1 {
    /// The caller may patch the property.
    Writable,
    /// The caller must not patch the property.
    Forbidden,
}

/// Closed authority for one generated canonical value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ManifestValueSourceV1 {
    /// Value supplied in an action input object.
    Input,
    /// Value read from committed sparse entity fields.
    StoredField,
    /// Host-owned immutable entity identifier.
    EntityId,
    /// Host-owned persisted IOA lifecycle status.
    LifecycleStatus,
}

/// Canonical action metadata used for typed generation and host validation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ManifestActionV1 {
    /// Case-sensitive IOA/CSDL action name.
    pub canonical_name: String,
    /// Generated Rust method name.
    pub generated_name: String,
    /// Non-binding action parameters.
    #[serde(default)]
    pub parameters: Vec<ManifestPropertyV1>,
    /// Canonical result type when the action returns a value.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result_type: Option<String>,
    /// Closed enum members for an enum result type.
    #[serde(default)]
    pub result_enum_members: Vec<String>,
    /// Canonical result cardinality for generated decoding and helpers.
    ///
    /// Absent only on authenticated historical manifests whose canonical JSON
    /// predates module SDK contract version 2.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result_cardinality: Option<ManifestActionResultCardinalityV1>,
    /// Whether this action uses the verified composite-action path.
    pub composite: bool,
}

/// Closed canonical result cardinality for a bound action.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ManifestActionResultCardinalityV1 {
    /// The action has no declared result.
    Void,
    /// The action declares a non-nullable result.
    Required,
    /// The action declares a nullable result.
    Nullable,
}
