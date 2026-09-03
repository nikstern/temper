macro_rules! impl_sim_reads_methods {
    () => {
        async fn read_events(
            &self,
            persistence_id: &str,
            from_sequence: u64,
        ) -> Result<Vec<PersistenceEnvelope>, PersistenceError> {
            let mut inner = self.inner.lock().expect("SimEventStore lock poisoned"); // ci-ok: infallible lock

            // Deterministic injected read failure (see `fail_next_reads`).
            if let Some(remaining) = inner.pending_read_failures.get_mut(persistence_id) {
                *remaining -= 1;
                let cleared = *remaining == 0;
                if cleared {
                    inner.pending_read_failures.remove(persistence_id);
                }
                return Err(PersistenceError::Storage(format!(
                    "injected read failure for {persistence_id}"
                )));
            }

            let journal = match inner.journals.get(persistence_id) {
                Some(j) => j,
                None => return Ok(Vec::new()),
            };

            let mut events: Vec<PersistenceEnvelope> = journal
                .iter()
                .filter(|e| e.sequence_nr > from_sequence)
                .cloned()
                .collect();

            // Fault injection: truncate the returned events.
            let rt_prob = inner.faults.read_truncation_prob;
            if !events.is_empty() && inner.rng.chance(rt_prob) {
                let truncate_at = (inner.rng.next_u64() as usize) % events.len();
                events.truncate(truncate_at.max(1));
            }

            Ok(events)
        }

        async fn read_events_limited(
            &self,
            persistence_id: &str,
            from_sequence: u64,
            limit: usize,
        ) -> Result<Vec<PersistenceEnvelope>, PersistenceError> {
            if limit == 0 {
                return Ok(Vec::new());
            }
            let mut inner = self.inner.lock().expect("SimEventStore lock poisoned"); // ci-ok: infallible lock
            if let Some(remaining) = inner.pending_read_failures.get_mut(persistence_id) {
                *remaining -= 1;
                let cleared = *remaining == 0;
                if cleared {
                    inner.pending_read_failures.remove(persistence_id);
                }
                return Err(PersistenceError::Storage(format!(
                    "injected read failure for {persistence_id}"
                )));
            }
            let Some(journal) = inner.journals.get(persistence_id) else {
                return Ok(Vec::new());
            };
            let mut events = journal
                .iter()
                .filter(|event| event.sequence_nr > from_sequence)
                .take(limit)
                .cloned()
                .collect::<Vec<_>>();
            let read_truncation_probability = inner.faults.read_truncation_prob;
            if !events.is_empty() && inner.rng.chance(read_truncation_probability) {
                let truncate_at = (inner.rng.next_u64() as usize) % events.len();
                events.truncate(truncate_at.max(1));
            }
            Ok(events)
        }

        async fn read_latest_events(
            &self,
            persistence_id: &str,
            limit: usize,
        ) -> Result<Vec<PersistenceEnvelope>, PersistenceError> {
            if limit == 0 {
                return Ok(Vec::new());
            }
            let mut inner = self.inner.lock().expect("SimEventStore lock poisoned"); // ci-ok: infallible lock
            if let Some(remaining) = inner.pending_read_failures.get_mut(persistence_id) {
                *remaining -= 1;
                let cleared = *remaining == 0;
                if cleared {
                    inner.pending_read_failures.remove(persistence_id);
                }
                return Err(PersistenceError::Storage(format!(
                    "injected read failure for {persistence_id}"
                )));
            }
            let Some(journal) = inner.journals.get(persistence_id) else {
                return Ok(Vec::new());
            };
            let start = journal.len().saturating_sub(limit);
            let mut events = journal[start..].to_vec();
            let read_truncation_probability = inner.faults.read_truncation_prob;
            if !events.is_empty() && inner.rng.chance(read_truncation_probability) {
                let truncate_at = (inner.rng.next_u64() as usize) % events.len();
                events.truncate(truncate_at.max(1));
            }
            Ok(events)
        }

        async fn save_snapshot(
            &self,
            persistence_id: &str,
            sequence_nr: u64,
            snapshot: &[u8],
        ) -> Result<(), PersistenceError> {
            let mut inner = self.inner.lock().expect("SimEventStore lock poisoned"); // ci-ok: infallible lock

            // Fault injection: snapshot save failure.
            let sf_prob = inner.faults.snapshot_failure_prob;
            if inner.rng.chance(sf_prob) {
                return Err(PersistenceError::Storage(
                    "SimEventStore: injected snapshot failure".into(),
                ));
            }

            inner
                .snapshots
                .insert(persistence_id.to_string(), (sequence_nr, snapshot.to_vec()));
            inner
                .snapshot_history
                .entry(persistence_id.to_string())
                .or_default()
                .insert(sequence_nr, snapshot.to_vec());
            let segments = inner
                .event_segments
                .entry(persistence_id.to_string())
                .or_insert_with(|| {
                    vec![SimEventSegment {
                        segment_index: 0,
                        start_sequence_nr: 1,
                        end_sequence_nr: Some(sequence_nr),
                        snapshot_sequence: None,
                        event_count: sequence_nr,
                        sealed: false,
                    }]
                });
            if segments.last().map(|s| s.sealed).unwrap_or(true) {
                let idx = segments.last().map(|s| s.segment_index + 1).unwrap_or(0);
                segments.push(SimEventSegment {
                    segment_index: idx,
                    start_sequence_nr: 1,
                    end_sequence_nr: Some(sequence_nr),
                    snapshot_sequence: None,
                    event_count: sequence_nr,
                    sealed: false,
                });
            }
            let active = segments
                .last_mut()
                .expect("segments must contain an active segment");
            active.end_sequence_nr = Some(sequence_nr);
            active.snapshot_sequence = Some(sequence_nr);
            active.event_count = sequence_nr
                .saturating_sub(active.start_sequence_nr)
                .saturating_add(1);
            active.sealed = true;
            let next_index = active.segment_index + 1;
            segments.push(SimEventSegment {
                segment_index: next_index,
                start_sequence_nr: sequence_nr + 1,
                end_sequence_nr: None,
                snapshot_sequence: None,
                event_count: 0,
                sealed: false,
            });
            Ok(())
        }

        async fn load_snapshot(
            &self,
            persistence_id: &str,
        ) -> Result<Option<(u64, Vec<u8>)>, PersistenceError> {
            let inner = self.inner.lock().expect("SimEventStore lock poisoned"); // ci-ok: infallible lock
            Ok(inner.snapshots.get(persistence_id).cloned())
        }

        async fn list_entity_ids(
            &self,
            tenant: &str,
        ) -> Result<Vec<(String, String)>, PersistenceError> {
            let inner = self.inner.lock().expect("SimEventStore lock poisoned"); // ci-ok: infallible lock
            let mut result = Vec::new();
            let mut seen = std::collections::BTreeSet::new();

            for persistence_id in inner.journals.keys() {
                if let Ok((t, entity_type, entity_id)) = parse_persistence_id_parts(persistence_id)
                    && t == tenant
                {
                    let key = (entity_type.to_string(), entity_id.to_string());
                    if seen.insert(key.clone()) {
                        result.push(key);
                    }
                }
            }

            Ok(result)
        }

        async fn list_entity_ids_by_type(
            &self,
            tenant: &str,
            entity_type: &str,
        ) -> Result<Vec<String>, PersistenceError> {
            let inner = self.inner.lock().expect("SimEventStore lock poisoned"); // ci-ok: infallible lock
            let mut result = Vec::new();
            let mut seen = std::collections::BTreeSet::new();

            for persistence_id in inner.journals.keys() {
                if let Ok((t, found_type, entity_id)) = parse_persistence_id_parts(persistence_id)
                    && t == tenant
                    && found_type == entity_type
                    && seen.insert(entity_id.to_string())
                {
                    result.push(entity_id.to_string());
                }
            }

            Ok(result)
        }

        async fn list_journal_ids_page(
            &self,
            tenant: &str,
            entity_type: Option<&str>,
            after: Option<(&str, &str)>,
            limit: usize,
        ) -> Result<Vec<(String, String)>, PersistenceError> {
            if limit == 0 {
                return Ok(Vec::new());
            }
            let inner = self.inner.lock().expect("SimEventStore lock poisoned"); // ci-ok: infallible lock
            let result = inner
                .journals
                .keys()
                .filter_map(|persistence_id| {
                    parse_persistence_id_parts(persistence_id)
                        .ok()
                        .filter(|(found_tenant, _, _)| *found_tenant == tenant)
                        .map(|(_, entity_type, entity_id)| {
                            (entity_type.to_string(), entity_id.to_string())
                        })
                })
                .filter(|(found_type, _)| entity_type.is_none_or(|wanted| found_type == wanted))
                .filter(|(entity_type, entity_id)| {
                    after.is_none_or(|cursor| (entity_type.as_str(), entity_id.as_str()) > cursor)
                })
                .take(limit)
                .collect::<Vec<_>>();
            Ok(result)
        }
    };
}
