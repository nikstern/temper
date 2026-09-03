use super::*;

impl SimEventStore {
    /// Create a new SimEventStore with the given seed and fault config.
    pub fn new(seed: u64, faults: SimFaultConfig) -> Self {
        Self {
            inner: Arc::new(Mutex::new(SimEventStoreInner {
                schema_deployments: schema_deployment::SimSchemaDeploymentState::default(),
                unscoped_stream_fences: BTreeMap::new(),
                journals: BTreeMap::new(),
                snapshots: BTreeMap::new(),
                snapshot_history: BTreeMap::new(),
                event_segments: BTreeMap::new(),
                rng: DeterministicRng::new(seed),
                faults,
                pending_schema_failures: BTreeMap::new(),
                pending_concurrency_violations: BTreeMap::new(),
                pending_read_failures: BTreeMap::new(),
                pending_append_delays: BTreeMap::new(),
                key_index: BTreeMap::new(),
                key_index_watermark: BTreeMap::new(),
                vector_index: BTreeMap::new(),
                vector_index_watermark: BTreeMap::new(),
                creation_contracts: BTreeMap::new(),
                creation_metadata: BTreeMap::new(),
                creation_coverage: BTreeMap::new(),
                create_or_verify_idempotency: BTreeMap::new(),
                query_projections: BTreeMap::new(),
            })),
        }
    }

    /// Inject exactly `count` deterministic `ConcurrencyViolation` errors on
    /// the next `count` `append` calls for `persistence_id`, then behave
    /// normally.
    ///
    /// Use this for retry-path tests where the probabilistic fault injection
    /// in `SimFaultConfig` would be flaky. Each injected violation reports
    /// `actual = expected_sequence` (the journal has not actually moved), so
    /// any callers with post-replay sequence assertions still hold after the
    /// retry replays back to the same spot.
    pub fn inject_concurrency_violations(&self, persistence_id: &str, count: u64) {
        let mut inner = self.inner.lock().expect("SimEventStore lock poisoned"); // ci-ok: infallible lock

        if count == 0 {
            inner.pending_concurrency_violations.remove(persistence_id);
        } else {
            inner
                .pending_concurrency_violations
                .insert(persistence_id.to_string(), count);
        }
    }

    /// Make the next `count` `read_events` calls for `persistence_id` fail with a
    /// storage error, then behave normally. Deterministic (unlike
    /// `read_truncation_prob`) so tests can prove read-failure handling — e.g. that
    /// the key-index backfill classifies an unreadable entity as `LoadFailed` and
    /// therefore does not watermark its type. `count == 0` clears the injection.
    pub fn fail_next_reads(&self, persistence_id: &str, count: usize) {
        let mut inner = self.inner.lock().expect("SimEventStore lock poisoned"); // ci-ok: infallible lock
        if count == 0 {
            inner.pending_read_failures.remove(persistence_id);
        } else {
            inner
                .pending_read_failures
                .insert(persistence_id.to_string(), count);
        }
    }

    /// Return the current count of pending injected concurrency violations for
    /// `persistence_id`. Zero if none are queued.
    pub fn pending_concurrency_violations(&self, persistence_id: &str) -> u64 {
        let inner = self.inner.lock().expect("SimEventStore lock poisoned"); // ci-ok: infallible lock
        inner
            .pending_concurrency_violations
            .get(persistence_id)
            .copied()
            .unwrap_or(0)
    }

    /// Delay the next append for `persistence_id` by `delay`.
    ///
    /// The delay is consumed once. Multiple calls queue multiple delays in
    /// FIFO order.
    pub fn inject_append_delay(&self, persistence_id: &str, delay: Duration) {
        let mut inner = self
            .inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        inner
            .pending_append_delays
            .entry(persistence_id.to_string())
            .or_default()
            .push_back(delay);
    }

    /// Create a SimEventStore with no fault injection.
    pub fn no_faults(seed: u64) -> Self {
        Self::new(seed, SimFaultConfig::none())
    }

    /// Return the total number of events across all journals.
    pub fn total_events(&self) -> usize {
        let inner = self.inner.lock().expect("SimEventStore lock poisoned"); // ci-ok: infallible lock
        inner.journals.values().map(|j| j.len()).sum()
    }

    /// Return the number of distinct persistence IDs with events.
    pub fn entity_count(&self) -> usize {
        let inner = self.inner.lock().expect("SimEventStore lock poisoned"); // ci-ok: infallible lock
        inner.journals.len()
    }

    /// List all persistence IDs that have at least one event.
    ///
    /// Used by DST invariant checkers to iterate all entities in the store.
    pub fn list_all_persistence_ids(&self) -> Vec<String> {
        let inner = self.inner.lock().expect("SimEventStore lock poisoned"); // ci-ok: infallible lock
        inner.journals.keys().cloned().collect()
    }

