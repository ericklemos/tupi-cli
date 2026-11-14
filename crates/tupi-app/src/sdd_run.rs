//! Agent turn runner and SDD plan orchestrator.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use futures_util::StreamExt;
use tupi_domain::{Event, Message, Role, ToolCall};
use tupi_kernel::{ChatRequest, EventBus, Provider, ProviderEvent, ToolRegistry};
use tupi_tools::sdd::{
    AcceptanceTest, SddPlan, SddPlanStatus, SddStore, SddSubtask, SddSubtaskStatus, SddTask,
    SddTaskStatus, write_mirror,
};

use crate::app::run_tool;
use crate::sdd::{format_run_report, format_subtask_brief};

/// Um turno de agente enxuto: sem poda, memória, telemetria ou handoff.
pub(crate) struct AgentTurn<'a> {
    pub provider: &'a Arc<dyn Provider>,
    pub model: &'a str,
    pub prefix: &'a str,
    pub tools: &'a ToolRegistry,
    pub max_steps: usize,
    pub timeout: Duration,
    pub cancel: Arc<AtomicBool>,
    pub in_flight: Arc<tokio::sync::Notify>,
    pub in_flight_count: Arc<AtomicUsize>,
}

#[derive(Debug, Clone)]
pub(crate) struct AgentOutcome {
    pub ok: bool,
    pub text: String,
    pub error: Option<String>,
}

impl AgentOutcome {
    fn failure(message: impl Into<String>) -> Self {
        Self {
            ok: false,
            text: String::new(),
            error: Some(message.into()),
        }
    }
}

pub(crate) async fn run_agent(turn: AgentTurn<'_>, brief: &str) -> AgentOutcome {
    let messages = vec![
        Message::new(Role::System, turn.prefix),
        Message::new(Role::User, brief),
    ];
    match tokio::time::timeout(turn.timeout, run_agent_loop(&turn, messages)).await {
        Ok(outcome) => outcome,
        Err(_) => AgentOutcome::failure("timeout do agente"),
    }
}

async fn run_agent_loop(turn: &AgentTurn<'_>, mut messages: Vec<Message>) -> AgentOutcome {
    for _ in 0..turn.max_steps {
        if turn.cancel.load(Ordering::SeqCst) {
            return AgentOutcome::failure("cancelado");
        }
        let request = ChatRequest {
            model: turn.model.to_string(),
            messages: messages.clone(),
            tools: turn.tools.specs(),
        };
        let mut stream = match turn.provider.stream_chat(request).await {
            Ok(stream) => stream,
            Err(error) => return AgentOutcome::failure(error.to_string()),
        };
        let mut text = String::new();
        let mut calls = Vec::new();
        while let Some(item) = stream.next().await {
            match item {
                Ok(ProviderEvent::Text(chunk)) => text.push_str(&chunk),
                Ok(ProviderEvent::ToolCalls(inner)) => calls.extend(inner),
                Ok(_) => {}
                Err(error) => return AgentOutcome::failure(error.to_string()),
            }
        }
        if calls.is_empty() {
            return AgentOutcome {
                ok: true,
                text,
                error: None,
            };
        }
        messages.push(Message::assistant_with_calls(text, calls.clone()));
        for call in calls {
            let content = match run_tool(
                turn.tools,
                &call,
                &turn.cancel,
                &turn.in_flight,
                &turn.in_flight_count,
            )
            .await
            {
                Ok(content) => content,
                Err(error) => format!("erro: {error}"),
            };
            messages.push(Message::tool(&call.id, content));
        }
    }
    AgentOutcome::failure(format!("limite de {} passos", turn.max_steps))
}

/// Tudo que o orquestrador precisa para rodar um plano fechado.
pub(crate) struct RunContext {
    pub root: PathBuf,
    pub store: Arc<dyn SddStore>,
    pub repo_key: String,
    pub tools: ToolRegistry,
    pub agent_tools: Arc<dyn Fn() -> ToolRegistry + Send + Sync>,
    pub provider: Arc<dyn Provider>,
    pub model: String,
    pub agent_prefix: String,
    pub max_parallel: usize,
    pub agent_max_steps: usize,
    pub agent_timeout: Duration,
    pub events: EventBus,
    pub request_id: u64,
    pub cancel: Arc<AtomicBool>,
    pub in_flight: Arc<tokio::sync::Notify>,
    pub in_flight_count: Arc<AtomicUsize>,
}

#[derive(Debug, Clone)]
pub(crate) struct SddRunOutcome {
    pub ok: bool,
    pub report: String,
}

