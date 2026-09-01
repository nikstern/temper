use temper_wasm_sdk::data::ModuleSdkManifest;

/// A fail-closed schema or naming error during module SDK generation.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ModuleSdkCodegenError {
    #[error("invalid module data grant: {0}")]
    InvalidGrant(String),
    #[error("granted entity type '{0}' is absent from the verified CSDL closure")]
    MissingEntity(String),
    #[error("entity type '{0}' has no entity set in the verified CSDL closure")]
    MissingEntitySet(String),
    #[error("granted schema symbol '{symbol}' is absent from entity '{entity_type}'")]
    MissingSymbol { entity_type: String, symbol: String },
    #[error(
        "granted bound action '{action}' is ambiguous on entity '{entity_type}': {matches} exact overloads match"
    )]
    AmbiguousBoundAction {
        entity_type: String,
        action: String,
        matches: usize,
    },
    #[error("generated Rust identifier collision '{0}'")]
    IdentifierCollision(String),
    #[error("invalid default '{value}' for schema symbol '{symbol}' of type '{type_name}'")]
    InvalidDefault {
        symbol: String,
        type_name: String,
        value: String,
    },
    #[error("granted entity type '{0}' has no verified IOA source")]
    MissingIoaSource(String),
    #[error("verified IOA source for '{entity_type}' is invalid: {message}")]
    InvalidIoaSource {
        entity_type: String,
        message: String,
    },
    #[error("entity '{entity_type}' has unsupported canonical key properties {key_properties:?}")]
    UnsupportedEntityKey {
        entity_type: String,
        key_properties: Vec<String>,
    },
    #[error(
        "entity '{entity_type}' has no canonical lifecycle property for IOA initial state '{initial_state}'"
    )]
    MissingLifecycleProperty {
        entity_type: String,
        initial_state: String,
    },
    #[error(
        "entity '{entity_type}' has ambiguous canonical lifecycle properties {candidates:?} for IOA initial state '{initial_state}'"
    )]
    AmbiguousLifecycleProperty {
        entity_type: String,
        initial_state: String,
        candidates: Vec<String>,
    },
    #[error(
        "entity '{entity_type}' lifecycle property '{property}' default does not match IOA initial state '{initial_state}'"
    )]
    LifecycleDefaultMismatch {
        entity_type: String,
        property: String,
        initial_state: String,
    },
    #[error(
        "bound action '{action}' on '{entity_type}' returns different entity type '{result_type}'"
    )]
    UnsupportedEntityResult {
        action: String,
        entity_type: String,
        result_type: String,
    },
    #[error("failed to construct module SDK manifest: {0}")]
    Manifest(String),
    #[error("invalid verified stream capability: {0}")]
    StreamCapability(String),
    #[error("failed to bind compiled module artifact: {0}")]
    ArtifactBinding(String),
}

/// Generated Rust source and the activation manifest that binds it.
#[derive(Debug, Clone)]
pub struct GeneratedModuleSdk {
    /// Complete generated Rust guest source.
    pub source: String,
    /// Canonical activation binding packaged beside the compiled artifact.
    pub manifest: ModuleSdkManifest,
}

/// Compiled WASM bytes with an artifact-carried SDK binding and matching sidecar.
#[derive(Debug, Clone)]
pub struct PackagedModuleSdk {
    /// WebAssembly bytes containing the host-readable custom section.
    pub wasm: Vec<u8>,
    /// Sidecar manifest whose artifact digest covers the exact packaged bytes.
    pub manifest: ModuleSdkManifest,
}
