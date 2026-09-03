//! In-memory, deterministic event store for simulation testing.
//!
//! `SimEventStore` implements the [`EventStore`] trait using `BTreeMap` journals.
//! All operations resolve immediately and deterministically. Fault injection
//! is controlled by a seeded RNG for reproducible failures.
//!
//! This crate follows the FoundationDB pattern: swap the I/O, keep the code.
//! Server tests route this implementation through the `StorageStack`
//! event-journal capability so production actor code runs unchanged.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use temper_runtime::persistence::schema_deployment::{
    SchemaExecutionPin, SchemaScope, scoped_journal_pin_suffix, split_scoped_journal_entity_id,
};
use temper_runtime::persistence::{
    CreateOrVerifyRequest, CreateOrVerifyStoreOutcome, CreationContract,
    CreationContractComparison, EntityVectorCandidate, EntityVectorRow, EventStore,
    FirstEventMetadata, PersistenceAppend, PersistenceAppendResult, PersistenceEnvelope,
    PersistenceError, compare_creation_contracts, compare_creation_contracts_for_alternate_owner,
};
use temper_runtime::tenant::parse_persistence_id_parts;

mod create_or_verify;
#[macro_use]
mod event_store_methods_creation;
#[macro_use]
mod event_store_methods_append;
#[macro_use]
mod event_store_methods_indexes;
#[macro_use]
mod event_store_methods_batch;
#[macro_use]
mod event_store_methods_reads;
#[macro_use]
mod event_store_methods_scopes;
mod store_api;
use create_or_verify::commit_first_event_locked;
mod schema_deployment;
pub use schema_deployment::SimSchemaFaultPoint;

/// Fault injection configuration for simulation.
///
/// Controls the probability of injected failures during event store operations.
/// All probabilities are in \[0.0, 1.0\].
#[derive(Debug, Clone)]
pub struct SimFaultConfig {
    /// Probability of a write failure on `append()`.
    pub write_failure_prob: f64,
    /// Probability of a spurious concurrency violation on `append()`.
    pub concurrency_violation_prob: f64,
    /// Probability of truncating journal on `read_events()`.
    pub read_truncation_prob: f64,
    /// Probability of snapshot save failure.
    pub snapshot_failure_prob: f64,
    /// Probability that create-or-verify commits but loses its reply.
    pub create_or_verify_reply_loss_prob: f64,
}

impl SimFaultConfig {
    /// No fault injection — all operations succeed.
    pub fn none() -> Self {
        Self {
            write_failure_prob: 0.0,
            concurrency_violation_prob: 0.0,
            read_truncation_prob: 0.0,
            snapshot_failure_prob: 0.0,
            create_or_verify_reply_loss_prob: 0.0,
        }
    }

    /// Heavy fault injection for stress testing.
    pub fn heavy() -> Self {
        Self {
            write_failure_prob: 0.05,
            concurrency_violation_prob: 0.02,
            read_truncation_prob: 0.01,
            snapshot_failure_prob: 0.03,
            create_or_verify_reply_loss_prob: 0.05,
        }
    }
}

impl Default for SimFaultConfig {
    fn default() -> Self {
        Self::none()
    }
}

/// Deterministic pseudo-random number generator for fault injection.
///
/// Simple xorshift64 — fast, deterministic, good enough for fault injection.
/// Uses `BTreeMap` internally (DST compliance: deterministic iteration order).
#[derive(Debug, Clone)]
pub struct DeterministicRng {
    state: u64,
}

impl DeterministicRng {
    /// Create a new RNG with the given seed.
    pub fn new(seed: u64) -> Self {
        Self {
            state: if seed == 0 { 1 } else { seed },
        }
    }

    /// Generate next u64.
    pub fn next_u64(&mut self) -> u64 {
        let mut x = self.state;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.state = x;
        x
    }

    /// Return true with the given probability \[0.0, 1.0\].
    pub fn chance(&mut self, prob: f64) -> bool {
        if prob <= 0.0 {
            return false;
        }
        if prob >= 1.0 {
            return true;
        }
        let threshold = (prob * u64::MAX as f64) as u64;
        self.next_u64() < threshold
    }
}