/// Executa as tasks abertas do plano: uma stage por task, com join no fim.
pub(crate) async fn run_plan(ctx: &RunContext, slug: &str) -> SddRunOutcome {
    let mut plan = match ctx.store.load(&ctx.repo_key, slug) {
        Ok(Some(plan)) if plan.status == SddPlanStatus::Closed => plan,
        Ok(Some(_)) => return failure("plano SDD não está fechado"),
        Ok(None) => return failure("plano SDD não encontrado"),
        Err(error) => return failure(format!("falha ao carregar plano SDD: {error}")),
    };

    let mut lines = Vec::new();
    let mut ok = true;

    'tasks: for task_index in 0..plan.tasks.len() {
        if ctx.cancel.load(Ordering::SeqCst) {
            ok = false;
            break 'tasks;
        }
        if plan.tasks[task_index].status == SddTaskStatus::Done {
            continue;
        }
        plan.tasks[task_index].status = SddTaskStatus::InProgress;
        if !save(ctx, &plan) {
            return failure("falha ao salvar plano SDD");
        }
        mirror(ctx, &plan);

        let task_id = plan.tasks[task_index].id.clone();
        let parallel = stage_is_parallel(&plan.tasks[task_index]);
        if plan.tasks[task_index].parallel && !parallel {
            ctx.events.publish(Event::Notice {
                message: format!(
                    "SDD {}: subtasks com arquivos sobrepostos; rodando em série",
                    task_id
                ),
            });
        }
        let subtask_ids: Vec<String> = if plan.tasks[task_index].subtasks.is_empty() {
            vec![task_id.clone()]
        } else {
            plan.tasks[task_index]
                .subtasks
                .iter()
                .filter(|sub| sub.status != SddSubtaskStatus::Done)
                .map(|sub| sub.id.clone())
                .collect()
        };

        if parallel && subtask_ids.len() > 1 {
            let plan_ref = &plan;
            let results = {
                let mut stream = futures_util::stream::iter(subtask_ids.iter().cloned())
                    .map(|sub_id| async move {
                        let (outcome, elapsed) = run_one(ctx, plan_ref, &sub_id).await;
                        (sub_id, outcome, elapsed)
                    })
                    .buffer_unordered(ctx.max_parallel.max(1));
                let mut results = Vec::new();
                while let Some(result) = stream.next().await {
                    results.push(result);
                }
                results
            };
            let mut results = results;
            results.sort_by_key(|(id, _, _)| {
                subtask_ids
                    .iter()
                    .position(|candidate| candidate == id)
                    .unwrap_or(usize::MAX)
            });
            let mut stage_ok = true;
            for (sub_id, outcome, elapsed) in results {
                if !record_result(
                    ctx, &mut plan, &task_id, &sub_id, outcome, elapsed, &mut lines,
                )
                .await
                {
                    stage_ok = false;
                }
            }
            if !stage_ok {
                ok = false;
                break 'tasks;
            }
        } else {
            for sub_id in subtask_ids {
                if ctx.cancel.load(Ordering::SeqCst) {
                    ok = false;
                    break 'tasks;
                }
                let (outcome, elapsed) = run_one(ctx, &plan, &sub_id).await;
                if !record_result(
                    ctx, &mut plan, &task_id, &sub_id, outcome, elapsed, &mut lines,
                )
                .await
                {
                    ok = false;
                    break 'tasks;
                }
            }
        }
    }

    if ok
        && plan
            .tasks
            .iter()
            .all(|task| task.status == SddTaskStatus::Done)
    {
        plan.status = SddPlanStatus::Completed;
        save(ctx, &plan);
        mirror(ctx, &plan);
    }

    SddRunOutcome {
        ok,
        report: format_run_report(&plan, &lines),
    }
}

fn stage_is_parallel(task: &SddTask) -> bool {
    task.parallel && !has_declared_overlap(&task.subtasks)
}

fn has_declared_overlap(subtasks: &[SddSubtask]) -> bool {
    for (index, left) in subtasks.iter().enumerate() {
        for right in &subtasks[index + 1..] {
            if left.touches.iter().any(|path| right.touches.contains(path)) {
                return true;
            }
        }
    }
    false
}

