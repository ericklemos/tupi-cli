use std::sync::Arc;

use async_trait::async_trait;
use tupi_domain::Message;

use crate::QueryError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Query {
    History,
    ListProviders,
    ListModels,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QueryResult {
    History(Vec<Message>),
    Providers(Vec<String>),
    Models(Vec<String>),
}

#[async_trait]
pub trait QueryHandler: Send + Sync {
    async fn query(&self, query: Query) -> Result<QueryResult, QueryError>;
}

#[derive(Clone)]
pub struct QueryBus {
    handler: Arc<dyn QueryHandler>,
}

impl QueryBus {
    pub fn new(handler: Arc<dyn QueryHandler>) -> Self {
        Self { handler }
    }

    pub async fn dispatch(&self, query: Query) -> Result<QueryResult, QueryError> {
        self.handler.query(query).await
    }
}
