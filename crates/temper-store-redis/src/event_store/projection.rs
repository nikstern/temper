//! Redis query-projection reads and writes.

use fred::prelude::*;
use temper_runtime::persistence::{FirstEventProjection, PersistenceError, storage_error};

use super::RedisEventStore;

impl RedisEventStore {
    /// Upsert one durable query projection.
    pub async fn upsert_query_projection(
        &self,
        tenant: &str,
        entity_type: &str,
        entity_id: &str,
        projection: &FirstEventProjection,
    ) -> Result<(), PersistenceError> {
        let key = Self::create_or_verify_hash_key(tenant, entity_type, "projections");
        let encoded = serde_json::to_string(projection)
            .map_err(|error| PersistenceError::Serialization(error.to_string()))?;
        let _: i64 = self
            .client
            .hset(&key, (entity_id, encoded))
            .await
            .map_err(storage_error)?;
        Ok(())
    }

    /// Remove one durable query projection.
    pub async fn remove_query_projection(
        &self,
        tenant: &str,
        entity_type: &str,
        entity_id: &str,
    ) -> Result<(), PersistenceError> {
        let key = Self::create_or_verify_hash_key(tenant, entity_type, "projections");
        let _: i64 = self
            .client
            .hdel(&key, entity_id)
            .await
            .map_err(storage_error)?;
        Ok(())
    }

    /// Load requested durable query projections in caller order.
    pub async fn load_query_projections(
        &self,
        tenant: &str,
        entity_type: &str,
        entity_ids: &[String],
    ) -> Result<Vec<(String, FirstEventProjection)>, PersistenceError> {
        let key = Self::create_or_verify_hash_key(tenant, entity_type, "projections");
        let mut rows = Vec::with_capacity(entity_ids.len());
        for entity_id in entity_ids {
            let encoded: Option<String> = self
                .client
                .hget(&key, entity_id)
                .await
                .map_err(storage_error)?;
            if let Some(encoded) = encoded {
                let projection = serde_json::from_str(&encoded)
                    .map_err(|error| PersistenceError::Serialization(error.to_string()))?;
                rows.push((entity_id.clone(), projection));
            }
        }
        Ok(rows)
    }

    /// Load every durable query projection for one tenant and entity type.
    pub async fn list_query_projections(
        &self,
        tenant: &str,
        entity_type: &str,
    ) -> Result<Vec<(String, FirstEventProjection)>, PersistenceError> {
        let key = Self::create_or_verify_hash_key(tenant, entity_type, "projections");
        let values: std::collections::HashMap<String, String> =
            self.client.hgetall(&key).await.map_err(storage_error)?;
        let mut rows = values
            .into_iter()
            .map(|(entity_id, encoded)| {
                serde_json::from_str(&encoded)
                    .map(|projection| (entity_id, projection))
                    .map_err(|error| PersistenceError::Serialization(error.to_string()))
            })
            .collect::<Result<Vec<_>, _>>()?;
        rows.sort_by(|left, right| left.0.cmp(&right.0));
        Ok(rows)
    }
}