async fn run_one(ctx: &RunContext, plan: &SddPlan, sub_id: &str) -> (AgentOutcome, Duration) {
    let brief = {
        let Some((task, sub)) = find_target(plan, sub_id) else {
            return (
                AgentOutcome::failure("subtask não encontrada"),
                Duration::ZERO,
            );
        };
        format_subtask_brief(plan, task, sub)
    };
    let tools = (ctx.agent_tools)();
    ctx.events.publish(Event::ToolStarted {
        request_id: ctx.request_id,
        call_id: format!("sdd#{sub_id}"),
        name: "agent".to_string(),
        arguments: "{}".to_string(),
        summary: format!("{sub_id} — {}", find_title(plan, sub_id)),
    });

    let started = Instant::now();
    let outcome = run_agent(
        AgentTurn {
            provider: &ctx.provider,
            model: &ctx.model,
            prefix: &ctx.agent_prefix,
            tools: &tools,
            max_steps: ctx.agent_max_steps,
            timeout: ctx.agent_timeout,
            cancel: ctx.cancel.clone(),
            in_flight: ctx.in_flight.clone(),
            in_flight_count: ctx.in_flight_count.clone(),
        },
        &brief,
    )
    .await;
    ctx.events.publish(Event::ToolFinished {
        request_id: ctx.request_id,
        call_id: format!("sdd#{sub_id}"),
        name: "agent".to_string(),
        ok: outcome.ok,
        summary: summarize_outcome(&outcome),
    });
    (outcome, started.elapsed())
}

async fn record_result(
    ctx: &RunContext,
    plan: &mut SddPlan,
    task_id: &str,
    sub_id: &str,
    outcome: AgentOutcome,
    elapsed: Duration,
    lines: &mut Vec<String>,
) -> bool {
    let summary = summarize_outcome(&outcome);
    let mut ok = outcome.ok;
    let mut acceptance_error = None;

    if ok {
        if let Some(test) = find_acceptance(plan, sub_id) {
            if let Err(error) = run_acceptance(ctx, &test).await {
                ok = false;
                acceptance_error = Some(error);
            }
        }
    }

    if ok {
        mark_done(plan, task_id, sub_id);
        lines.push(format!(
            "{sub_id} ok {:.1}s — {summary}",
            elapsed.as_secs_f32()
        ));
    } else {
        let reason = match acceptance_error {
            Some(error) => format!("aceite falhou: {error}"),
            None => outcome
                .error
                .clone()
                .unwrap_or_else(|| "sem detalhe".to_string()),
        };
        lines.push(format!("{sub_id} falhou: {reason}"));
    }

    sync_task(plan, task_id);
    if !save(ctx, plan) {
        return false;
    }
    mirror(ctx, plan);
    ok
}

async fn run_acceptance(ctx: &RunContext, test: &AcceptanceTest) -> Result<(), String> {
    if !ctx.tools.has("exec") {
        return Err("tool exec não registrada para o aceite".to_string());
    }
    let mut arguments = serde_json::json!({ "kind": "test", "filter": test.filter });
    if let Some(package) = &test.package {
        arguments["package"] = serde_json::json!(package);
    }
    let call = ToolCall {
        id: format!("sdd-aceite-{}", test.filter),
        name: "exec".to_string(),
        arguments: arguments.to_string(),
    };
    let output = run_tool(
        &ctx.tools,
        &call,
        &ctx.cancel,
        &ctx.in_flight,
        &ctx.in_flight_count,
    )
    .await?;
    if output.contains(" 0 passed") {
        return Err("nenhum teste casou o filtro do aceite".to_string());
    }
    Ok(())
}

fn find_acceptance(plan: &SddPlan, id: &str) -> Option<AcceptanceTest> {
    plan.tasks
        .iter()
        .flat_map(|task| task.subtasks.iter())
        .find(|sub| sub.id == id)
        .and_then(|sub| sub.acceptance_test.clone())
}

fn failure(message: impl Into<String>) -> SddRunOutcome {
    SddRunOutcome {
        ok: false,
        report: format!("Execução SDD interrompida: {}", message.into()),
    }
}

fn save(ctx: &RunContext, plan: &SddPlan) -> bool {
    ctx.store.save(plan).is_ok()
}

fn mirror(ctx: &RunContext, plan: &SddPlan) {
    if let Err(error) = write_mirror(&ctx.root, plan) {
        ctx.events.publish(Event::Notice {
            message: format!("SDD: falha ao gravar espelho: {error}"),
        });
    }
}

fn find_target<'a>(plan: &'a SddPlan, id: &str) -> Option<(&'a SddTask, Option<&'a SddSubtask>)> {
    for task in &plan.tasks {
        if task.id == id {
            return Some((task, None));
        }
        if let Some(sub) = task.subtasks.iter().find(|sub| sub.id == id) {
            return Some((task, Some(sub)));
        }
    }
    None
}

