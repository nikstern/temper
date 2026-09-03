//! Tenant-slot Redis key construction.

use temper_runtime::persistence::PersistenceError;

use super::RedisEventStore;
use crate::keys::{decode_lex_component, encode_lex_component};

impl RedisEventStore {
    pub(super) fn tenant_hash_tag(tenant: &str) -> String {
        format!("{{{}}}", encode_lex_component(tenant))
    }

    pub(super) fn events_key(tenant: &str, entity_type: &str, entity_id: &str) -> String {
        Self::tagged(
            tenant,
            &format!("events:{tenant}:{entity_type}:{entity_id}"),
        )
    }

    pub(super) fn seq_key(tenant: &str, entity_type: &str, entity_id: &str) -> String {
        Self::tagged(
            tenant,
            &format!("events_seq:{tenant}:{entity_type}:{entity_id}"),
        )
    }

    pub(super) fn create_or_verify_hash_key(tenant: &str, entity_type: &str, kind: &str) -> String {
        Self::tagged(
            tenant,
            &format!("create_or_verify:{tenant}:{entity_type}:{kind}"),
        )
    }

    pub(super) fn creation_coverage_key(
        tenant: &str,
        entity_type: &str,
        schema_identity: &str,
        contract_revision: u32,
        declared_key_signature: &str,
    ) -> String {
        Self::tagged(
            tenant,
            &format!(
                "create_or_verify:{tenant}:{entity_type}:coverage:{schema_identity}:{contract_revision}:{declared_key_signature}"
            ),
        )
    }

    pub(super) fn snapshot_key(tenant: &str, entity_type: &str, entity_id: &str) -> String {
        Self::tagged(
            tenant,
            &format!("snapshot:{tenant}:{entity_type}:{entity_id}"),
        )
    }

    pub(super) fn snapshot_history_key(
        tenant: &str,
        entity_type: &str,
        entity_id: &str,
        sequence_nr: u64,
    ) -> String {
        Self::tagged(
            tenant,
            &format!("snapshot_history:{tenant}:{entity_type}:{entity_id}:{sequence_nr}"),
        )
    }

    pub(super) fn current_segment_key(tenant: &str, entity_type: &str, entity_id: &str) -> String {
        Self::tagged(
            tenant,
            &format!("event_segment_current:{tenant}:{entity_type}:{entity_id}"),
        )
    }

    pub(super) fn segment_key(
        tenant: &str,
        entity_type: &str,
        entity_id: &str,
        segment_index: u64,
    ) -> String {
        Self::tagged(
            tenant,
            &format!("event_segment:{tenant}:{entity_type}:{entity_id}:{segment_index}"),
        )
    }

    pub(super) fn tenant_entities_key(tenant: &str) -> String {
        Self::tagged(tenant, &format!("entities:{tenant}"))
    }

    pub(super) fn tenant_journals_key(tenant: &str) -> String {
        Self::tagged(tenant, &format!("journals:{tenant}"))
    }

    pub(super) fn unscoped_journals_key(tenant: &str, entity_type: &str) -> String {
        Self::tagged(
            tenant,
            &format!(
                "unscoped_journals:{tenant}:{}",
                encode_lex_component(entity_type)
            ),
        )
    }

    pub(super) fn unscoped_index_cursor_key(tenant: &str, entity_type: &str) -> String {
        Self::tagged(
            tenant,
            &format!(
                "unscoped_journals_cursor:{tenant}:{}",
                encode_lex_component(entity_type)
            ),
        )
    }

    pub(super) fn unscoped_index_complete_key(tenant: &str, entity_type: &str) -> String {
        Self::tagged(
            tenant,
            &format!(
                "unscoped_journals_complete:{tenant}:{}",
                encode_lex_component(entity_type)
            ),
        )
    }

    pub(super) fn unscoped_generation_key(tenant: &str, entity_type: &str) -> String {
        Self::tagged(
            tenant,
            &format!(
                "unscoped_generation:{tenant}:{}",
                encode_lex_component(entity_type)
            ),
        )
    }

    pub(super) fn unscoped_fence_key(tenant: &str, entity_type: &str) -> String {
        Self::tagged(
            tenant,
            &format!(
                "unscoped_fence:{tenant}:{}",
                encode_lex_component(entity_type)
            ),
        )
    }

    pub(super) fn unscoped_application_fence_key(tenant: &str, application_id: &str) -> String {
        Self::tagged(
            tenant,
            &format!(
                "unscoped_application_fence:{tenant}:{}",
                encode_lex_component(application_id)
            ),
        )
    }

    pub(super) fn journal_member(entity_type: &str, entity_id: &str) -> String {
        format!(
            "{}!{}",
            encode_lex_component(entity_type),
            encode_lex_component(entity_id)
        )
    }

    pub(super) fn parse_journal_member(member: &str) -> Result<(String, String), PersistenceError> {
        let (entity_type, entity_id) = member.split_once('!').ok_or_else(|| {
            PersistenceError::Serialization("invalid Redis journal index member".to_string())
        })?;
        Ok((
            decode_lex_component(entity_type)?,
            decode_lex_component(entity_id)?,
        ))
    }

    pub(super) fn trajectory_key(tenant: &str) -> String {
        Self::tagged(tenant, &format!("trajectories:{tenant}"))
    }

    fn tagged(tenant: &str, suffix: &str) -> String {
        format!(
            "{}:{}:{suffix}",
            crate::keys::PREFIX,
            Self::tenant_hash_tag(tenant)
        )
    }
}
