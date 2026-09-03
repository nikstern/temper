use std::collections::{BTreeMap, BTreeSet};

use temper_runtime::persistence::{PersistenceEnvelope, PersistenceError};

use crate::entity_actor::EntityEvent;
use crate::state::ServerState;
use crate::storage::BoxedEventStore;

use super::EntityStateChange;

const JOURNAL_PAGE_BUDGET: usize = 256;
const JOURNAL_SCAN_BUDGET: usize = 10_000;
const JOURNAL_EVENT_BUDGET: usize = 10_000;

pub(crate) async fn replay_durable_entity_changes(
    state: &ServerState,
    tenant: &str,
    entity_type: &str,
    public_entity_id: &str,
    since: u64,
) -> Result<Vec<EntityStateChange>, PersistenceError> {
    let Some((store, _)) = state.event_journal() else {
        return Ok(Vec::new());
    };
    let direct_id = format!("{tenant}:{entity_type}:{public_entity_id}");
    if entity_journal_public_id(&store, &direct_id)
        .await?
        .is_some()
    {
        return read_entity_changes(
            &store,
            &direct_id,
            tenant,
            entity_type,
            public_entity_id,
            since,
            JOURNAL_EVENT_BUDGET,
        )
        .await;
    }

    if let Some(persistence_id) = find_entity_journal(
        &store,
        tenant,
        entity_type,
        public_entity_id,
        JOURNAL_SCAN_BUDGET,
    )
    .await?
    {
        return read_entity_changes(
            &store,
            &persistence_id,
            tenant,
            entity_type,
            public_entity_id,
            since,
            JOURNAL_EVENT_BUDGET,
        )
        .await;
    }
    Ok(Vec::new())
}

pub(crate) async fn replay_durable_tenant_changes(
    state: &ServerState,
    tenant: &str,
    entity_type: Option<&str>,
    entity_id: Option<&str>,
) -> Result<Vec<EntityStateChange>, PersistenceError> {
    if let (Some(entity_type), Some(entity_id)) = (entity_type, entity_id) {
        return replay_durable_entity_changes(state, tenant, entity_type, entity_id, 0).await;
    }
    let Some((store, _)) = state.event_journal() else {
        return Ok(Vec::new());
    };
    let mut after: Option<(String, String)> = None;
    let mut changes = Vec::new();
    let mut seen = BTreeSet::new();
    let mut scanned_journals = 0usize;
    loop {
        let remaining_journals = JOURNAL_SCAN_BUDGET.saturating_sub(scanned_journals);
        if remaining_journals == 0 {
            let lookahead = store
                .list_journal_ids_page(
                    tenant,
                    entity_type,
                    after
                        .as_ref()
                        .map(|(kind, id)| (kind.as_str(), id.as_str())),
                    1,
                )
                .await?;
            if lookahead.is_empty() {
                break;
            }
            return Err(replay_budget_error("journal scan"));
        }
        let requested_journals = JOURNAL_PAGE_BUDGET.min(remaining_journals);
        let page = store
            .list_journal_ids_page(
                tenant,
                entity_type,
                after
                    .as_ref()
                    .map(|(kind, id)| (kind.as_str(), id.as_str())),
                requested_journals,
            )
            .await?;
        if page.is_empty() {
            break;
        }
        scanned_journals += page.len();
        for (kind, journal_id) in &page {
            let persistence_id = format!("{tenant}:{kind}:{journal_id}");
            let Some(public_id) = entity_journal_public_id(&store, &persistence_id).await? else {
                // Private journals share the event store but are not public entity streams.
                continue;
            };
            if entity_id.is_some_and(|filter| filter != public_id) {
                continue;
            }
            let remaining_events = JOURNAL_EVENT_BUDGET.saturating_sub(changes.len());
            for change in read_entity_changes(
                &store,
                &persistence_id,
                tenant,
                kind,
                &public_id,
                0,
                remaining_events,
            )
            .await?
            {
                if seen.insert((
                    change.entity_type.clone(),
                    change.entity_id.clone(),
                    change.seq,
                )) {
                    changes.push(change);
                }
            }
        }
        after = page.last().cloned();
        if page.len() < requested_journals {
            break;
        }
    }
    changes.sort_by(|left, right| {
        (&left.entity_type, &left.entity_id, left.seq).cmp(&(
            &right.entity_type,
            &right.entity_id,
            right.seq,
        ))
    });
    Ok(changes)
}

