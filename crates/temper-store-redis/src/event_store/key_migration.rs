//! Resumable migration from legacy untagged keys to tenant hash-tag keys.

use std::collections::BTreeSet;

use fred::prelude::*;
use tokio_stream::StreamExt;

use super::RedisEventStore;
use temper_runtime::persistence::{PersistenceError, storage_error};

fn legacy_prefix(tenant: &str) -> String {
    format!("{}:", crate::keys::PREFIX) + "*" + ":" + tenant
}

fn tagged_key(tenant: &str, legacy: &str) -> Option<String> {
    let prefix = format!("{}:", crate::keys::PREFIX);
    let rest = legacy.strip_prefix(&prefix)?;
    if rest.starts_with('{') {
        return None;
    }
    let (_, after_kind) = rest.split_once(':')?;
    if after_kind != tenant && !after_kind.starts_with(&format!("{tenant}:")) {
        return None;
    }
    Some(format!(
        "{prefix}{}:{rest}",
        RedisEventStore::tenant_hash_tag(tenant)
    ))
}

fn legacy_tenant(key: &str) -> Option<String> {
    let prefix = format!("{}:", crate::keys::PREFIX);
    let rest = key.strip_prefix(&prefix)?;
    if rest.starts_with('{') {
        return None;
    }
    let (_, tenant_and_suffix) = rest.split_once(':')?;
    let tenant = tenant_and_suffix.split(':').next()?;
    (!tenant.is_empty()).then(|| tenant.to_string())
}

impl RedisEventStore {
    /// Discover every tenant that still owns an untagged legacy key.
    pub async fn legacy_key_tenants(&self) -> Result<Vec<String>, PersistenceError> {
        let pattern = format!("{}:*", crate::keys::PREFIX);
        let mut tenants = BTreeSet::new();
        if self.clustered {
            let mut scan = self.client.scan_cluster_buffered(pattern, Some(256), None);
            while let Some(key) = scan.next().await {
                let key = key.map_err(storage_error)?.into_string().ok_or_else(|| {
                    PersistenceError::Serialization("Redis key is not UTF-8".into())
                })?;
                if let Some(tenant) = legacy_tenant(&key) {
                    tenants.insert(tenant);
                }
            }
        } else {
            let mut scan = self.client.scan_buffered(pattern, Some(256), None);
            while let Some(key) = scan.next().await {
                let key = key.map_err(storage_error)?.into_string().ok_or_else(|| {
                    PersistenceError::Serialization("Redis key is not UTF-8".into())
                })?;
                if let Some(tenant) = legacy_tenant(&key) {
                    tenants.insert(tenant);
                }
            }
        }
        Ok(tenants.into_iter().collect())
    }

    /// Migrate all legacy tenants while holding the new-version migration lock.
    ///
    /// `legacy_writers_quiesced` is an explicit operator fence. The migration
    /// refuses to move any key unless the rollout has first stopped every old
    /// binary, since those binaries cannot honor a lock introduced later.
    pub async fn migrate_all_legacy_keys(
        &self,
        legacy_writers_quiesced: bool,
    ) -> Result<usize, PersistenceError> {
        let tenants = self.legacy_key_tenants().await?;
        if tenants.is_empty() {
            return Ok(0);
        }
        if !legacy_writers_quiesced {
            return Err(PersistenceError::Storage(
                "legacy Redis keys exist; stop all pre-hash-tag writers and set \
                 TEMPER_REDIS_LEGACY_WRITERS_QUIESCED=1 before startup"
                    .into(),
            ));
        }
        let lock_key = format!("{}:{{migration}}:legacy-key-migration", crate::keys::PREFIX);
        let token = uuid::Uuid::new_v4().to_string();
        let acquired: Option<String> = self
            .client
            .set(
                &lock_key,
                &token,
                Some(Expiration::PX(3_600_000)),
                Some(SetOptions::NX),
                false,
            )
            .await
            .map_err(storage_error)?;
        if acquired.is_none() {
            return Err(PersistenceError::Storage(
                "another Redis legacy-key migration is active".into(),
            ));
        }
        let mut migrated = 0;
        let mut result = Ok(());
        for tenant in tenants {
            match self.migrate_legacy_tenant_keys(&tenant).await {
                Ok(count) => migrated += count,
                Err(error) => {
                    result = Err(error);
                    break;
                }
            }
        }
        let owner: Option<String> = self.client.get(&lock_key).await.map_err(storage_error)?;
        if owner.as_deref() == Some(token.as_str()) {
            let _: i64 = self.client.del(&lock_key).await.map_err(storage_error)?;
        }
        result.map(|()| migrated)
    }