fn find_title(plan: &SddPlan, id: &str) -> String {
    find_target(plan, id)
        .map(|(task, sub)| {
            sub.map(|s| s.title.clone())
                .unwrap_or_else(|| task.title.clone())
        })
        .unwrap_or_else(|| id.to_string())
}

fn mark_done(plan: &mut SddPlan, task_id: &str, sub_id: &str) {
    let Some(task) = plan.tasks.iter_mut().find(|task| task.id == task_id) else {
        return;
    };
    if task.subtasks.is_empty() {
        task.status = SddTaskStatus::Done;
        return;
    }
    if let Some(sub) = task.subtasks.iter_mut().find(|sub| sub.id == sub_id) {
        sub.status = SddSubtaskStatus::Done;
    }
}

fn sync_task(plan: &mut SddPlan, task_id: &str) {
    let Some(task) = plan.tasks.iter_mut().find(|task| task.id == task_id) else {
        return;
    };
    if task.subtasks.is_empty() {
        if task.status == SddTaskStatus::InProgress {
            task.status = SddTaskStatus::Open;
        }
        return;
    }
    let all_done = task
        .subtasks
        .iter()
        .all(|sub| sub.status == SddSubtaskStatus::Done);
    task.status = if all_done {
        SddTaskStatus::Done
    } else {
        SddTaskStatus::Open
    };
}

