macro_rules! redis_event_store_core_methods {
    () => {
        async fn append(
            &self,
            persistence_id: &str,
            expected_sequence: u64,
            events: &[PersistenceEnvelope],
        ) -> Result<u64, PersistenceError> {
            let (tenant, entity_type, entity_id) =
                parse_persistence_id_parts(persistence_id).map_err(PersistenceError::Storage)?;
            let seq_key = Self::seq_key(tenant, entity_type, entity_id);
            let events_key = Self::events_key(tenant, entity_type, entity_id);
            let entities_key = Self::tenant_entities_key(tenant);
            let journals_key = Self::tenant_journals_key(tenant);

            // Pre-serialize events with provisional sequence numbers.
            let mut args: Vec<String> = Vec::with_capacity(events.len() + 4);
            args.push(expected_sequence.to_string());

            let entity_ref = EntityRef {
                entity_type: entity_type.to_string(),
                entity_id: entity_id.to_string(),
            };
            let entity_ref_json = serde_json::to_string(&entity_ref)
                .map_err(|e| PersistenceError::Serialization(e.to_string()))?;
            args.push(entity_ref_json);
            args.push(Self::journal_member(entity_type, entity_id));
            let unscoped = split_scoped_journal_entity_id(entity_id).is_none();
            args.push(
                unscoped
                    .then(|| encode_lex_component(entity_id))
                    .unwrap_or_default(),
            );

            let mut seq = expected_sequence;
            for event in events {
                seq += 1;
                let mut env = event.clone();
                env.sequence_nr = seq;
                let encoded = serde_json::to_string(&env)
                    .map_err(|e| PersistenceError::Serialization(e.to_string()))?;
                args.push(encoded);
            }

            let keys = vec![
                seq_key,
                events_key,
                entities_key,
                journals_key,
                Self::unscoped_journals_key(tenant, entity_type),
                Self::unscoped_generation_key(tenant, entity_type),
                Self::unscoped_fence_key(tenant, entity_type),
            ];
            let result: Vec<i64> = self
                .append_script
                .evalsha_with_reload(&self.client, keys, args)
                .await
                .map_err(storage_error)?;

            match result.as_slice() {
                [1, new_seq] => {
                    let new_seq = *new_seq as u64;
                    self.update_segment_after_append(
                        tenant,
                        entity_type,
                        entity_id,
                        expected_sequence,
                        new_seq,
                    )
                    .await?;
                    Ok(new_seq)
                }
                [0, actual] => Err(PersistenceError::ConcurrencyViolation {
                    expected: expected_sequence,
                    actual: *actual as u64,
                }),
                [-1, _] => Err(PersistenceError::Storage(
                    "stream descriptor publication fence".into(),
                )),
                other => Err(PersistenceError::Storage(format!(
                    "unexpected Lua script result: {other:?}"
                ))),
            }
        }

        async fn append_batch(
            &self,
            appends: &[PersistenceAppend],
        ) -> Result<Vec<PersistenceAppendResult>, PersistenceError> {
            self.append_batch_inner(appends).await
        }

        async fn read_events(
            &self,
            persistence_id: &str,
            from_sequence: u64,
        ) -> Result<Vec<PersistenceEnvelope>, PersistenceError> {
            let (tenant, entity_type, entity_id) =
                parse_persistence_id_parts(persistence_id).map_err(PersistenceError::Storage)?;
            let events_key = Self::events_key(tenant, entity_type, entity_id);

            // Events are stored via RPUSH with sequential indices starting at 0.
            // Event at index i has sequence_nr = i + 1.
            // To read events with sequence_nr > from_sequence, start at index from_sequence.
            let start_index = from_sequence as i64;
            let encoded_events: Vec<String> = self
                .client
                .lrange(&events_key, start_index, -1)
                .await
                .map_err(storage_error)?;

            let mut out = Vec::with_capacity(encoded_events.len());
            for encoded in encoded_events {
                let env: PersistenceEnvelope = serde_json::from_str(&encoded)
                    .map_err(|e| PersistenceError::Serialization(e.to_string()))?;
                out.push(env);
            }
            out.sort_by_key(|e| e.sequence_nr);
            Ok(out)
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
            let (tenant, entity_type, entity_id) =
                parse_persistence_id_parts(persistence_id).map_err(PersistenceError::Storage)?;
            let events_key = Self::events_key(tenant, entity_type, entity_id);
            let end = from_sequence
                .saturating_add(limit as u64)
                .saturating_sub(1)
                .min(i64::MAX as u64) as i64;
            let encoded_events: Vec<String> = self
                .client
                .lrange(&events_key, from_sequence.min(i64::MAX as u64) as i64, end)
                .await
                .map_err(storage_error)?;
            encoded_events
                .into_iter()
                .map(|encoded| {
                    serde_json::from_str(&encoded)
                        .map_err(|error| PersistenceError::Serialization(error.to_string()))
                })
                .collect()
        }

        async fn read_latest_events(
            &self,
            persistence_id: &str,
            limit: usize,
        ) -> Result<Vec<PersistenceEnvelope>, PersistenceError> {
            if limit == 0 {
                return Ok(Vec::new());
            }
            let (tenant, entity_type, entity_id) =
                parse_persistence_id_parts(persistence_id).map_err(PersistenceError::Storage)?;
            let events_key = Self::events_key(tenant, entity_type, entity_id);
            let start = -(limit.min(i64::MAX as usize) as i64);
            let encoded_events: Vec<String> = self
                .client
                .lrange(&events_key, start, -1)
                .await
                .map_err(storage_error)?;
            encoded_events
                .into_iter()
                .map(|encoded| {
                    serde_json::from_str(&encoded)
                        .map_err(|error| PersistenceError::Serialization(error.to_string()))
                })
                .collect()
        }

        async fn save_snapshot(
            &self,
            persistence_id: &str,
            sequence_nr: u64,
            snapshot: &[u8],
        ) -> Result<(), PersistenceError> {
            let (tenant, entity_type, entity_id) =
                parse_persistence_id_parts(persistence_id).map_err(PersistenceError::Storage)?;
            let key = Self::snapshot_key(tenant, entity_type, entity_id);
            let record = SnapshotRecord {
                sequence_nr,
                snapshot: snapshot.to_vec(),
            };
            let encoded = serde_json::to_string(&record)
                .map_err(|e| PersistenceError::Serialization(e.to_string()))?;
            let _: () = self
                .client
                .set(&key, encoded, None, None, false)
                .await
                .map_err(storage_error)?;

            let history_key =
                Self::snapshot_history_key(tenant, entity_type, entity_id, sequence_nr);
            let history = SnapshotHistoryRecord {
                sequence_nr,
                snapshot: snapshot.to_vec(),
                created_at: chrono::Utc::now(),
            };
            let encoded_history = serde_json::to_string(&history)
                .map_err(|e| PersistenceError::Serialization(e.to_string()))?;
            let _: () = self
                .client
                .set(&history_key, encoded_history, None, None, false)
                .await
                .map_err(storage_error)?;

            let current_segment_key = Self::current_segment_key(tenant, entity_type, entity_id);
            let current_segment_raw: Option<String> = self
                .client
                .get(&current_segment_key)
                .await
                .map_err(storage_error)?;
            let current_segment = current_segment_raw
                .and_then(|raw| raw.parse::<u64>().ok())
                .unwrap_or(0);
            let segment_key = Self::segment_key(tenant, entity_type, entity_id, current_segment);
            let existing: Option<String> =
                self.client.get(&segment_key).await.map_err(storage_error)?;
            let mut segment = existing
                .as_deref()
                .map(serde_json::from_str::<SegmentRecord>)
                .transpose()
                .map_err(|e| PersistenceError::Serialization(e.to_string()))?
                .unwrap_or_else(|| SegmentRecord {
                    segment_index: current_segment,
                    start_sequence_nr: 1,
                    end_sequence_nr: Some(sequence_nr),
                    snapshot_sequence: Some(sequence_nr),
                    event_count: sequence_nr,
                    sealed_at: None,
                    created_at: chrono::Utc::now(),
                });
            segment.end_sequence_nr = Some(sequence_nr);
            segment.snapshot_sequence = Some(sequence_nr);
            segment.event_count = sequence_nr.saturating_sub(segment.start_sequence_nr) + 1;
            segment.sealed_at = Some(chrono::Utc::now());
            let encoded_segment = serde_json::to_string(&segment)
                .map_err(|e| PersistenceError::Serialization(e.to_string()))?;
            let _: () = self
                .client
                .set(&segment_key, encoded_segment, None, None, false)
                .await
                .map_err(storage_error)?;

            let next_segment = current_segment + 1;
            let next_segment_key = Self::segment_key(tenant, entity_type, entity_id, next_segment);
            let next = SegmentRecord {
                segment_index: next_segment,
                start_sequence_nr: sequence_nr + 1,
                end_sequence_nr: None,
                snapshot_sequence: None,
                event_count: 0,
                sealed_at: None,
                created_at: chrono::Utc::now(),
            };
            let encoded_next = serde_json::to_string(&next)
                .map_err(|e| PersistenceError::Serialization(e.to_string()))?;
            let _: () = self
                .client
                .set(&next_segment_key, encoded_next, None, None, false)
                .await
                .map_err(storage_error)?;
            let _: () = self
                .client
                .set(
                    &current_segment_key,
                    next_segment.to_string(),
                    None,
                    None,
                    false,
                )
                .await
                .map_err(storage_error)?;
            Ok(())
        }

        async fn load_snapshot(
            &self,
            persistence_id: &str,
        ) -> Result<Option<(u64, Vec<u8>)>, PersistenceError> {
            let (tenant, entity_type, entity_id) =
                parse_persistence_id_parts(persistence_id).map_err(PersistenceError::Storage)?;
            let key = Self::snapshot_key(tenant, entity_type, entity_id);
            let encoded: Option<String> = self.client.get(&key).await.map_err(storage_error)?;
            let Some(encoded) = encoded else {
                return Ok(None);
            };
            let record: SnapshotRecord = serde_json::from_str(&encoded)
                .map_err(|e| PersistenceError::Serialization(e.to_string()))?;
            Ok(Some((record.sequence_nr, record.snapshot)))
        }

        async fn list_entity_ids(
            &self,
            tenant: &str,
        ) -> Result<Vec<(String, String)>, PersistenceError> {
            let key = Self::tenant_entities_key(tenant);
            let members: Vec<String> = self.client.smembers(&key).await.map_err(storage_error)?;

            let mut out = Vec::with_capacity(members.len());
            for encoded in members {
                let entity_ref: EntityRef = serde_json::from_str(&encoded)
                    .map_err(|e| PersistenceError::Serialization(e.to_string()))?;
                out.push((entity_ref.entity_type, entity_ref.entity_id));
            }

            out.sort();
            out.dedup();
            Ok(out)
        }

        async fn list_entity_ids_by_type(
            &self,
            tenant: &str,
            entity_type: &str,
        ) -> Result<Vec<String>, PersistenceError> {
            let key = Self::tenant_entities_key(tenant);
            let members: Vec<String> = self.client.smembers(&key).await.map_err(storage_error)?;

            let mut out = Vec::new();
            for encoded in members {
                let entity_ref: EntityRef = serde_json::from_str(&encoded)
                    .map_err(|e| PersistenceError::Serialization(e.to_string()))?;
                if entity_ref.entity_type == entity_type {
                    out.push(entity_ref.entity_id);
                }
            }

            out.sort();
            out.dedup();
            Ok(out)
        }

        async fn list_creation_source_ids_by_type(
            &self,
            tenant: &str,
            entity_type: &str,
        ) -> Result<Vec<String>, PersistenceError> {
            let mut out = self.list_entity_ids_by_type(tenant, entity_type).await?;
            let owners_key = Self::create_or_verify_hash_key(tenant, entity_type, "owners");
            let entity_keys_key =
                Self::create_or_verify_hash_key(tenant, entity_type, "entity_keys");
            let owners: Vec<String> = self
                .client
                .hvals(&owners_key)
                .await
                .map_err(storage_error)?;
            let indexed_entities: Vec<String> = self
                .client
                .hkeys(&entity_keys_key)
                .await
                .map_err(storage_error)?;
            out.extend(owners);
            out.extend(indexed_entities);
            out.sort();
            out.dedup();
            Ok(out)
        }
    };
}
