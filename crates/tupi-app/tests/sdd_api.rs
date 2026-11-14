use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use futures_util::stream;
use tupi_kernel::{
    ChatRequest, EventBus, Provider, ProviderError, ProviderEvent, ProviderStream, ToolRegistry,
};
use tupi_tools::sdd::{
    InMemorySddStore, SddPlan, SddPlanStatus, SddStore, SddSubtask, SddSubtaskStatus, SddTask,
    SddTaskStatus,
};

struct Scripted {
    calls: Mutex<Vec<Vec<Result<ProviderEvent, ProviderError>>>>,
}

#[async_trait]
impl Provider for Scripted {
    fn id(&self) -> &str {
        "scripted"
    }

    async fn list_models(&self) -> Result<Vec<String>, ProviderError> {
        Ok(Vec::new())
    }

    async fn stream_chat(&self, _request: ChatRequest) -> Result<ProviderStream, ProviderError> {
        let events = self.calls.lock().unwrap().remove(0);
        Ok(Box::pin(stream::iter(events)))
    }
}

fn plan() -> SddPlan {
    SddPlan {
        slug: "auth".to_string(),
        repo_key: "repo".to_string(),
        title: "Auth".to_string(),
        status: SddPlanStatus::Closed,
        goal: "OAuth".to_string(),
        constraints: vec![],
        nuances: vec![],
        tasks: vec![SddTask {
            id: "T1".to_string(),
            title: "Refresh".to_string(),
            details: String::new(),
            nuances: vec![],
            parallel: false,
            status: SddTaskStatus::Open,
            subtasks: vec![SddSubtask {
                id: "T1.1".to_string(),
                title: "Lock".to_string(),
                acceptance: String::new(),
                touches: vec![],
                acceptance_test: None,
                status: SddSubtaskStatus::Open,
            }],
        }],
        created_at: 1,
        updated_at: 1,
        closed_at: Some(1),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn blocking_execution_runs_inside_spawn_blocking() {
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(InMemorySddStore::default());
    store.save(&plan()).unwrap();
    let provider: Arc<dyn Provider> = Arc::new(Scripted {
        calls: Mutex::new(vec![vec![Ok(ProviderEvent::Text("feito T1.1".into()))]]),
    });
    let request = tupi_app::sdd_api::SddExecRequest {
        root: dir.path().to_path_buf(),
        store: store.clone(),
        repo_key: "repo".to_string(),
        tools: ToolRegistry::new(),
        agent_tools: Arc::new(|| ToolRegistry::new()),
        provider,
        model: "m".to_string(),
        agent_prefix: "sys".to_string(),
        max_parallel: 2,
        agent_max_steps: 4,
        agent_timeout: Duration::from_secs(5),
        events: EventBus::new(16),
        request_id: 0,
        cancel: Arc::new(AtomicBool::new(false)),
        in_flight: Arc::new(tokio::sync::Notify::new()),
        in_flight_count: Arc::new(AtomicUsize::new(0)),
    };

    let outcome = tokio::time::timeout(
        Duration::from_secs(10),
        tokio::task::spawn_blocking(move || {
            tokio::runtime::Handle::current().block_on(tupi_app::sdd_api::execute(request, "auth"))
        }),
    )
    .await
    .expect("a execução bloqueante não pode travar")
    .unwrap();

    assert!(outcome.ok, "{}", outcome.report);
    let saved = store.load("repo", "auth").unwrap().unwrap();
    assert_eq!(saved.status, SddPlanStatus::Completed);
    assert_eq!(saved.tasks[0].subtasks[0].status, SddSubtaskStatus::Done);
}
