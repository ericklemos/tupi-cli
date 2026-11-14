use std::sync::Arc;

use async_trait::async_trait;
use tupi_domain::Message;

use crate::QueryError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Query {
    History,
    ListProviders,
    ListModels,
    ListWorkflows,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QueryResult {
    History(Vec<Message>),
    Providers(Vec<String>),
    Models(Vec<String>),
    Workflows(Vec<tupi_domain::Workflow>),
}

#[async_trait]
pub trait QueryHandler: Send + Sync {
    async fn query(&self, query: Query) -> Result<QueryResult, QueryError>;

    /// Roteia a consulta para a sessão indicada. `None` resolve no foco atual.
    async fn query_in(
        &self,
        _session: Option<tupi_domain::SessionId>,
        query: Query,
    ) -> Result<QueryResult, QueryError> {
        self.query(query).await
    }
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
        self.handler.query_in(None, query).await
    }

    /// Despacha a consulta para uma sessão explícita sem mover o foco.
    pub async fn dispatch_in(
        &self,
        session: tupi_domain::SessionId,
        query: Query,
    ) -> Result<QueryResult, QueryError> {
        self.handler.query_in(Some(session), query).await
    }
}