pub(crate) fn durable_entity_change_stream(
    state: ServerState,
    mut receiver: tokio::sync::broadcast::Receiver<EntityStateChange>,
    tenant: String,
    entity_type: Option<String>,
    entity_id: Option<String>,
    mut high_water: BTreeMap<(String, String), u64>,
) -> impl tokio_stream::Stream<Item = EntityStateChange> {
    async_stream::stream! {
        loop {
            match receiver.recv().await {
                Ok(change) => {
                    if matches_filter(&change, &tenant, entity_type.as_deref(), entity_id.as_deref())
                        && is_after_high_water(&change, &mut high_water)
                    {
                        yield change;
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                    match replay_durable_tenant_changes(
                        &state,
                        &tenant,
                        entity_type.as_deref(),
                        entity_id.as_deref(),
                    ).await {
                        Ok(recovered) => {
                            for change in recovered {
                                if is_after_high_water(&change, &mut high_water) {
                                    yield change;
                                }
                            }
                        }
                        Err(error) => {
                            tracing::error!(%error, "durable SSE lag recovery failed");
                            break;
                        }
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            }
        }
    }
}

fn matches_filter(
    change: &EntityStateChange,
    tenant: &str,
    entity_type: Option<&str>,
    entity_id: Option<&str>,
) -> bool {
    change.tenant == tenant
        && entity_type.is_none_or(|filter| change.entity_type == filter)
        && entity_id.is_none_or(|filter| change.entity_id == filter)
}

fn is_after_high_water(
    change: &EntityStateChange,
    high_water: &mut BTreeMap<(String, String), u64>,
) -> bool {
    let sequence = high_water
        .entry((change.entity_type.clone(), change.entity_id.clone()))
        .or_default();
    if change.seq <= *sequence {
        return false;
    }
    *sequence = change.seq;
    true
}

async fn entity_journal_public_id(
    store: &BoxedEventStore,
    persistence_id: &str,
) -> Result<Option<String>, PersistenceError> {
    let first = store.read_events_limited(persistence_id, 0, 1).await?;
    Ok(first.first().and_then(|envelope| {
        serde_json::from_value::<EntityEvent>(envelope.payload.clone())
            .ok()
            .and_then(|event| {
                event
                    .params
                    .get("Id")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_string)
            })
    }))
}

async fn find_entity_journal(
    store: &BoxedEventStore,
    tenant: &str,
    entity_type: &str,
    public_entity_id: &str,
    budget: usize,
) -> Result<Option<String>, PersistenceError> {
    let mut after: Option<(String, String)> = None;
    let mut scanned = 0usize;
    loop {
        let remaining = budget.saturating_sub(scanned);
        if remaining == 0 {
            let lookahead = store
                .list_journal_ids_page(
                    tenant,
                    Some(entity_type),
                    after
                        .as_ref()
                        .map(|(kind, id)| (kind.as_str(), id.as_str())),
                    1,
                )
                .await?;
            return if lookahead.is_empty() {
                Ok(None)
            } else {
                Err(replay_budget_error("journal scan"))
            };
        }
        let requested = JOURNAL_PAGE_BUDGET.min(remaining);
        let page = store
            .list_journal_ids_page(
                tenant,
                Some(entity_type),
                after
                    .as_ref()
                    .map(|(kind, id)| (kind.as_str(), id.as_str())),
                requested,
            )
            .await?;
        if page.is_empty() {
            return Ok(None);
        }
        scanned += page.len();
        for (kind, journal_id) in &page {
            let persistence_id = format!("{tenant}:{kind}:{journal_id}");
            if entity_journal_public_id(store, &persistence_id)
                .await?
                .as_deref()
                == Some(public_entity_id)
            {
                return Ok(Some(persistence_id));
            }
        }
        after = page.last().cloned();
        if page.len() < requested {
            return Ok(None);
        }
    }
}

async fn read_entity_changes(
    store: &BoxedEventStore,
    persistence_id: &str,
    tenant: &str,
    entity_type: &str,
    public_entity_id: &str,
    since: u64,
    budget: usize,
) -> Result<Vec<EntityStateChange>, PersistenceError> {
    let mut cursor = since;
    let mut changes = Vec::new();
    loop {
        let remaining = budget.saturating_sub(changes.len());
        let requested = JOURNAL_PAGE_BUDGET.min(remaining.saturating_add(1));
        let envelopes = store
            .read_events_limited(persistence_id, cursor, requested)
            .await?;
        if envelopes.len() > remaining {
            return Err(replay_budget_error("event"));
        }
        let read = envelopes.len();
        if let Some(last) = envelopes.last() {
            cursor = last.sequence_nr;
        }
        changes.extend(changes_from_envelopes(
            tenant,
            entity_type,
            public_entity_id,
            envelopes,
        )?);
        if read < requested {
            break;
        }
    }
    Ok(changes)
}

fn changes_from_envelopes(
    tenant: &str,
    entity_type: &str,
    public_entity_id: &str,
    envelopes: Vec<PersistenceEnvelope>,
) -> Result<Vec<EntityStateChange>, PersistenceError> {
    envelopes
        .into_iter()
        .map(|envelope| {
            let event: EntityEvent = serde_json::from_value(envelope.payload)
                .map_err(|error| PersistenceError::Serialization(error.to_string()))?;
            Ok(EntityStateChange {
                seq: envelope.sequence_nr,
                entity_type: entity_type.to_string(),
                entity_id: public_entity_id.to_string(),
                action: event.action,
                status: event.to_status,
                tenant: tenant.to_string(),
                agent_id: None,
                session_id: None,
                intent: None,
                observation_metadata: None,
            })
        })
        .collect()
}

fn replay_budget_error(kind: &str) -> PersistenceError {
    PersistenceError::Storage(format!("durable SSE replay {kind} budget exhausted"))
}