    /// Temporarily disable all fault injection.
    ///
    /// Returns the previous config so it can be restored. Useful for
    /// restart phases where reads must succeed reliably.
    pub fn disable_faults(&self) -> SimFaultConfig {
        let mut inner = self.inner.lock().expect("SimEventStore lock poisoned"); // ci-ok: infallible lock
        let prev = inner.faults.clone();
        inner.faults = SimFaultConfig::none();
        prev
    }

    /// Restore a previously saved fault config.
    pub fn restore_faults(&self, faults: SimFaultConfig) {
        let mut inner = self.inner.lock().expect("SimEventStore lock poisoned"); // ci-ok: infallible lock
        inner.faults = faults;
    }

    /// Dump all events for a persistence_id (for test assertions).
    pub fn dump_journal(&self, persistence_id: &str) -> Vec<PersistenceEnvelope> {
        let inner = self.inner.lock().expect("SimEventStore lock poisoned"); // ci-ok: infallible lock
        inner
            .journals
            .get(persistence_id)
            .cloned()
            .unwrap_or_default()
    }

    /// Return the projection atomically stored with a stream's first event.
    pub fn dump_first_event_projection(
        &self,
        persistence_id: &str,
    ) -> Option<temper_runtime::persistence::FirstEventProjection> {
        self.inner
            .lock()
            .expect("SimEventStore lock poisoned")
            .query_projections
            .get(persistence_id)
            .cloned()
    }

    /// Upsert a durable query projection.
    pub fn upsert_query_projection(
        &self,
        tenant: &str,
        entity_type: &str,
        entity_id: &str,
        projection: temper_runtime::persistence::FirstEventProjection,
    ) {
        self.inner
            .lock()
            .expect("SimEventStore lock poisoned")
            .query_projections
            .insert(format!("{tenant}:{entity_type}:{entity_id}"), projection);
    }

    /// Remove a durable query projection.
    pub fn remove_query_projection(&self, tenant: &str, entity_type: &str, entity_id: &str) {
        self.inner
            .lock()
            .expect("SimEventStore lock poisoned")
            .query_projections
            .remove(&format!("{tenant}:{entity_type}:{entity_id}"));
    }

    /// Load requested durable query projections in caller order.
    pub fn load_query_projections(
        &self,
        tenant: &str,
        entity_type: &str,
        entity_ids: &[String],
    ) -> Vec<(String, temper_runtime::persistence::FirstEventProjection)> {
        let inner = self.inner.lock().expect("SimEventStore lock poisoned");
        entity_ids
            .iter()
            .filter_map(|entity_id| {
                inner
                    .query_projections
                    .get(&format!("{tenant}:{entity_type}:{entity_id}"))
                    .cloned()
                    .map(|projection| (entity_id.clone(), projection))
            })
            .collect()
    }

    /// Load every durable query projection for one type.
    pub fn list_query_projections(
        &self,
        tenant: &str,
        entity_type: &str,
    ) -> Vec<(String, temper_runtime::persistence::FirstEventProjection)> {
        let prefix = format!("{tenant}:{entity_type}:");
        self.inner
            .lock()
            .expect("SimEventStore lock poisoned")
            .query_projections
            .iter()
            .filter_map(|(id, projection)| {
                id.strip_prefix(&prefix)
                    .map(|entity_id| (entity_id.to_string(), projection.clone()))
            })
            .collect()
    }

    /// Count durable query projections by tenant.
    pub fn query_projection_counts_by_tenant(&self) -> Vec<(String, u64)> {
        let mut counts = BTreeMap::<String, u64>::new();
        for id in self
            .inner
            .lock()
            .expect("SimEventStore lock poisoned")
            .query_projections
            .keys()
        {
            if let Some((tenant, _)) = id.split_once(':') {
                *counts.entry(tenant.to_string()).or_default() += 1;
            }
        }
        counts.into_iter().collect()
    }

    pub fn snapshot_history_len(&self, persistence_id: &str) -> usize {
        let inner = self.inner.lock().expect("SimEventStore lock poisoned"); // ci-ok: infallible lock
        inner
            .snapshot_history
            .get(persistence_id)
            .map(BTreeMap::len)
            .unwrap_or(0)
    }

    pub fn dump_segments(&self, persistence_id: &str) -> Vec<SimEventSegment> {
        let inner = self.inner.lock().expect("SimEventStore lock poisoned"); // ci-ok: infallible lock
        inner
            .event_segments
            .get(persistence_id)
            .cloned()
            .unwrap_or_default()
    }
}