/// In-memory, deterministic event store for DST.
///
/// Implements `EventStore` trait. All operations resolve immediately.
/// Fault injection controlled by `DeterministicRng`.
///
/// Uses `BTreeMap` exclusively (no `HashMap`) for deterministic iteration order.
#[derive(Clone)]
pub struct SimEventStore {
    /// Event journals keyed by persistence_id.
    /// Each journal is an ordered list of envelopes.
    inner: Arc<Mutex<SimEventStoreInner>>,
}

#[derive(Debug)]
struct SimEventStoreInner {
    /// Deterministic schema-deployment authority state.
    schema_deployments: schema_deployment::SimSchemaDeploymentState,
    /// Durable tenant-global stream publication fences by entity type.
    unscoped_stream_fences: BTreeMap<(String, String), (String, String, String, String)>,
    /// Event journals: persistence_id → Vec<PersistenceEnvelope>
    journals: BTreeMap<String, Vec<PersistenceEnvelope>>,
    /// Snapshots: persistence_id → (sequence_nr, snapshot_bytes)
    snapshots: BTreeMap<String, (u64, Vec<u8>)>,
    /// Immutable snapshot history: persistence_id → sequence_nr → snapshot bytes.
    snapshot_history: BTreeMap<String, BTreeMap<u64, Vec<u8>>>,
    /// Event segment metadata: persistence_id → Vec<SimEventSegment>.
    event_segments: BTreeMap<String, Vec<SimEventSegment>>,
    /// Fault injection RNG.
    rng: DeterministicRng,
    /// Fault injection configuration.
    faults: SimFaultConfig,
    /// One-shot deterministic failures for schema lifecycle transactions.
    pending_schema_failures: BTreeMap<SimSchemaFaultPoint, u64>,
    /// One-shot concurrency-violation injection counters per `persistence_id`.
    ///
    /// Each entry tells `append` to return a `ConcurrencyViolation` on the next
    /// N calls for that id, then behave normally. Intended for deterministic
    /// retry-path tests where probabilistic injection would be flaky. See
    /// `inject_concurrency_violations`.
    pending_concurrency_violations: BTreeMap<String, u64>,
    /// Each entry makes `read_events` return a storage error on the next N calls for
    /// that id, then behave normally. Deterministic analogue to
    /// `pending_concurrency_violations`, for tests that need a journal-read failure
    /// (e.g. proving the key-index backfill treats an unreadable entity as
    /// `LoadFailed` and does not watermark its type). See `fail_next_reads`.
    pending_read_failures: BTreeMap<String, usize>,
    /// One-shot append delays per `persistence_id`.
    ///
    /// Used by dispatch retry tests to deterministically model "the actor
    /// persisted the transition, but the caller's ask timeout expired before
    /// the reply arrived".
    pending_append_delays: BTreeMap<String, VecDeque<Duration>>,
    /// ADR-0153: declared key-index, co-committed with the journal under the same
    /// lock. `(tenant, entity_type, key_name, key_hash) -> entity_id`. This is the
    /// deterministic reference for the negative-existence access path the real
    /// stores maintain in `entity_key_index`.
    key_index: BTreeMap<(String, String, String, String), String>,
    /// ADR-0153 backfill watermark: `(tenant, entity_type) -> key_set` — each completed
    /// type mapped to the sorted comma-joined declared key names the backfill covered.
    /// The deterministic reference for the real stores' `key_index_backfill_watermark`
    /// table — gates authoritative keyed absence, and detects a key-set change so a
    /// newly-declared key re-keys instead of being treated as already complete.
    key_index_watermark: BTreeMap<(String, String), String>,
    /// ADR-0155: derived vector index, co-committed with the journal under the same
    /// lock. `(tenant, entity_type, decl_name, model_tag, entity_id) -> vector`. The
    /// deterministic reference for the real stores' `entity_vector_index` — the
    /// exact-scan kNN access path. Unlike the key index this has no uniqueness
    /// constraint; it is derived, rebuildable ranking state.
    vector_index: BTreeMap<(String, String, String, String, String), Vec<f32>>,
    /// ADR-0155 backfill watermark: `(tenant, entity_type) -> vector_set` — each
    /// completed type mapped to the sorted comma-joined declared vector-path names the
    /// backfill covered. Mirrors `key_index_watermark`.
    vector_index_watermark: BTreeMap<(String, String), String>,
    creation_contracts: BTreeMap<String, CreationContract>,
    creation_metadata: BTreeMap<String, (FirstEventMetadata, u64)>,
    creation_coverage: BTreeMap<
        (String, String, String, u32, String),
        temper_runtime::persistence::CreationCoveragePublication,
    >,
    create_or_verify_idempotency: BTreeMap<CreateOrVerifyKey, CreateOrVerifyRecord>,
    query_projections: BTreeMap<String, temper_runtime::persistence::FirstEventProjection>,
}

