//! Shared event, index, and persistence value types.

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

use super::stream_descriptor::KernelEventMetadata;

/// Event type used for the parent-journal record of a Composite action.
pub const COMPOSITE_EVENT_TYPE: &str = "CompositeEvent";

/// Replay/audit record for one Composite action application.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompositeEvent {
    pub tenant: String,
    pub parent_entity_type: String,
    pub parent_entity_id: String,
    pub parent_action: String,
    pub composite_idempotency_key: String,
    pub sub_writes: Vec<CompositeEventSubWrite>,
}

/// One concrete sub-write recorded in a [`CompositeEvent`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompositeEventSubWrite {
    pub index: usize,
    pub entity_type: String,
    pub entity_id: String,
    pub action: String,
    pub idempotency_key: String,
}

/// Marker trait for serializable domain events.
pub trait DomainEvent:
    Send + Serialize + for<'de> Deserialize<'de> + std::fmt::Debug + 'static
{
}

/// Metadata attached to every persisted event.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EventMetadata {
    pub event_id: uuid::Uuid,
    pub causation_id: uuid::Uuid,
    pub correlation_id: uuid::Uuid,
    pub timestamp: chrono::DateTime<chrono::Utc>,
    pub actor_id: String,
    /// Reserved, versioned metadata minted and validated by the kernel.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kernel: Option<KernelEventMetadata>,
}

/// Trait for event-sourced persistent actors.
pub trait PersistentActor: Send + 'static {
    type Event: DomainEvent;
    type State: Send + Serialize + for<'de> Deserialize<'de> + 'static;

    fn persistence_id(&self) -> &str;
    fn apply_event(state: &mut Self::State, event: &Self::Event);
    fn snapshot_every(&self) -> u64 {
        100
    }
}

/// A declared-key row to co-commit with an append.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EntityKeyRow {
    pub key_name: String,
    pub key_hash: String,
}

/// Current durable creation-contract encoding version.
pub const CREATION_CONTRACT_VERSION_V1: u32 = 1;

/// Maximum number of schema-owned field identifiers disclosed for one conflict.
pub const CREATE_OR_VERIFY_CONFLICT_FIELD_BUDGET: usize = 32;

/// One ordered field in an immutable canonical sequence-1 creation contract.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreationContractField {
    /// Canonical schema property identifier.
    pub name: String,
    /// Canonical type descriptor, including reference target or enum type.
    pub type_descriptor: String,
    /// Canonical manifest value-source name.
    pub value_source: String,
    /// Whether canonical null is admitted.
    pub nullable: bool,
    /// Whether create admission requires the caller-visible property.
    ///
    /// Older persisted contracts omitted this marker. Preserve that absence so
    /// comparison can fail closed with `MigrationRequired` on every backend.
    #[serde(default)]
    pub create_required: Option<bool>,
    /// Digest of the canonical default/null rule, distinct from the field value.
    pub default_digest: String,
    /// Domain-separated digest of the canonical sequence-1 field value.
    pub value_digest: String,
}

/// Versioned immutable comparison authority for one entity creation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreationContract {
    /// Contract encoding version.
    pub version: u32,
    /// Exact verified global schema or scoped bundle digest.
    pub schema_digest: String,
    /// Fields sorted by canonical schema identifier.
    pub fields: Vec<CreationContractField>,
    /// Domain-separated digest of every ordered descriptor and value digest.
    pub digest: String,
}

