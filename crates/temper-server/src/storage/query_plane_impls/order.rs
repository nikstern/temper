use temper_runtime::persistence::{QueryProjectionOrder, QueryProjectionOrderTarget};

use super::super::{
    QueryFieldIndexOrder, QueryFieldIndexOrderDirection, QueryFieldIndexOrderTarget,
};

pub(super) fn storage_order_by(order_by: &[QueryFieldIndexOrder]) -> Vec<QueryProjectionOrder> {
    order_by
        .iter()
        .map(|order| QueryProjectionOrder {
            target: match &order.target {
                QueryFieldIndexOrderTarget::Property(field) => {
                    QueryProjectionOrderTarget::Property(field.clone())
                }
                QueryFieldIndexOrderTarget::Status => QueryProjectionOrderTarget::Status,
                QueryFieldIndexOrderTarget::EntityId => QueryProjectionOrderTarget::EntityId,
                QueryFieldIndexOrderTarget::EntityCommitSequence => {
                    QueryProjectionOrderTarget::EntityCommitSequence
                }
            },
            descending: order.direction == QueryFieldIndexOrderDirection::Desc,
        })
        .collect()
}