fn summarize_outcome(outcome: &AgentOutcome) -> String {
    let line = outcome
        .text
        .lines()
        .find(|line| !line.trim().is_empty())
        .unwrap_or("")
        .trim();
    let mut summary: String = line.chars().take(120).collect();
    if summary.is_empty() && !outcome.ok {
        summary = outcome.error.clone().unwrap_or_default();
    }
    summary
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use async_trait::async_trait;
    use futures_util::stream;
    use tupi_domain::ToolCall;
    use tupi_kernel::{
        ChatRequest, Provider, ProviderError, ProviderEvent, ProviderStream, Tool, ToolError,
        ToolRegistry, ToolSpec,
    };
    use tupi_tools::sdd::InMemorySddStore;

    use super::*;

    struct Scripted {
        calls: Mutex<VecDeque<Vec<Result<ProviderEvent, ProviderError>>>>,
        requests: Arc<Mutex<Vec<ChatRequest>>>,
    }

    impl Scripted {
        fn new(calls: Vec<Vec<Result<ProviderEvent, ProviderError>>>) -> Arc<dyn Provider> {
            Self::new_tracked(calls).0
        }

        fn new_tracked(
            calls: Vec<Vec<Result<ProviderEvent, ProviderError>>>,
        ) -> (Arc<dyn Provider>, Arc<Mutex<Vec<ChatRequest>>>) {
            let requests = Arc::new(Mutex::new(Vec::new()));
            let provider = Arc::new(Self {
                calls: Mutex::new(calls.into()),
                requests: requests.clone(),
            });
            (provider, requests)
        }
    }

    #[async_trait]
    impl Provider for Scripted {
        fn id(&self) -> &str {
            "scripted"
        }

        async fn list_models(&self) -> Result<Vec<String>, ProviderError> {
            Ok(Vec::new())
        }

        async fn stream_chat(&self, request: ChatRequest) -> Result<ProviderStream, ProviderError> {
            self.requests.lock().unwrap().push(request);
            let events = self.calls.lock().unwrap().pop_front().unwrap_or_default();
            Ok(Box::pin(stream::iter(events)))
        }
    }

    struct SlowProvider {
        active: AtomicUsize,
        peak: AtomicUsize,
        requests: Arc<Mutex<Vec<ChatRequest>>>,
    }

    impl SlowProvider {
        fn new() -> (Arc<Self>, Arc<Mutex<Vec<ChatRequest>>>) {
            let requests = Arc::new(Mutex::new(Vec::new()));
            let provider = Arc::new(Self {
                active: AtomicUsize::new(0),
                peak: AtomicUsize::new(0),
                requests: requests.clone(),
            });
            (provider, requests)
        }
    }

    #[async_trait]
    impl Provider for SlowProvider {
        fn id(&self) -> &str {
            "slow"
        }

        async fn list_models(&self) -> Result<Vec<String>, ProviderError> {
            Ok(Vec::new())
        }

        async fn stream_chat(&self, request: ChatRequest) -> Result<ProviderStream, ProviderError> {
            self.requests.lock().unwrap().push(request);
            let now = self.active.fetch_add(1, Ordering::SeqCst) + 1;
            self.peak.fetch_max(now, Ordering::SeqCst);
            tokio::time::sleep(Duration::from_millis(50)).await;
            self.active.fetch_sub(1, Ordering::SeqCst);
            Ok(Box::pin(stream::iter(vec![Ok(ProviderEvent::Text(
                "feito".to_string(),
            ))])))
        }
    }

    struct CancelProvider {
        cancel: Arc<AtomicBool>,
        requests: Arc<Mutex<Vec<ChatRequest>>>,
    }

    #[async_trait]
    impl Provider for CancelProvider {
        fn id(&self) -> &str {
            "cancel"
        }

        async fn list_models(&self) -> Result<Vec<String>, ProviderError> {
            Ok(Vec::new())
        }

        async fn stream_chat(&self, request: ChatRequest) -> Result<ProviderStream, ProviderError> {
            self.requests.lock().unwrap().push(request);
            self.cancel.store(true, Ordering::SeqCst);
            Ok(Box::pin(stream::iter(vec![Ok(ProviderEvent::Text(
                "feito".to_string(),
            ))])))
        }
    }

    struct EchoTool {
        hits: Arc<AtomicUsize>,
    }

    impl Tool for EchoTool {
        fn spec(&self) -> ToolSpec {
            ToolSpec {
                name: "echo".to_string(),
                description: "eco".to_string(),
                schema: serde_json::json!({"type": "object"}),
            }
        }

        fn call(&self, _arguments: serde_json::Value) -> Result<String, ToolError> {
            self.hits.fetch_add(1, Ordering::SeqCst);
            Ok("conteudo".to_string())
        }
    }

    struct ExecTool {
        output: String,
        fail: bool,
    }

    impl Tool for ExecTool {
        fn spec(&self) -> ToolSpec {
            ToolSpec {
                name: "exec".to_string(),
                description: "exec".to_string(),
                schema: serde_json::json!({"type": "object"}),
            }
        }

        fn call(&self, _arguments: serde_json::Value) -> Result<String, ToolError> {
            if self.fail {
                Err(ToolError::Other("teste quebrou".to_string()))
            } else {
                Ok(self.output.clone())
            }
        }
    }

    fn call_event(id: &str) -> Result<ProviderEvent, ProviderError> {
        Ok(ProviderEvent::ToolCalls(vec![ToolCall {
            id: id.to_string(),
            name: "echo".to_string(),
            arguments: "{}".to_string(),
        }]))
    }

    fn text_event(value: &str) -> Result<ProviderEvent, ProviderError> {
        Ok(ProviderEvent::Text(value.to_string()))
    }

    fn fixture<'a>(
        provider: &'a Arc<dyn Provider>,
        tools: &'a ToolRegistry,
        max_steps: usize,
        cancel: Arc<AtomicBool>,
    ) -> AgentTurn<'a> {
        AgentTurn {
            provider,
            model: "m",
            prefix: "sys",
            tools,
            max_steps,
            timeout: Duration::from_secs(5),
            cancel,
            in_flight: Arc::new(tokio::sync::Notify::new()),
            in_flight_count: Arc::new(AtomicUsize::new(0)),
        }
    }

    #[tokio::test]
    async fn agent_runs_a_tool_and_returns_the_final_text() {
        let provider = Scripted::new(vec![vec![call_event("c1")], vec![text_event("feito")]]);
        let hits = Arc::new(AtomicUsize::new(0));
        let mut tools = ToolRegistry::new();
        tools.register(Arc::new(EchoTool { hits: hits.clone() }));
        let cancel = Arc::new(AtomicBool::new(false));

        let outcome = run_agent(fixture(&provider, &tools, 4, cancel), "brief").await;

        assert!(outcome.ok, "{:?}", outcome.error);
        assert_eq!(outcome.text, "feito");
        assert_eq!(hits.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn agent_stops_at_max_steps_and_reports_failure() {
        let provider = Scripted::new(vec![
            vec![call_event("c1")],
            vec![call_event("c2")],
            vec![call_event("c3")],
        ]);
        let hits = Arc::new(AtomicUsize::new(0));
        let mut tools = ToolRegistry::new();
        tools.register(Arc::new(EchoTool { hits }));
        let cancel = Arc::new(AtomicBool::new(false));

        let outcome = run_agent(fixture(&provider, &tools, 2, cancel), "brief").await;

        assert!(!outcome.ok);
        assert!(outcome.error.unwrap_or_default().contains("passos"));
    }

    #[tokio::test]
    async fn agent_provider_error_is_a_failure() {
        let provider = Scripted::new(vec![vec![Err(ProviderError::Network("caiu".into()))]]);
        let tools = ToolRegistry::new();
        let cancel = Arc::new(AtomicBool::new(false));

        let outcome = run_agent(fixture(&provider, &tools, 4, cancel), "brief").await;

        assert!(!outcome.ok);
        assert!(outcome.error.unwrap_or_default().contains("caiu"));
    }

    #[tokio::test]
    async fn agent_observes_cancel_before_the_next_step() {
        let provider = Scripted::new(vec![vec![text_event("nunca")]]);
        let tools = ToolRegistry::new();
        let cancel = Arc::new(AtomicBool::new(true));

        let outcome = run_agent(fixture(&provider, &tools, 4, cancel), "brief").await;

        assert!(!outcome.ok);
        assert!(outcome.error.unwrap_or_default().contains("cancelado"));
    }

    fn subtask(id: &str) -> SddSubtask {
        SddSubtask {
            id: id.to_string(),
            title: format!("Sub {id}"),
            acceptance: String::new(),
            touches: vec![],
            acceptance_test: None,
            status: SddSubtaskStatus::Open,
        }
    }

    fn task(id: &str, subtasks: Vec<SddSubtask>) -> SddTask {
        SddTask {
            id: id.to_string(),
            title: format!("Task {id}"),
            details: String::new(),
            nuances: vec![],
            parallel: false,
            status: SddTaskStatus::Open,
            subtasks,
        }
    }

    fn closed_plan(tasks: Vec<SddTask>) -> SddPlan {
        SddPlan {
            slug: "auth".to_string(),
            repo_key: "repo".to_string(),
            title: "Auth".to_string(),
            status: SddPlanStatus::Closed,
            goal: "OAuth".to_string(),
            constraints: vec![],
            nuances: vec![],
            tasks,
            created_at: 1,
            updated_at: 1,
            closed_at: Some(1),
        }
    }

    fn run_context(
        root: &std::path::Path,
        store: Arc<InMemorySddStore>,
        provider: Arc<dyn Provider>,
        tools: ToolRegistry,
        cancel: Arc<AtomicBool>,
    ) -> RunContext {
        RunContext {
            root: root.to_path_buf(),
            store,
            repo_key: "repo".to_string(),
            tools,
            agent_tools: Arc::new(|| ToolRegistry::new()),
            provider,
            model: "m".to_string(),
            agent_prefix: "sys".to_string(),
            max_parallel: 4,
            agent_max_steps: 4,
            agent_timeout: Duration::from_secs(5),
            events: EventBus::new(64),
            request_id: 1,
            cancel,
            in_flight: Arc::new(tokio::sync::Notify::new()),
            in_flight_count: Arc::new(AtomicUsize::new(0)),
        }
    }

    fn requested_subtasks(requests: &Arc<Mutex<Vec<ChatRequest>>>) -> Vec<String> {
        requests
            .lock()
            .unwrap()
            .iter()
            .map(|request| {
                let text = &request.messages.last().unwrap().content;
                text.split("Subtarefa: ")
                    .nth(1)
                    .and_then(|rest| rest.split(' ').next())
                    .unwrap_or("?")
                    .to_string()
            })
            .collect()
    }

    #[tokio::test]
    async fn sequential_tasks_run_in_order_and_complete_the_plan() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(InMemorySddStore::default());
        let plan = closed_plan(vec![
            task("T1", vec![subtask("T1.1"), subtask("T1.2")]),
            task("T2", vec![subtask("T2.1")]),
        ]);
        store.save(&plan).unwrap();
        let (provider, requests) = Scripted::new_tracked(vec![
            vec![text_event("feito T1.1")],
            vec![text_event("feito T1.2")],
            vec![text_event("feito T2.1")],
        ]);
        let ctx = run_context(
            dir.path(),
            store.clone(),
            provider,
            ToolRegistry::new(),
            Arc::new(AtomicBool::new(false)),
        );

        let outcome = run_plan(&ctx, "auth").await;

        assert!(outcome.ok, "{}", outcome.report);
        assert_eq!(requested_subtasks(&requests), vec!["T1.1", "T1.2", "T2.1"]);
        let saved = store.load("repo", "auth").unwrap().unwrap();
        assert_eq!(saved.status, SddPlanStatus::Completed);
        assert!(
            saved
                .tasks
                .iter()
                .all(|task| task.status == SddTaskStatus::Done)
        );
        let md = std::fs::read_to_string(dir.path().join(".agents/tupi/sdd/auth.md")).unwrap();
        assert!(md.contains("[x]"), "{md}");
    }

    #[tokio::test]
    async fn a_task_without_subtasks_runs_one_agent_and_closes_the_task() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(InMemorySddStore::default());
        store.save(&closed_plan(vec![task("T1", vec![])])).unwrap();
        let (provider, requests) = Scripted::new_tracked(vec![vec![text_event("feito T1")]]);
        let ctx = run_context(
            dir.path(),
            store.clone(),
            provider,
            ToolRegistry::new(),
            Arc::new(AtomicBool::new(false)),
        );

        let outcome = run_plan(&ctx, "auth").await;

        assert!(outcome.ok, "{}", outcome.report);
        assert_eq!(requests.lock().unwrap().len(), 1);
        let saved = store.load("repo", "auth").unwrap().unwrap();
        assert_eq!(saved.tasks[0].status, SddTaskStatus::Done);
        assert_eq!(saved.status, SddPlanStatus::Completed);
    }

    #[tokio::test]
    async fn done_subtasks_are_skipped() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(InMemorySddStore::default());
        let mut first = subtask("T1.1");
        first.status = SddSubtaskStatus::Done;
        store
            .save(&closed_plan(vec![task("T1", vec![first, subtask("T1.2")])]))
            .unwrap();
        let (provider, requests) = Scripted::new_tracked(vec![vec![text_event("feito T1.2")]]);
        let ctx = run_context(
            dir.path(),
            store,
            provider,
            ToolRegistry::new(),
            Arc::new(AtomicBool::new(false)),
        );

        let outcome = run_plan(&ctx, "auth").await;

        assert!(outcome.ok, "{}", outcome.report);
        assert_eq!(requested_subtasks(&requests), vec!["T1.2"]);
    }

    #[tokio::test]
    async fn parallel_task_runs_subtasks_together_and_joins_before_the_next_task() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(InMemorySddStore::default());
        let mut t1 = task("T1", vec![subtask("T1.1"), subtask("T1.2")]);
        t1.parallel = true;
        store
            .save(&closed_plan(vec![t1, task("T2", vec![subtask("T2.1")])]))
            .unwrap();
        let (provider, requests) = SlowProvider::new();
        let ctx = run_context(
            dir.path(),
            store.clone(),
            provider.clone(),
            ToolRegistry::new(),
            Arc::new(AtomicBool::new(false)),
        );

        let outcome = run_plan(&ctx, "auth").await;

        assert!(outcome.ok, "{}", outcome.report);
        assert!(provider.peak.load(Ordering::SeqCst) >= 2);
        let ids = requested_subtasks(&requests);
        assert_eq!(ids.len(), 3);
        assert_eq!(ids[2], "T2.1", "a T2 só começa depois do join: {ids:?}");
    }

    #[tokio::test]
    async fn overlapping_touches_degrade_the_stage_to_serial_and_notice() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(InMemorySddStore::default());
        let mut left = subtask("T1.1");
        left.touches = vec!["src/a.rs".to_string()];
        let mut right = subtask("T1.2");
        right.touches = vec!["src/a.rs".to_string()];
        let mut t1 = task("T1", vec![left, right]);
        t1.parallel = true;
        store.save(&closed_plan(vec![t1])).unwrap();
        let (provider, _requests) = SlowProvider::new();
        let ctx = run_context(
            dir.path(),
            store,
            provider.clone(),
            ToolRegistry::new(),
            Arc::new(AtomicBool::new(false)),
        );
        let mut rx = ctx.events.subscribe();

        let outcome = run_plan(&ctx, "auth").await;

        assert!(outcome.ok, "{}", outcome.report);
        assert_eq!(provider.peak.load(Ordering::SeqCst), 1);
        let mut noticed = false;
        while let Ok(event) = rx.try_recv() {
            if let Event::Notice { message } = event {
                if message.contains("sobrepostos") {
                    noticed = true;
                }
            }
        }
        assert!(noticed, "esperava o notice de sobreposição");
    }

    #[tokio::test]
    async fn a_parallel_task_with_a_single_subtask_finishes() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(InMemorySddStore::default());
        let mut t1 = task("T1", vec![subtask("T1.1")]);
        t1.parallel = true;
        store.save(&closed_plan(vec![t1])).unwrap();
        let (provider, _requests) = SlowProvider::new();
        let ctx = run_context(
            dir.path(),
            store,
            provider,
            ToolRegistry::new(),
            Arc::new(AtomicBool::new(false)),
        );

        let outcome = tokio::time::timeout(Duration::from_secs(2), run_plan(&ctx, "auth"))
            .await
            .expect("não pode travar");

        assert!(outcome.ok, "{}", outcome.report);
    }

    #[tokio::test]
    async fn acceptance_test_failure_keeps_the_subtask_open_and_stops_the_pipeline() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(InMemorySddStore::default());
        let mut sub = subtask("T1.1");
        sub.acceptance_test = Some(AcceptanceTest {
            package: Some("tupi-app".to_string()),
            filter: "sdd_run".to_string(),
        });
        store
            .save(&closed_plan(vec![
                task("T1", vec![sub]),
                task("T2", vec![subtask("T2.1")]),
            ]))
            .unwrap();
        let (provider, requests) = Scripted::new_tracked(vec![
            vec![text_event("feito T1.1")],
            vec![text_event("feito T2.1")],
        ]);
        let mut tools = ToolRegistry::new();
        tools.register(Arc::new(ExecTool {
            output: String::new(),
            fail: true,
        }));
        let ctx = run_context(
            dir.path(),
            store.clone(),
            provider,
            tools,
            Arc::new(AtomicBool::new(false)),
        );

        let outcome = run_plan(&ctx, "auth").await;

        assert!(!outcome.ok);
        assert!(outcome.report.contains("aceite"));
        assert_eq!(requests.lock().unwrap().len(), 1, "T2 não pode rodar");
        let saved = store.load("repo", "auth").unwrap().unwrap();
        assert_eq!(saved.tasks[0].subtasks[0].status, SddSubtaskStatus::Open);
        assert_eq!(saved.status, SddPlanStatus::Closed);
    }

    #[tokio::test]
    async fn acceptance_test_with_zero_tests_is_inconclusive() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(InMemorySddStore::default());
        let mut sub = subtask("T1.1");
        sub.acceptance_test = Some(AcceptanceTest {
            package: None,
            filter: "nao_existe".to_string(),
        });
        store
            .save(&closed_plan(vec![task("T1", vec![sub])]))
            .unwrap();
        let (provider, _requests) = Scripted::new_tracked(vec![vec![text_event("feito T1.1")]]);
        let mut tools = ToolRegistry::new();
        tools.register(Arc::new(ExecTool {
            output: "running 0 tests\ntest result: ok. 0 passed".to_string(),
            fail: false,
        }));
        let ctx = run_context(
            dir.path(),
            store.clone(),
            provider,
            tools,
            Arc::new(AtomicBool::new(false)),
        );

        let outcome = run_plan(&ctx, "auth").await;

        assert!(!outcome.ok);
        let saved = store.load("repo", "auth").unwrap().unwrap();
        assert_eq!(saved.tasks[0].subtasks[0].status, SddSubtaskStatus::Open);
    }

    #[tokio::test]
    async fn a_failed_agent_stops_before_the_next_task() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(InMemorySddStore::default());
        store
            .save(&closed_plan(vec![
                task("T1", vec![subtask("T1.1"), subtask("T1.2")]),
                task("T2", vec![subtask("T2.1")]),
            ]))
            .unwrap();
        let (provider, requests) = Scripted::new_tracked(vec![
            vec![Err(ProviderError::Network("caiu".into()))],
            vec![text_event("não deveria rodar")],
        ]);
        let ctx = run_context(
            dir.path(),
            store.clone(),
            provider,
            ToolRegistry::new(),
            Arc::new(AtomicBool::new(false)),
        );

        let outcome = run_plan(&ctx, "auth").await;

        assert!(!outcome.ok);
        assert_eq!(requests.lock().unwrap().len(), 1);
        let saved = store.load("repo", "auth").unwrap().unwrap();
        assert_eq!(saved.status, SddPlanStatus::Closed);
    }

    #[tokio::test]
    async fn cancel_stops_the_stage_and_keeps_the_next_task_open() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(InMemorySddStore::default());
        store
            .save(&closed_plan(vec![
                task("T1", vec![subtask("T1.1"), subtask("T1.2")]),
                task("T2", vec![subtask("T2.1")]),
            ]))
            .unwrap();
        let cancel = Arc::new(AtomicBool::new(false));
        let requests = Arc::new(Mutex::new(Vec::new()));
        let provider: Arc<dyn Provider> = Arc::new(CancelProvider {
            cancel: cancel.clone(),
            requests: requests.clone(),
        });
        let ctx = run_context(
            dir.path(),
            store.clone(),
            provider,
            ToolRegistry::new(),
            cancel,
        );

        let outcome = run_plan(&ctx, "auth").await;

        assert!(!outcome.ok);
        assert_eq!(requests.lock().unwrap().len(), 1);
        let saved = store.load("repo", "auth").unwrap().unwrap();
        assert_eq!(saved.tasks[0].subtasks[1].status, SddSubtaskStatus::Open);
        assert_eq!(saved.tasks[1].status, SddTaskStatus::Open);
    }
}