/// Immutable metadata co-committed with the first event of an entity stream.
///
/// This is the single persistence payload shared by ordinary create and
/// create-or-verify. It deliberately contains no retry identity: ordinary
/// create remains an optimistic sequence-0 append, while create-or-verify adds
/// its separately scoped idempotency identity around this payload.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FirstEventCommit {
    /// Tenant owning the stream and all derived metadata.
    pub tenant: String,
    /// Canonical runtime entity type.
    pub entity_type: String,
    /// Exact durable entity identifier, including a scoped journal identity
    /// when the entity belongs to a pinned bundle.
    pub entity_id: String,
    /// Exact persistence stream receiving the event.
    pub persistence_id: String,
    /// Canonical sequence-1 event.
    pub event: PersistenceEnvelope,
    /// Immutable canonical creation contract compiled before actor creation.
    pub contract: CreationContract,
    /// Versioned contract descriptor revision used by coverage fencing.
    pub contract_revision: u32,
    /// Exact verified global schema or scoped bundle identity.
    pub schema_identity: String,
    /// Deterministic signature of the declared-key schema covered by this write.
    pub declared_key_signature: String,
    /// Exact declared-key ownership set for the newly created entity.
    pub key_rows: Vec<EntityKeyRow>,
    /// Existing derived vector projection rows co-committed where supported.
    pub vector_rows: Vec<EntityVectorRow>,
    /// Whether the vector projection is an exact replacement.
    pub reconcile_vectors: bool,
    /// Initial durable query projection co-committed where the backend owns a
    /// transactional query plane.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub projection: Option<FirstEventProjection>,
}

/// Initial query-plane row accompanying an atomic sequence-1 commit.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FirstEventProjection {
    /// Canonical lifecycle status.
    pub status: String,
    /// Query-visible canonical fields.
    pub fields: serde_json::Value,
    /// Full canonical actor state at sequence 1.
    pub state: serde_json::Value,
    /// Authoritative sequence represented by this projection.
    pub sequence_nr: u64,
}

/// Immutable portion of a [`FirstEventCommit`] embedded in an atomic batch.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FirstEventMetadata {
    /// Immutable canonical creation contract.
    pub contract: CreationContract,
    /// Versioned contract descriptor revision.
    pub contract_revision: u32,
    /// Exact verified global schema or scoped bundle identity.
    pub schema_identity: String,
    /// Deterministic declared-key schema signature.
    pub declared_key_signature: String,
}

/// Atomic store request for create-or-verify.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateOrVerifyRequest {
    /// Stable logical module name owning the request identity.
    pub module_name: String,
    /// Non-empty caller-provided request identity.
    pub idempotency_key: String,
    /// Shared first-event commit payload used if no owner exists.
    pub first_event: FirstEventCommit,
}

impl std::ops::Deref for CreateOrVerifyRequest {
    type Target = FirstEventCommit;

    fn deref(&self) -> &Self::Target {
        &self.first_event
    }
}

impl std::ops::DerefMut for CreateOrVerifyRequest {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.first_event
    }
}

impl FirstEventCommit {
    /// Validate the closed sequence-1 shape before a backend opens a transaction.
    pub fn validate(&self) -> Result<(), PersistenceError> {
        if self.event.sequence_nr != 1 {
            return Err(PersistenceError::Storage(
                "first-event commit must contain sequence 1".to_string(),
            ));
        }
        if self.contract_revision != self.contract.version {
            return Err(PersistenceError::Storage(
                "first-event contract revision does not match its encoding version".to_string(),
            ));
        }
        if self.schema_identity != self.contract.schema_digest {
            return Err(PersistenceError::Storage(
                "first-event schema identity does not match its creation contract".to_string(),
            ));
        }
        let expected = format!("{}:{}:{}", self.tenant, self.entity_type, self.entity_id);
        if self.persistence_id != expected || self.event.metadata.actor_id != expected {
            return Err(PersistenceError::Storage(
                "first-event stream identity is inconsistent".to_string(),
            ));
        }
        let mut ordered = self.key_rows.clone();
        ordered.sort_by(|left, right| {
            (&left.key_name, &left.key_hash).cmp(&(&right.key_name, &right.key_hash))
        });
        ordered.dedup_by(|left, right| {
            left.key_name == right.key_name && left.key_hash == right.key_hash
        });
        if ordered != self.key_rows {
            return Err(PersistenceError::Storage(
                "first-event declared keys must be sorted and unique".to_string(),
            ));
        }
        Ok(())
    }
}