type CreateOrVerifyKey = (String, String, String, String);
type CreateOrVerifyRecord = (String, String, CreationContract, bool);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SimEventSegment {
    pub segment_index: u64,
    pub start_sequence_nr: u64,
    pub end_sequence_nr: Option<u64>,
    pub snapshot_sequence: Option<u64>,
    pub event_count: u64,
    pub sealed: bool,
}

impl std::fmt::Debug for SimEventStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let inner = self.inner.lock().expect("SimEventStore lock poisoned"); // ci-ok: infallible lock
        f.debug_struct("SimEventStore")
            .field("journals", &inner.journals.len())
            .field("snapshots", &inner.snapshots.len())
            .finish()
    }
}

fn creation_source_write_version_locked(
    inner: &SimEventStoreInner,
    tenant: &str,
    entity_type: &str,
) -> Result<u64, PersistenceError> {
    inner
        .journals
        .iter()
        .filter(|(id, events)| {
            !events.is_empty()
                && parse_persistence_id_parts(id)
                    .is_ok_and(|(t, et, _)| t == tenant && et == entity_type)
        })
        .try_fold(0_u64, |sum, (_, events)| {
            sum.checked_add(events.last().map_or(0, |event| event.sequence_nr))
                .ok_or_else(|| PersistenceError::Storage("creation write version overflow".into()))
        })
}

fn advance_creation_coverage_after_append_locked(
    inner: &mut SimEventStoreInner,
    persistence_id: &str,
    tenant: &str,
    entity_type: &str,
    entity_id: &str,
    prior_write_version: u64,
    source_write_version: u64,
) -> Result<(), PersistenceError> {
    let Some((metadata, stored_source)) = inner.creation_metadata.get_mut(persistence_id) else {
        return Ok(());
    };
    *stored_source = source_write_version;
    let metadata = metadata.clone();
    let total = creation_source_write_version_locked(inner, tenant, entity_type)?;
    let matching = inner
        .creation_metadata
        .iter()
        .filter(|(id, (candidate, _))| {
            parse_persistence_id_parts(id).is_ok_and(|(t, et, _)| {
                t == tenant
                    && et == entity_type
                    && candidate.schema_identity == metadata.schema_identity
                    && candidate.contract_revision == metadata.contract_revision
                    && candidate.declared_key_signature == metadata.declared_key_signature
            })
        })
        .collect::<Vec<_>>();
    let stream_count = inner
        .journals
        .iter()
        .filter(|(id, events)| {
            !events.is_empty()
                && parse_persistence_id_parts(id)
                    .is_ok_and(|(t, et, _)| t == tenant && et == entity_type)
        })
        .count();
    let reconciled = matching
        .iter()
        .try_fold(0_u64, |sum, (_, (_, version))| sum.checked_add(*version))
        .ok_or_else(|| PersistenceError::Storage("creation write version overflow".into()))?;
    if matching.len() != stream_count || reconciled != total {
        return Ok(());
    }
    let key = (
        tenant.to_string(),
        entity_type.to_string(),
        metadata.schema_identity.clone(),
        metadata.contract_revision,
        metadata.declared_key_signature.clone(),
    );
    if prior_write_version == 0 {
        inner.creation_coverage.entry(key).or_insert(
            temper_runtime::persistence::CreationCoveragePublication {
                tenant: tenant.to_string(),
                entity_type: entity_type.to_string(),
                metadata,
                cursor: entity_id.to_string(),
                source_write_version: total,
            },
        );
    } else if let Some(coverage) = inner.creation_coverage.get_mut(&key)
        && coverage.source_write_version == prior_write_version
    {
        coverage.cursor = entity_id.to_string();
        coverage.source_write_version = total;
    }
    Ok(())
}

impl EventStore for SimEventStore {
    impl_sim_creation_methods!();

    impl_sim_append_methods!();

    impl_sim_indexes_methods!();

    impl_sim_batch_methods!();

    impl_sim_reads_methods!();

    impl_sim_scopes_methods!();
}

#[cfg(test)]
mod tests;