    /// Move every legacy untagged key for `tenant` into the tenant's Redis
    /// Cluster hash slot.
    ///
    /// The migration is restart-safe: it restores and verifies the tagged copy
    /// before deleting the source. A pre-existing non-identical destination is
    /// treated as corruption and stops the migration. Operators must ensure
    /// legacy binaries are quiescent while this runs.
    pub async fn migrate_legacy_tenant_keys(
        &self,
        tenant: &str,
    ) -> Result<usize, PersistenceError> {
        temper_runtime::tenant::TenantId::try_new(tenant.to_string())
            .map_err(PersistenceError::Storage)?;
        let base = legacy_prefix(tenant);
        let patterns = [base.clone(), format!("{base}:*")];
        let mut sources = BTreeSet::new();
        for pattern in patterns {
            if self.clustered {
                let mut scan = self.client.scan_cluster_buffered(pattern, Some(256), None);
                while let Some(key) = scan.next().await {
                    let key = key.map_err(storage_error)?.into_string().ok_or_else(|| {
                        PersistenceError::Serialization("Redis key is not UTF-8".into())
                    })?;
                    if tagged_key(tenant, &key).is_some() {
                        sources.insert(key);
                    }
                }
            } else {
                let mut scan = self.client.scan_buffered(pattern, Some(256), None);
                while let Some(key) = scan.next().await {
                    let key = key.map_err(storage_error)?.into_string().ok_or_else(|| {
                        PersistenceError::Serialization("Redis key is not UTF-8".into())
                    })?;
                    if tagged_key(tenant, &key).is_some() {
                        sources.insert(key);
                    }
                }
            }
        }

        let mut migrated = 0;
        for source in sources {
            let target = tagged_key(tenant, &source).expect("source was filtered above");
            let Some(serialized) = self
                .client
                .dump::<Option<fred::types::Value>, _>(&source)
                .await
                .map_err(storage_error)?
            else {
                continue;
            };
            let ttl: i64 = self.client.pttl(&source).await.map_err(storage_error)?;
            let existing = self
                .client
                .dump::<Option<fred::types::Value>, _>(&target)
                .await
                .map_err(storage_error)?;
            if let Some(existing) = existing {
                if existing != serialized {
                    return Err(PersistenceError::Storage(format!(
                        "Redis key migration found divergent destination '{target}'"
                    )));
                }
            } else {
                let restore_ttl = ttl.max(0);
                let _: () = self
                    .client
                    .restore(
                        &target,
                        restore_ttl,
                        serialized.clone(),
                        false,
                        false,
                        None,
                        None,
                    )
                    .await
                    .map_err(storage_error)?;
            }
            let verified = self
                .client
                .dump::<Option<fred::types::Value>, _>(&target)
                .await
                .map_err(storage_error)?;
            if verified.as_ref() != Some(&serialized) {
                return Err(PersistenceError::Storage(format!(
                    "Redis key migration could not verify destination '{target}'"
                )));
            }
            let _: i64 = self.client.del(&source).await.map_err(storage_error)?;
            migrated += 1;
        }
        Ok(migrated)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_key_conversion_is_tenant_exact_and_idempotent() {
        assert_eq!(
            tagged_key("acme", "temper:events:acme:Order:o-1").as_deref(),
            Some("temper:{61636d65}:events:acme:Order:o-1")
        );
        assert_eq!(
            legacy_tenant("temper:events:acme:Order:o-1").as_deref(),
            Some("acme")
        );
        assert_eq!(legacy_tenant("temper:{61636d65}:events:acme"), None);
        assert_eq!(tagged_key("acme", "temper:events:acme2:Order:o-1"), None);
        assert_eq!(
            tagged_key("acme", "temper:{61636d65}:events:acme:Order:o-1"),
            None
        );
    }

    #[tokio::test]
    async fn migration_copies_verifies_and_removes_legacy_keys() {
        let Ok(url) = std::env::var("REDIS_URL") else {
            return;
        };
        let store = RedisEventStore::new(&url).await.expect("Redis store");
        let tenant = format!("legacy-{}", uuid::Uuid::new_v4());
        let source = format!("temper:events_seq:{tenant}:Candidate:one");
        let target = tagged_key(&tenant, &source).expect("tagged key");
        let _: () = store
            .client
            .set(&source, "7", None, None, false)
            .await
            .expect("seed legacy key");
        assert_eq!(store.migrate_legacy_tenant_keys(&tenant).await.unwrap(), 1);
        assert_eq!(
            store
                .client
                .get::<Option<String>, _>(&source)
                .await
                .unwrap(),
            None
        );
        assert_eq!(store.client.get::<String, _>(&target).await.unwrap(), "7");
        assert_eq!(store.migrate_legacy_tenant_keys(&tenant).await.unwrap(), 0);
        let _: i64 = store.client.del(&target).await.unwrap();
    }

    #[tokio::test]
    async fn all_tenant_migration_requires_the_operator_quiescence_fence() {
        let Ok(url) = std::env::var("REDIS_URL") else {
            return;
        };
        let store = RedisEventStore::new(&url).await.expect("Redis store");
        let tenant_a = format!("legacy-a-{}", uuid::Uuid::new_v4());
        let tenant_b = format!("legacy-b-{}", uuid::Uuid::new_v4());
        let source_a = format!("temper:events_seq:{tenant_a}:Candidate:one");
        let source_b = format!("temper:events_seq:{tenant_b}:Candidate:two");
        let _: () = store
            .client
            .set(&source_a, "1", None, None, false)
            .await
            .unwrap();
        let _: () = store
            .client
            .set(&source_b, "2", None, None, false)
            .await
            .unwrap();
        assert!(store.migrate_all_legacy_keys(false).await.is_err());
        assert_eq!(store.migrate_all_legacy_keys(true).await.unwrap(), 2);
        assert_eq!(
            store
                .client
                .get::<String, _>(tagged_key(&tenant_a, &source_a).unwrap())
                .await
                .unwrap(),
            "1"
        );
        assert_eq!(
            store
                .client
                .get::<String, _>(tagged_key(&tenant_b, &source_b).unwrap())
                .await
                .unwrap(),
            "2"
        );
    }

    #[tokio::test]
    async fn actual_cluster_lua_and_restart_safe_migration() {
        let Ok(url) = std::env::var("REDIS_CLUSTER_URL") else {
            if std::env::var_os("TEMPER_REQUIRE_BACKEND_PARITY").is_some() {
                panic!("REDIS_CLUSTER_URL is required by the backend-parity lane");
            }
            return;
        };
        let store = RedisEventStore::new(&url)
            .await
            .expect("Redis Cluster store");
        assert!(store.clustered, "cluster URL must exercise cluster routing");
        let namespace = uuid::Uuid::new_v4().simple().to_string();
        temper_runtime::persistence::conformance::run(
            &store,
            &format!("cluster-conformance-{namespace}"),
        )
        .await
        .expect("create and append Lua keys must share the tenant slot");

        let tenant_a = format!("cluster-a-{namespace}");
        let tenant_b = format!("cluster-b-{namespace}");
        let source_a = format!("temper:events_seq:{tenant_a}:Candidate:one");
        let source_b = format!("temper:events_seq:{tenant_b}:Candidate:two");
        let target_a = tagged_key(&tenant_a, &source_a).unwrap();
        let target_b = tagged_key(&tenant_b, &source_b).unwrap();
        let _: () = store
            .client
            .set(&source_a, "7", Some(Expiration::PX(120_000)), None, false)
            .await
            .unwrap();
        let _: () = store
            .client
            .set(&source_b, "8", None, None, false)
            .await
            .unwrap();

        // Simulate interruption after RESTORE and before source deletion.
        let serialized = store
            .client
            .dump::<Option<fred::types::Value>, _>(&source_a)
            .await
            .unwrap()
            .unwrap();
        let _: () = store
            .client
            .restore(&target_a, 120_000, serialized, false, false, None, None)
            .await
            .unwrap();
        assert_eq!(store.migrate_all_legacy_keys(true).await.unwrap(), 2);
        assert_eq!(store.client.get::<String, _>(&target_a).await.unwrap(), "7");
        assert_eq!(store.client.get::<String, _>(&target_b).await.unwrap(), "8");
        let ttl: i64 = store.client.pttl(&target_a).await.unwrap();
        assert!(
            ttl > 0 && ttl <= 120_000,
            "TTL must survive migration: {ttl}"
        );
        assert_eq!(store.migrate_all_legacy_keys(true).await.unwrap(), 0);

        let divergent_tenant = format!("cluster-divergent-{namespace}");
        let divergent_source = format!("temper:events_seq:{divergent_tenant}:Candidate:three");
        let divergent_target = tagged_key(&divergent_tenant, &divergent_source).unwrap();
        let _: () = store
            .client
            .set(&divergent_source, "1", None, None, false)
            .await
            .unwrap();
        let _: () = store
            .client
            .set(&divergent_target, "2", None, None, false)
            .await
            .unwrap();
        assert!(store.migrate_all_legacy_keys(true).await.is_err());

        for key in [target_a, target_b, divergent_source, divergent_target] {
            let _: i64 = store.client.del(key).await.unwrap();
        }
    }
}