/// Atomic store outcome before authoritative actor state is loaded.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum CreateOrVerifyStoreOutcome {
    /// The request committed the sequence-1 event and metadata.
    Created {
        /// Winning canonical entity identifier.
        entity_id: String,
        /// Committed sequence, always one for a new stream.
        sequence_nr: u64,
    },
    /// An existing owner has the same canonical sequence-1 contract.
    AlreadyMatches {
        /// Winning canonical entity identifier, which may differ from the request.
        entity_id: String,
        /// Immutable creation sequence, always one for the winning stream.
        sequence_nr: u64,
        /// A prior Created disposition committed but its externally visible
        /// notification has not yet been durably acknowledged.
        notification_pending: bool,
    },
    /// Existing ownership or creation fields differ.
    Conflict {
        /// Sorted, bounded canonical schema-owned identifiers.
        fields: Vec<String>,
        /// Whether additional differing identifiers were withheld.
        truncated: bool,
    },
    /// Stored and requested schemas cannot be compared automatically.
    CreationContractMigrationRequired,
}

/// Schema-aware comparison result for two immutable creation contracts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CreationContractComparison {
    /// Every target field has the same canonical creation value.
    Matches,
    /// Comparable fields differ.
    Conflict {
        /// Sorted, bounded canonical schema-owned identifiers.
        fields: Vec<String>,
        /// Whether additional differing identifiers were withheld.
        truncated: bool,
    },
    /// Automatic comparison cannot preserve the original contract meaning.
    MigrationRequired,
}

/// Compare a stored sequence-1 contract with a request compiled under the target schema.
pub fn compare_creation_contracts(
    stored: &CreationContract,
    requested: &CreationContract,
) -> CreationContractComparison {
    compare_creation_contracts_inner(stored, requested, false)
}

/// Compare contracts owned through a declared key, permitting only the entity-ID value to differ.
pub fn compare_creation_contracts_for_alternate_owner(
    stored: &CreationContract,
    requested: &CreationContract,
) -> CreationContractComparison {
    compare_creation_contracts_inner(stored, requested, true)
}

fn compare_creation_contracts_inner(
    stored: &CreationContract,
    requested: &CreationContract,
    alternate_owner: bool,
) -> CreationContractComparison {
    if stored.version != CREATION_CONTRACT_VERSION_V1
        || requested.version != CREATION_CONTRACT_VERSION_V1
        || stored
            .fields
            .iter()
            .chain(requested.fields.iter())
            .any(|field| field.create_required.is_none())
    {
        return CreationContractComparison::MigrationRequired;
    }
    if stored.digest == requested.digest {
        return CreationContractComparison::Matches;
    }

    let stored_by_name = stored
        .fields
        .iter()
        .map(|field| (field.name.as_str(), field))
        .collect::<BTreeMap<_, _>>();
    let mut conflicts = BTreeSet::new();
    for target in &requested.fields {
        let Some(previous) = stored_by_name.get(target.name.as_str()) else {
            if target.create_required == Some(true) {
                return CreationContractComparison::MigrationRequired;
            }
            if target.value_digest != target.default_digest {
                conflicts.insert(target.name.clone());
            }
            continue;
        };
        if previous.type_descriptor != target.type_descriptor
            || previous.value_source != target.value_source
        {
            return CreationContractComparison::MigrationRequired;
        }
        let entity_identity_value = alternate_owner && target.value_source == "entity_id";
        if previous.nullable != target.nullable
            || previous.create_required != target.create_required
            || previous.default_digest != target.default_digest
            || (!entity_identity_value && previous.value_digest != target.value_digest)
        {
            conflicts.insert(target.name.clone());
        }
    }
    if conflicts.is_empty() {
        return CreationContractComparison::Matches;
    }
    let truncated = conflicts.len() > CREATE_OR_VERIFY_CONFLICT_FIELD_BUDGET;
    CreationContractComparison::Conflict {
        fields: conflicts
            .into_iter()
            .take(CREATE_OR_VERIFY_CONFLICT_FIELD_BUDGET)
            .collect(),
        truncated,
    }
}

