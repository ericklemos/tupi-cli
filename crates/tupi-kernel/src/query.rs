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
    /// Planos SDD persistidos em `.tupi/sdd/`; a camada app filtra por status.
    ListSddSpecs,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QueryResult {
    History(Vec<Message>),
    Providers(Vec<String>),
    Models(Vec<String>),
    Workflows(Vec<tupi_domain::Workflow>),
    /// Resumos dos planos SDD na ordem devolvida pelo store.
    SddSpecs(Vec<tupi_domain::sdd::SddSpecSummary>),
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

#[cfg(test)]
mod tests {
    use super::*;
    use tupi_domain::sdd::{SddSpecStatus, SddSpecSummary};

    /// Handler fixo: devolve sempre a mesma sequência de resumos.
    struct FixedSpecs;

    #[async_trait::async_trait]
    impl QueryHandler for FixedSpecs {
        async fn query(&self, _query: Query) -> Result<QueryResult, QueryError> {
            Ok(QueryResult::SddSpecs(
                ["zeta", "alfa", "meio"]
                    .iter()
                    .map(|slug| summary(slug))
                    .collect(),
            ))
        }
    }

    fn summary(slug: &str) -> SddSpecSummary {
        SddSpecSummary::new(slug, format!("Plano {slug}"), SddSpecStatus::Draft, 1, 2)
    }

    #[tokio::test]
    async fn sdd_specs_keeps_the_order_it_received() {
        let bus = QueryBus::new(Arc::new(FixedSpecs));
        let result = match bus.dispatch(Query::ListSddSpecs).await {
            Ok(result) => result,
            Err(error) => panic!("dispatch falhou: {error}"),
        };
        let QueryResult::SddSpecs(specs) = result else {
            panic!("esperava QueryResult::SddSpecs");
        };
        let slugs: Vec<&str> = specs.iter().map(|spec| spec.slug.as_str()).collect();
        assert_eq!(slugs, ["zeta", "alfa", "meio"]);
    }
}
