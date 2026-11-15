use std::sync::Arc;

use async_trait::async_trait;
use tupi_app::App;
use tupi_domain::sdd::SddSpecStatus;
use tupi_kernel::{
    ChatRequest, EventBus, Provider, ProviderError, ProviderEvent, ProviderRegistry,
    ProviderStream, Query, QueryError, QueryHandler, QueryResult,
};
use tupi_tools::sdd::{InMemorySddStore, SddPlan, SddPlanStatus, SddStore, SddTask, SddTaskStatus};

/// Provedor que nunca é chamado pela listagem de planos.
struct UnusedProvider;

#[async_trait]
impl Provider for UnusedProvider {
    fn id(&self) -> &str {
        "scripted"
    }

    async fn list_models(&self) -> Result<Vec<String>, ProviderError> {
        Ok(Vec::new())
    }

    async fn stream_chat(&self, _request: ChatRequest) -> Result<ProviderStream, ProviderError> {
        Ok(Box::pin(futures_util::stream::empty::<
            Result<ProviderEvent, ProviderError>,
        >()))
    }
}

fn app(store: Option<Arc<dyn SddStore>>) -> Arc<App> {
    let registry = ProviderRegistry::new();
    registry.register(Arc::new(UnusedProvider));
    let app = App::new(registry, "scripted", "modelo".into(), EventBus::new(8))
        .with_repo_key("repo".into());
    match store {
        Some(store) => app.with_sdd_store(store),
        None => app,
    }
}

fn plan(slug: &str, status: SddPlanStatus, tasks: usize, done: usize) -> SddPlan {
    SddPlan {
        slug: slug.to_string(),
        repo_key: "repo".to_string(),
        title: format!("Plano {slug}"),
        status,
        goal: "objetivo".to_string(),
        tasks: (0..tasks)
            .map(|index| SddTask {
                id: format!("T{}", index + 1),
                title: format!("Task {}", index + 1),
                details: String::new(),
                nuances: Vec::new(),
                parallel: false,
                status: if index < done {
                    SddTaskStatus::Done
                } else {
                    SddTaskStatus::Open
                },
                subtasks: Vec::new(),
            })
            .collect(),
        created_at: 1,
        updated_at: 1,
        ..Default::default()
    }
}

async fn specs(app: &Arc<App>) -> Vec<tupi_domain::sdd::SddSpecSummary> {
    match app.query(Query::ListSddSpecs).await.unwrap() {
        QueryResult::SddSpecs(specs) => specs,
        other => panic!("esperava SddSpecs, veio {other:?}"),
    }
}

#[tokio::test]
async fn list_sdd_specs_hides_only_completed_plans() {
    let store = Arc::new(InMemorySddStore::default());
    store
        .save(&plan("auth", SddPlanStatus::Draft, 3, 1))
        .unwrap();
    store
        .save(&plan("velho", SddPlanStatus::Completed, 2, 2))
        .unwrap();
    store
        .save(&plan("quebrado", SddPlanStatus::Failed, 1, 0))
        .unwrap();

    let specs = specs(&app(Some(store))).await;

    let slugs: Vec<&str> = specs.iter().map(|spec| spec.slug.as_str()).collect();
    assert_eq!(slugs, vec!["auth", "quebrado"], "{specs:?}");
    assert_eq!(specs[0].slug, "auth");
    assert_eq!(specs[0].title, "Plano auth");
    assert_eq!(specs[0].status, SddSpecStatus::Draft);
    assert_eq!(specs[0].open_tasks, 2);
    assert_eq!(specs[0].total_tasks, 3);
    assert_eq!(specs[1].status, SddSpecStatus::Failed);
}

#[tokio::test]
async fn closed_plans_stay_resumable_and_keep_the_store_order() {
    let store = Arc::new(InMemorySddStore::default());
    let mut novo = plan("novo", SddPlanStatus::Draft, 1, 0);
    novo.updated_at = 10;
    let mut fechado = plan("fechado", SddPlanStatus::Closed, 2, 1);
    fechado.updated_at = 5;
    store.save(&novo).unwrap();
    store.save(&fechado).unwrap();

    let specs = specs(&app(Some(store))).await;

    let slugs: Vec<&str> = specs.iter().map(|spec| spec.slug.as_str()).collect();
    assert_eq!(slugs, vec!["novo", "fechado"]);
    assert_eq!(specs[1].status, SddSpecStatus::Draft);
}

#[tokio::test]
async fn listing_without_store_degrades_with_a_clear_error() {
    let error = app(None).query(Query::ListSddSpecs).await.unwrap_err();

    match error {
        QueryError::Internal(message) => {
            assert!(message.contains("SddStore"), "{message}");
        }
        other => panic!("esperava Internal, veio {other:?}"),
    }
}