/// Return a deterministic schema-owned disclosure for a forced contract conflict.
pub fn creation_contract_conflict_fields(
    stored: &CreationContract,
    requested: &CreationContract,
) -> (Vec<String>, bool) {
    let stored_by_name = stored
        .fields
        .iter()
        .map(|field| (field.name.as_str(), field))
        .collect::<BTreeMap<_, _>>();
    let requested_by_name = requested
        .fields
        .iter()
        .map(|field| (field.name.as_str(), field))
        .collect::<BTreeMap<_, _>>();
    let mut fields = stored_by_name
        .keys()
        .chain(requested_by_name.keys())
        .filter(|name| stored_by_name.get(**name) != requested_by_name.get(**name))
        .map(|name| (*name).to_string())
        .collect::<BTreeSet<_>>();
    if fields.is_empty() {
        fields.extend(requested.fields.iter().map(|field| field.name.clone()));
    }
    let truncated = fields.len() > CREATE_OR_VERIFY_CONFLICT_FIELD_BUDGET;
    (
        fields
            .into_iter()
            .take(CREATE_OR_VERIFY_CONFLICT_FIELD_BUDGET)
            .collect(),
        truncated,
    )
}

/// A derived vector-index row to co-commit with an append.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EntityVectorRow {
    pub decl_name: String,
    pub model_tag: String,
    pub vector: Vec<f32>,
}

/// Pack an `f32` slice to little-endian bytes.
pub fn pack_f32_le(vector: &[f32]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(vector.len() * 4);
    for value in vector {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    bytes
}

/// Unpack finite little-endian `f32` values.
pub fn unpack_f32_le(bytes: &[u8]) -> Option<Vec<f32>> {
    if !bytes.len().is_multiple_of(4) {
        return None;
    }
    let mut out = Vec::with_capacity(bytes.len() / 4);
    for chunk in bytes.chunks_exact(4) {
        let value = f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
        if !value.is_finite() {
            return None;
        }
        out.push(value);
    }
    Some(out)
}

/// One candidate row returned from the vector index.
#[derive(Debug, Clone, PartialEq)]
pub struct EntityVectorCandidate {
    pub entity_id: String,
    pub vector: Vec<f32>,
}

/// A persisted event with metadata.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PersistenceEnvelope {
    pub sequence_nr: u64,
    pub event_type: String,
    pub payload: serde_json::Value,
    pub metadata: EventMetadata,
}

/// One stream append inside an atomic multi-journal append.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersistenceAppend {
    pub persistence_id: String,
    pub expected_sequence: u64,
    pub events: Vec<PersistenceEnvelope>,
    /// Declared-key projection rows co-committed with this stream.
    #[serde(default)]
    pub key_rows: Vec<EntityKeyRow>,
    /// Vector projection rows co-committed with this stream.
    #[serde(default)]
    pub vector_rows: Vec<EntityVectorRow>,
    /// Whether prior vector rows for this entity must be replaced.
    #[serde(default)]
    pub reconcile_vectors: bool,
    /// Creation metadata when this append introduces an entity stream.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub first_event: Option<FirstEventMetadata>,
}

/// New sequence number for one stream after an atomic batch append.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PersistenceAppendResult {
    pub persistence_id: String,
    pub sequence_nr: u64,
}

#[path = "types/errors.rs"]
mod errors;
pub use errors::{PersistenceError, storage_error};

#[cfg(test)]
#[path = "types/creation_contract_tests.rs"]
mod creation_contract_tests;
