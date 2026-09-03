use temper_runtime::persistence::{FirstEventProjection, PersistenceError};
use temper_store_redis::RedisEventStore;

use super::super::{
    EntityCatalogRow, QueryFieldIndexOrder, QueryFieldIndexPage, QueryPlaneStore,
    QueryProjectionFieldsRow,
};

#[async_trait::async_trait]
impl QueryPlaneStore for RedisEventStore {
    async fn upsert_projection(
        &self,
        tenant: &str,
        entity_type: &str,
        entity_id: &str,
        status: &str,
        fields: &serde_json::Value,
        state: &serde_json::Value,
        sequence_nr: u64,
    ) -> Result<(), PersistenceError> {
        self.upsert_query_projection(
            tenant,
            entity_type,
            entity_id,
            &FirstEventProjection {
                status: status.to_string(),
                fields: fields.clone(),
                state: state.clone(),
                sequence_nr,
            },
        )
        .await
    }

    async fn remove_projection(
        &self,
        tenant: &str,
        entity_type: &str,
        entity_id: &str,
    ) -> Result<(), PersistenceError> {
        self.remove_query_projection(tenant, entity_type, entity_id)
            .await
    }

    async fn query_field_index(
        &self,
        tenant: &str,
        entity_type: &str,
        where_clause: &str,
        params: Vec<String>,
    ) -> Result<Option<Vec<String>>, PersistenceError> {
        let field_name = where_clause
            .contains("field_name")
            .then(|| params.last())
            .flatten();
        Ok(Some(
            self.list_query_projections(tenant, entity_type)
                .await?
                .into_iter()
                .filter(|(_, projection)| {
                    field_name.is_none_or(|name| projection.fields.get(name).is_some())
                })
                .map(|(entity_id, _)| entity_id)
                .collect(),
        ))
    }

    async fn query_field_index_page(
        &self,
        tenant: &str,
        entity_type: &str,
        _where_clause: &str,
        _params: Vec<String>,
        order_by: &[QueryFieldIndexOrder],
        skip: usize,
        top: usize,
        include_count: bool,
    ) -> Result<Option<QueryFieldIndexPage>, PersistenceError> {
        let _ = (tenant, entity_type, order_by, skip, top, include_count);
        // Redis retains authoritative projection documents but does not parse
        // the SQL-shaped page predicate. Use the common typed evaluator so
        // filtering and numeric ordering happen before pagination.
        Ok(None)
    }

    async fn load_projection_fields_many(
        &self,
        tenant: &str,
        entity_type: &str,
        entity_ids: &[String],
        field_names: &[&str],
    ) -> Result<Option<Vec<QueryProjectionFieldsRow>>, PersistenceError> {
        Ok(Some(
            self.load_query_projections(tenant, entity_type, entity_ids)
                .await?
                .into_iter()
                .map(|(entity_id, projection)| QueryProjectionFieldsRow {
                    entity_id,
                    status: projection.status,
                    fields: selected_fields(&projection.fields, field_names),
                })
                .collect(),
        ))
    }

    async fn load_entity_catalog_rows(
        &self,
        tenant: &str,
        entity_type: &str,
        entity_ids: &[String],
    ) -> Result<Option<Vec<EntityCatalogRow>>, PersistenceError> {
        Ok(Some(
            self.load_query_projections(tenant, entity_type, entity_ids)
                .await?
                .into_iter()
                .map(|(entity_id, projection)| EntityCatalogRow {
                    entity_id,
                    status: projection.status,
                    fields: projection.fields,
                    state: Some(projection.state),
                    sequence_nr: projection.sequence_nr,
                })
                .collect(),
        ))
    }

    async fn projected_entity_counts_by_tenant(
        &self,
    ) -> Result<Option<Vec<(String, u64)>>, PersistenceError> {
        Ok(None)
    }
}

fn selected_fields(
    fields: &serde_json::Value,
    field_names: &[&str],
) -> std::collections::BTreeMap<String, Option<String>> {
    field_names
        .iter()
        .filter_map(|name| {
            fields.get(*name).map(|value| {
                let value = if value.is_null() {
                    None
                } else {
                    Some(
                        value
                            .as_str()
                            .map_or_else(|| value.to_string(), str::to_string),
                    )
                };
                ((*name).to_string(), value)
            })
        })
        .collect()
}
