//! Agent turn runner and SDD plan orchestrator.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use futures_util::StreamExt;
use tupi_domain::{Event, Message, Role, ToolCall};
use tupi_kernel::{ChatRequest, EventBus, Provider, ProviderEvent, ToolRegistry};
use tupi_tools::sdd::{
    AcceptanceTest, SddPlan, SddPlanStatus, SddStore, SddSubtask, SddSubtaskStatus, SddTask,
    SddTaskStatus,
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
    /// Teto de tokens por resposta. Zero deixa o padrão do provedor.
    pub max_tokens: usize,
    pub timeout: Duration,
    pub cancel: Arc<AtomicBool>,
    pub in_flight: Arc<tokio::sync::Notify>,
    pub in_flight_count: Arc<AtomicUsize>,
    /// Publica o progresso das tools quando a subtarefa roda sob um turno.
    pub progress: Option<AgentProgress<'a>>,
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

pub(crate) struct AgentProgress<'a> {
    pub events: &'a EventBus,
    pub request_id: u64,
    pub label: &'a str,
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
    // Mesmo freio do turno principal: avisa a partir dos 60% e, no último
    // passo, retira as tools para que o modelo seja obrigado a concluir em
    // texto em vez de morrer no limite.
    let brake_from = turn.max_steps.saturating_mul(3) / 5;
    for step in 1..=turn.max_steps {
        if turn.cancel.load(Ordering::SeqCst) {
            return AgentOutcome::failure("cancelado");
        }
        let final_step = step >= turn.max_steps;
        let mut request_messages = messages.clone();
        if step >= brake_from.max(1) {
            let warning = if final_step {
                "Freio de passos: última chamada, sem tools. Responda agora, em texto, \
                 com o que mudou, os arquivos e como verificou. Se não terminou, diga \
                 exatamente o que falta e em qual arquivo parou."
                    .to_string()
            } else {
                format!(
                    "Freio de passos: restam {} de {} chamadas. Pare de explorar e comece \
                     a implementar agora; só chame tool se for indispensável.",
                    turn.max_steps.saturating_sub(step),
                    turn.max_steps
                )
            };
            request_messages.push(Message::new(Role::System, warning));
        }
        let specs = turn.tools.specs();
        tracing::debug!(
            step,
            max_steps = turn.max_steps,
            final_step,
            tools = ?specs.iter().map(|spec| spec.name.as_str()).collect::<Vec<_>>(),
            "passo do agente SDD"
        );
        let request = ChatRequest::new(turn.model.to_string(), request_messages)
            .with_tools(if final_step { Vec::new() } else { specs })
            .with_max_tokens(turn.max_tokens);
        let mut stream = match turn.provider.stream_chat(request).await {
            Ok(stream) => stream,
            Err(error) => return AgentOutcome::failure(error.to_string()),
        };
        let mut text = String::new();
        let mut reasoning = String::new();
        let mut calls = Vec::new();
        let mut finish_reason: Option<String> = None;
        while let Some(item) = stream.next().await {
            // ESC chega com o stream em voo: sem esta checagem o turno só
            // pararia no próximo passo, depois da resposta inteira.
            if turn.cancel.load(Ordering::SeqCst) {
                return AgentOutcome::failure("cancelado");
            }
            match item {
                Ok(ProviderEvent::Text(chunk)) => text.push_str(&chunk),
                Ok(ProviderEvent::Reasoning(chunk)) => reasoning.push_str(&chunk),
                Ok(ProviderEvent::ToolCalls(inner)) => calls.extend(inner),
                Ok(ProviderEvent::Finished(reason)) => finish_reason = Some(reason),
                Ok(_) => {}
                Err(error) => return AgentOutcome::failure(error.to_string()),
            }
        }

        // Resposta cortada no teto de tokens: as calls podem estar incompletas
        // (JSON truncado), então nada desta etapa roda. Guarda o texto, pede a
        // reemissão e gasta o passo; no último passo, falha com a causa real.
        if matches!(
            finish_reason.as_deref(),
            Some("length") | Some("max_tokens")
        ) {
            if !text.is_empty() {
                messages.push(Message::assistant(text).with_reasoning(reasoning));
            }
            if final_step {
                return AgentOutcome::failure(format!(
                    "resposta cortada no limite de {} tokens",
                    turn.max_tokens
                ));
            }
            messages.push(Message::new(
                Role::System,
                format!(
                    "A resposta anterior foi cortada pelo limite de {} tokens antes de terminar. Nada dela foi executado. Reemita a resposta/chamada completa; se o conteúdo for grande, divida em passos menores.",
                    turn.max_tokens
                ),
            ));
            continue;
        }
        if final_step {
            // Sem tools no request, chamadas tardias são ignoradas: o contrato
            // do último passo é responder em texto.
            if calls.is_empty() || !text.trim().is_empty() {
                tracing::debug!(
                    step,
                    text_chars = text.chars().count(),
                    "agente SDD concluiu no freio"
                );
                return AgentOutcome {
                    ok: true,
                    text,
                    error: None,
                };
            }
            tracing::warn!(
                step,
                max_steps = turn.max_steps,
                "agente SDD pediu tool no último passo, sem tools disponíveis"
            );
            return AgentOutcome::failure(format!(
                "o agente parou no limite de {} passos sem resposta final",
                turn.max_steps
            ));
        }
        if calls.is_empty() {
            return AgentOutcome {
                ok: true,
                text,
                error: None,
            };
        }
        messages.push(Message::assistant_with_calls(text, calls.clone()).with_reasoning(reasoning));
        for (index, call) in calls.into_iter().enumerate() {
            if turn.cancel.load(Ordering::SeqCst) {
                return AgentOutcome::failure("cancelado");
            }
            // Cada tool do subagente vira uma linha própria na TUI, aninhada
            // sob a subtarefa: `sdd#T1.1#<passo>.<índice>`.
            let call_id = turn
                .progress
                .as_ref()
                .map(|progress| format!("sdd#{}#{step}.{index}", progress.label));
            if let (Some(progress), Some(call_id)) = (turn.progress.as_ref(), call_id.as_ref()) {
                progress.events.publish(Event::ToolStarted {
                    request_id: progress.request_id,
                    call_id: call_id.clone(),
                    name: call.name.clone(),
                    arguments: call.arguments.clone(),
                    summary: crate::app::describe_call(turn.tools, &call),
                });
            }
            let started = Instant::now();
            let result = run_tool(
                turn.tools,
                &call,
                &turn.cancel,
                &turn.in_flight,
                &turn.in_flight_count,
            )
            .await;
            let elapsed = started.elapsed();
            let ok = result.is_ok();
            let content = match result {
                Ok(content) => content,
                Err(error) => format!("erro: {error}"),
            };
            if let (Some(progress), Some(call_id)) = (turn.progress.as_ref(), call_id.as_ref()) {
                progress.events.publish(Event::ToolFinished {
                    request_id: progress.request_id,
                    call_id: call_id.clone(),
                    name: call.name.clone(),
                    ok,
                    summary: crate::app::summarize(&content),
                    duration_us: crate::telemetry::micros(elapsed).max(1),
                });
            }
            tracing::debug!(
                step,
                tool = %call.name,
                result_chars = content.chars().count(),
                "tool do agente SDD"
            );
            messages.push(Message::tool(&call.id, content));
        }
    }
    tracing::warn!(
        max_steps = turn.max_steps,
        "agente SDD esgotou o orçamento de passos"
    );
    AgentOutcome::failure(format!("limite de {} passos", turn.max_steps))
}

/// Tudo que o orquestrador precisa para rodar um plano fechado.
#[allow(dead_code)]
pub type DocsAgentFactory =
    Arc<dyn Fn() -> (ToolRegistry, Arc<tupi_tools::DocsTool>) + Send + Sync>;

pub(crate) struct RunContext {
    pub store: Arc<dyn SddStore>,
    pub repo_key: String,
    pub tools: ToolRegistry,
    pub agent_tools: Arc<dyn Fn() -> ToolRegistry + Send + Sync>,
    pub docs_agent: Option<DocsAgentFactory>,
    pub provider: Arc<dyn Provider>,
    pub model: String,
    pub agent_prefix: String,
    pub max_parallel: usize,
    pub agent_max_steps: usize,
    /// Teto de tokens por resposta dos agentes SDD. Zero deixa o padrão.
    pub max_tokens: usize,
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
        Ok(Some(plan)) if matches!(plan.status, SddPlanStatus::Closed | SddPlanStatus::Failed) => {
            plan
        }
        Ok(Some(plan)) if plan.status == SddPlanStatus::Completed => {
            return failure("plano SDD já concluído");
        }
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
        let task_id = plan.tasks[task_index].id.clone();
        plan.tasks[task_index].status = SddTaskStatus::InProgress;
        {
            let activity = plan.record_activity("runner", "task_started", "info");
            activity.task = Some(task_id.clone());
        }
        if !save(ctx, &plan) {
            return failure("falha ao salvar plano SDD");
        }

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

        for sub_id in &subtask_ids {
            let activity = plan.record_activity("runner", "subtask_started", "info");
            activity.task = Some(task_id.clone());
            activity.subtask = Some(sub_id.clone());
        }
        if !save(ctx, &plan) {
            return failure("falha ao salvar plano SDD");
        }

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
        plan.record_activity("runner", "plan_completed", "ok");
        if ctx.docs_agent.is_some() {
            plan.docs_feedback = Some(tupi_tools::sdd::DocsFeedback {
                status: tupi_tools::sdd::DocsFeedbackStatus::Pending,
                files: Vec::new(),
                reason: None,
                reindexed: false,
            });
            save(ctx, &plan);
            let feedback = run_docs_feedback(ctx, &mut plan).await;
            plan.docs_feedback = Some(feedback);
            if plan.conclusion.is_none() {
                plan.conclusion = Some(format_run_report(&plan, &lines));
            }
        } else {
            plan.docs_feedback = Some(tupi_tools::sdd::DocsFeedback {
                status: tupi_tools::sdd::DocsFeedbackStatus::Skipped,
                files: Vec::new(),
                reason: Some("docs desabilitado".to_string()),
                reindexed: false,
            });
            plan.conclusion = Some(format_run_report(&plan, &lines));
        }
        save(ctx, &plan);
    } else if !ok && ctx.cancel.load(Ordering::SeqCst) {
        // Cancelar não é falhar: o plano fica no mesmo estado em que estava
        // quando o usuário interrompeu, pronto para ser retomado.
        plan.record_activity("runner", "plan_cancelled", "info");
        plan.conclusion = Some("cancelado pelo usuário".to_string());
        save(ctx, &plan);
        return SddRunOutcome {
            ok: false,
            report: "Execução SDD cancelada pelo usuário".to_string(),
        };
    } else if !ok {
        plan.status = SddPlanStatus::Failed;
        plan.record_activity("runner", "plan_failed", "failed");
        plan.docs_feedback = Some(tupi_tools::sdd::DocsFeedback {
            status: tupi_tools::sdd::DocsFeedbackStatus::Skipped,
            files: Vec::new(),
            reason: Some("plano falhou; docs não realimentados".to_string()),
            reindexed: false,
        });
        plan.conclusion = Some(format_run_report(&plan, &lines));
        save(ctx, &plan);
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

async fn run_docs_feedback(ctx: &RunContext, plan: &mut SddPlan) -> tupi_tools::sdd::DocsFeedback {
    let Some(factory) = &ctx.docs_agent else {
        let activity = plan.record_activity("runner", "docs_feedback_skipped", "skipped");
        activity.note = Some("docs desabilitado".to_string());
        return tupi_tools::sdd::DocsFeedback {
            status: tupi_tools::sdd::DocsFeedbackStatus::Skipped,
            files: Vec::new(),
            reason: Some("docs desabilitado".to_string()),
            reindexed: false,
        };
    };

    let _ = plan.record_activity("docs-agent", "docs_feedback_started", "info");
    plan.conclusion = None;
    let (tools, docs) = factory();
    let brief = format_docs_brief(plan);
    let call_id = "sdd#docs".to_string();
    ctx.events.publish(Event::ToolStarted {
        request_id: ctx.request_id,
        call_id: call_id.clone(),
        name: "agent".to_string(),
        arguments: "{}".to_string(),
        summary: "docs — realimentação".to_string(),
    });
    let started = Instant::now();
    let outcome = run_agent(
        AgentTurn {
            provider: &ctx.provider,
            model: &ctx.model,
            prefix: crate::sdd::DOCS_AGENT_PREFIX,
            tools: &tools,
            max_steps: ctx.agent_max_steps,
            max_tokens: ctx.max_tokens,
            timeout: ctx.agent_timeout,
            cancel: ctx.cancel.clone(),
            in_flight: ctx.in_flight.clone(),
            in_flight_count: ctx.in_flight_count.clone(),
            progress: Some(AgentProgress {
                events: &ctx.events,
                request_id: ctx.request_id,
                label: "docs",
            }),
        },
        &brief,
    )
    .await;
    ctx.events.publish(Event::ToolFinished {
        request_id: ctx.request_id,
        call_id,
        name: "agent".to_string(),
        ok: outcome.ok,
        summary: summarize_outcome(&outcome),
        duration_us: crate::telemetry::micros(started.elapsed()).max(1),
    });

    let refresh = docs.refresh();
    let records = docs.written_records();
    let files: Vec<String> = records.iter().map(|record| record.path.clone()).collect();
    let status = if !outcome.ok {
        tupi_tools::sdd::DocsFeedbackStatus::Failed
    } else if files.is_empty() {
        tupi_tools::sdd::DocsFeedbackStatus::Noop
    } else if records.iter().all(|record| record.created) {
        tupi_tools::sdd::DocsFeedbackStatus::Created
    } else {
        tupi_tools::sdd::DocsFeedbackStatus::Updated
    };
    if outcome.ok && !outcome.text.trim().is_empty() {
        plan.conclusion = Some(outcome.text.trim().to_string());
    }

    let write_action = if files.is_empty() {
        "docs_feedback_skipped"
    } else {
        "docs_written"
    };
    let activity = plan.record_activity(
        "docs-agent",
        write_action,
        if outcome.ok { "ok" } else { "failed" },
    );
    activity.files = files.clone();
    activity.note = outcome.error.clone().or_else(|| Some(outcome.text.clone()));

    let reindex = plan.record_activity(
        "runner",
        "docs_reindexed",
        if refresh.error.is_none() {
            "ok"
        } else {
            "failed"
        },
    );
    reindex.note = refresh.error.clone();

    tupi_tools::sdd::DocsFeedback {
        status,
        files,
        reason: outcome.error.or_else(|| Some(outcome.text)),
        reindexed: refresh.error.is_none(),
    }
}

fn format_docs_brief(plan: &SddPlan) -> String {
    format!(
        "Você é o agente de documentação do plano SDD {slug}.\n\
         Leia .tupi/sdd/{slug}/result.md, spec.md, tasks.json e meta.json.\n\
         Use docs map/query/read para ver a documentação atual.\n\
         Se o trabalho mudou conhecimento durável, atualize/crie arquivos sob docs_folders com docs write.\n\
         Se nada mudou, não escreva nada.\n\
         No final, responda uma conclusão curta (até 10 linhas) para result.md.\n\
         Não altere código, testes ou arquivos fora de docs_folders.",
        slug = plan.slug
    )
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
            max_tokens: ctx.max_tokens,
            timeout: ctx.agent_timeout,
            cancel: ctx.cancel.clone(),
            in_flight: ctx.in_flight.clone(),
            in_flight_count: ctx.in_flight_count.clone(),
            progress: Some(AgentProgress {
                events: &ctx.events,
                request_id: ctx.request_id,
                label: sub_id,
            }),
        },
        &brief,
    )
    .await;
    // A duração sai do executor da subtarefa: quem consome o evento não tem
    // como medir isso depois que ele passa pelo bus.
    let elapsed = started.elapsed();
    ctx.events.publish(Event::ToolFinished {
        request_id: ctx.request_id,
        call_id: format!("sdd#{sub_id}"),
        name: "agent".to_string(),
        ok: outcome.ok,
        summary: summarize_outcome(&outcome),
        duration_us: crate::telemetry::micros(elapsed).max(1),
    });
    (outcome, elapsed)
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
    let acceptance = find_acceptance(plan, sub_id);
    let files = find_target(plan, sub_id)
        .and_then(|(_, sub)| sub.map(|sub| sub.touches.clone()))
        .unwrap_or_default();
    let mut ok = outcome.ok;
    let mut acceptance_error = None;

    if ok {
        if let Some(test) = &acceptance {
            if let Err(error) = run_acceptance(ctx, test, sub_id).await {
                ok = false;
                acceptance_error = Some(error);
            }
        }
    }

    if acceptance.is_some() {
        let (action, status, note) = match &acceptance_error {
            Some(error) => (
                "acceptance_failed",
                "failed",
                format!("aceite falhou: {error}"),
            ),
            None => ("acceptance_succeeded", "ok", "aceite passou".to_string()),
        };
        let activity = plan.record_activity("runner", action, status);
        activity.task = Some(task_id.to_string());
        activity.subtask = Some(sub_id.to_string());
        activity.note = Some(note);
    }

    if ok {
        mark_done(plan, task_id, sub_id);
        lines.push(format!(
            "{sub_id} ok {:.1}s — {summary}",
            elapsed.as_secs_f32()
        ));
        let activity = plan.record_activity("runner", "subtask_succeeded", "ok");
        activity.task = Some(task_id.to_string());
        activity.subtask = Some(sub_id.to_string());
        activity.duration_ms = Some(elapsed.as_millis() as u64);
        activity.files = files;
        activity.note = Some(summary);
    } else {
        let reason = match acceptance_error {
            Some(error) => format!("aceite falhou: {error}"),
            None => outcome
                .error
                .clone()
                .unwrap_or_else(|| "sem detalhe".to_string()),
        };
        lines.push(format!("{sub_id} falhou: {reason}"));
        let activity = plan.record_activity("runner", "subtask_failed", "failed");
        activity.task = Some(task_id.to_string());
        activity.subtask = Some(sub_id.to_string());
        activity.duration_ms = Some(elapsed.as_millis() as u64);
        activity.files = files;
        activity.note = Some(reason);
    }

    sync_task(plan, task_id);
    if !save(ctx, plan) {
        return false;
    }
    ok
}

async fn run_acceptance(
    ctx: &RunContext,
    test: &AcceptanceTest,
    sub_id: &str,
) -> Result<(), String> {
    if !ctx.tools.has("exec") {
        return Err("tool exec não registrada para o aceite".to_string());
    }
    let mut arguments = serde_json::json!({ "kind": "test", "filter": test.filter });
    if let Some(package) = &test.package {
        arguments["package"] = serde_json::json!(package);
    }
    let call_id = format!("sdd-aceite-{sub_id}");
    ctx.events.publish(Event::ToolStarted {
        request_id: ctx.request_id,
        call_id: call_id.clone(),
        name: "exec".to_string(),
        arguments: arguments.to_string(),
        summary: format!("aceite {sub_id} · {}", test.filter),
    });
    let call = ToolCall {
        id: format!("sdd-aceite-{}", test.filter),
        name: "exec".to_string(),
        arguments: arguments.to_string(),
    };
    let started = Instant::now();
    let result = run_tool(
        &ctx.tools,
        &call,
        &ctx.cancel,
        &ctx.in_flight,
        &ctx.in_flight_count,
    )
    .await;
    let (ok, detail) = match &result {
        Ok(output) if output.contains(" 0 passed") => {
            (false, "nenhum teste casou o filtro".to_string())
        }
        Ok(output) => (true, crate::app::summarize(output)),
        Err(error) => (false, error.clone()),
    };
    ctx.events.publish(Event::ToolFinished {
        request_id: ctx.request_id,
        call_id,
        name: "exec".to_string(),
        ok,
        summary: detail,
        duration_us: crate::telemetry::micros(started.elapsed()).max(1),
    });
    let output = result?;
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

    /// Modelo que só para de chamar tool quando o request deixa de oferecer
    /// tools — o comportamento que o freio de passos precisa domar.
    struct StubbornProvider {
        requests: Arc<Mutex<Vec<ChatRequest>>>,
    }

    impl StubbornProvider {
        fn new() -> (Arc<Self>, Arc<Mutex<Vec<ChatRequest>>>) {
            let requests = Arc::new(Mutex::new(Vec::new()));
            (
                Arc::new(Self {
                    requests: requests.clone(),
                }),
                requests,
            )
        }
    }

    #[async_trait]
    impl Provider for StubbornProvider {
        fn id(&self) -> &str {
            "stubborn"
        }

        async fn list_models(&self) -> Result<Vec<String>, ProviderError> {
            Ok(Vec::new())
        }

        async fn stream_chat(&self, request: ChatRequest) -> Result<ProviderStream, ProviderError> {
            self.requests.lock().unwrap().push(request.clone());
            let events = if request.tools.is_empty() {
                vec![text_event("concluído no último passo")]
            } else {
                vec![call_event("c1")]
            };
            Ok(Box::pin(stream::iter(events)))
        }
    }

    fn docs_factory(root: &std::path::Path) -> DocsAgentFactory {
        let root = root.to_path_buf();
        Arc::new(move || {
            let mut registry = ToolRegistry::new();
            let docs = tupi_tools::register_docs_agent(
                root.clone(),
                &mut registry,
                tupi_tools::DocsAgentOptions {
                    docs_folders: vec![root.join("docs")],
                    embedder: None,
                    generator: None,
                },
            );
            (registry, docs)
        })
    }

    fn docs_write_event(path: &str, content: &str) -> Result<ProviderEvent, ProviderError> {
        Ok(ProviderEvent::ToolCalls(vec![ToolCall {
            id: "docs-write".to_string(),
            name: "docs".to_string(),
            arguments: serde_json::json!({
                "action": "write",
                "path": path,
                "content": content
            })
            .to_string(),
        }]))
    }

    #[tokio::test]
    async fn completed_plan_runs_docs_agent_and_records_feedback() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("docs")).unwrap();
        let store = Arc::new(tupi_tools::sdd::FsSddStore::new(dir.path()));
        store
            .save(&closed_plan(vec![task("T1", vec![subtask("T1.1")])]))
            .unwrap();
        let (provider, _requests) = Scripted::new_tracked(vec![
            vec![text_event("feito T1.1")],
            vec![docs_write_event("docs/guia.md", "# Guia\n")],
            vec![text_event("documentei o fluxo")],
        ]);
        let mut ctx = run_context(
            store.clone(),
            provider,
            ToolRegistry::new(),
            Arc::new(AtomicBool::new(false)),
        );
        ctx.docs_agent = Some(docs_factory(dir.path()));

        let outcome = run_plan(&ctx, "auth").await;

        assert!(outcome.ok, "{}", outcome.report);
        let saved = store.load("repo", "auth").unwrap().unwrap();
        assert_eq!(
            saved.docs_feedback.unwrap().status,
            tupi_tools::sdd::DocsFeedbackStatus::Created
        );
        assert!(dir.path().join("docs/guia.md").exists());
        assert!(
            saved
                .activities
                .iter()
                .any(|activity| activity.action == "docs_written")
        );
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
            max_tokens: 0,
            timeout: Duration::from_secs(5),
            cancel,
            in_flight: Arc::new(tokio::sync::Notify::new()),
            in_flight_count: Arc::new(AtomicUsize::new(0)),
            progress: None,
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
    async fn subagent_tool_calls_publish_progress_events_with_the_turn_id() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(tupi_tools::sdd::FsSddStore::new(dir.path()));
        store
            .save(&closed_plan(vec![task("T1", vec![subtask("T1.1")])]))
            .unwrap();
        let (provider, _requests) =
            Scripted::new_tracked(vec![vec![call_event("c1")], vec![text_event("feito T1.1")]]);
        let hits = Arc::new(AtomicUsize::new(0));
        let agent_hits = hits.clone();
        let mut ctx = run_context(
            store,
            provider,
            ToolRegistry::new(),
            Arc::new(AtomicBool::new(false)),
        );
        // O subagente roda com o registry da fábrica, não com `ctx.tools`.
        ctx.agent_tools = Arc::new(move || {
            let mut registry = ToolRegistry::new();
            registry.register(Arc::new(EchoTool {
                hits: agent_hits.clone(),
            }));
            registry
        });
        let mut rx = ctx.events.subscribe();

        let outcome = run_plan(&ctx, "auth").await;

        assert!(outcome.ok, "{}", outcome.report);
        let mut started = Vec::new();
        let mut finished = Vec::new();
        while let Ok(event) = rx.try_recv() {
            match event {
                Event::ToolStarted {
                    request_id,
                    call_id,
                    name,
                    ..
                } => {
                    assert_eq!(request_id, 1, "evento fora do turno");
                    started.push((call_id, name));
                }
                Event::ToolFinished {
                    request_id,
                    call_id,
                    ok,
                    ..
                } => {
                    assert_eq!(request_id, 1, "evento fora do turno");
                    finished.push((call_id, ok));
                }
                _ => {}
            }
        }

        assert!(
            started.contains(&("sdd#T1.1".to_string(), "agent".to_string())),
            "{started:?}"
        );
        assert!(
            started.contains(&("sdd#T1.1#1.0".to_string(), "echo".to_string())),
            "{started:?}"
        );
        assert!(
            finished.iter().any(|(id, ok)| id == "sdd#T1.1#1.0" && *ok),
            "{finished:?}"
        );
    }

    #[tokio::test]
    async fn acceptance_publishes_an_exec_line() {
        let store = Arc::new(InMemorySddStore::default());
        let mut sub = subtask("T1.1");
        sub.acceptance_test = Some(AcceptanceTest {
            package: None,
            filter: "space_search".to_string(),
        });
        store
            .save(&closed_plan(vec![task("T1", vec![sub])]))
            .unwrap();
        let (provider, _requests) = Scripted::new_tracked(vec![vec![text_event("feito T1.1")]]);
        let mut tools = ToolRegistry::new();
        tools.register(Arc::new(ExecTool {
            output: "running 2 tests\ntest result: ok. 2 passed".to_string(),
            fail: false,
        }));
        let ctx = run_context(store, provider, tools, Arc::new(AtomicBool::new(false)));
        let mut rx = ctx.events.subscribe();

        let outcome = run_plan(&ctx, "auth").await;

        assert!(outcome.ok, "{}", outcome.report);
        let mut lines = Vec::new();
        while let Ok(event) = rx.try_recv() {
            if let Event::ToolStarted {
                call_id,
                name,
                summary,
                ..
            } = event
            {
                lines.push((call_id, name, summary));
            }
        }
        assert!(
            lines.iter().any(|(call_id, name, summary)| {
                call_id == "sdd-aceite-T1.1" && name == "exec" && summary.contains("space_search")
            }),
            "{lines:?}"
        );
    }

    #[tokio::test]
    async fn docs_feedback_publishes_an_agent_line() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("docs")).unwrap();
        let store = Arc::new(tupi_tools::sdd::FsSddStore::new(dir.path()));
        store
            .save(&closed_plan(vec![task("T1", vec![subtask("T1.1")])]))
            .unwrap();
        let (provider, _requests) = Scripted::new_tracked(vec![
            vec![text_event("feito T1.1")],
            vec![docs_write_event("docs/guia.md", "# Guia\n")],
            vec![text_event("documentei")],
        ]);
        let mut ctx = run_context(
            store,
            provider,
            ToolRegistry::new(),
            Arc::new(AtomicBool::new(false)),
        );
        ctx.docs_agent = Some(docs_factory(dir.path()));
        let mut rx = ctx.events.subscribe();

        let outcome = run_plan(&ctx, "auth").await;

        assert!(outcome.ok, "{}", outcome.report);
        let mut started = Vec::new();
        let mut finished = Vec::new();
        while let Ok(event) = rx.try_recv() {
            match event {
                Event::ToolStarted { call_id, name, .. } => started.push((call_id, name)),
                Event::ToolFinished { call_id, ok, .. } => finished.push((call_id, ok)),
                _ => {}
            }
        }
        assert!(
            started.contains(&("sdd#docs".to_string(), "agent".to_string())),
            "{started:?}"
        );
        assert!(
            finished.iter().any(|(id, ok)| id == "sdd#docs" && *ok),
            "{finished:?}"
        );
    }

    #[tokio::test]
    async fn agent_forced_to_answer_when_the_budget_ends() {
        let (provider, requests) = StubbornProvider::new();
        let provider: Arc<dyn Provider> = provider;
        let hits = Arc::new(AtomicUsize::new(0));
        let mut tools = ToolRegistry::new();
        tools.register(Arc::new(EchoTool { hits: hits.clone() }));
        let cancel = Arc::new(AtomicBool::new(false));

        let outcome = run_agent(fixture(&provider, &tools, 3, cancel), "brief").await;

        assert!(outcome.ok, "{:?}", outcome.error);
        assert_eq!(outcome.text, "concluído no último passo");
        assert_eq!(
            hits.load(Ordering::SeqCst),
            2,
            "as tools só rodam antes do último passo"
        );
        let requests = requests.lock().unwrap();
        assert_eq!(requests.len(), 3);
        assert!(
            !requests[0].tools.is_empty(),
            "o primeiro passo ainda oferece tools"
        );
        assert!(
            requests[2].tools.is_empty(),
            "o último passo precisa forçar a resposta em texto"
        );
    }

    #[tokio::test]
    async fn agent_receives_the_step_brake_before_the_budget_runs_out() {
        let (provider, requests) = StubbornProvider::new();
        let provider: Arc<dyn Provider> = provider;
        let mut tools = ToolRegistry::new();
        tools.register(Arc::new(EchoTool {
            hits: Arc::new(AtomicUsize::new(0)),
        }));
        let cancel = Arc::new(AtomicBool::new(false));

        let outcome = run_agent(fixture(&provider, &tools, 5, cancel), "brief").await;

        assert!(outcome.ok, "{:?}", outcome.error);
        let requests = requests.lock().unwrap();
        let brake = |index: usize| {
            requests[index].messages.iter().any(|message| {
                message.role == Role::System && message.content.contains("Freio de passos")
            })
        };
        assert!(!brake(1), "sem freio antes dos 60% do orçamento");
        assert!(brake(2), "o freio entra a partir dos 60% do orçamento");
        assert!(brake(4), "o último passo também leva o freio");
        assert!(
            requests[4]
                .messages
                .last()
                .is_some_and(|message| message.content.contains("última chamada")),
            "o freio final manda responder em texto"
        );
    }

    #[tokio::test]
    async fn agent_discards_tool_calls_cut_by_the_token_limit() {
        let provider = Scripted::new(vec![
            vec![
                Ok(ProviderEvent::Text("vou editar".to_string())),
                Ok(ProviderEvent::ToolCalls(vec![
                    ToolCall {
                        id: "c1".to_string(),
                        name: "echo".to_string(),
                        arguments: "{}".to_string(),
                    },
                    ToolCall {
                        id: "c2".to_string(),
                        name: "echo".to_string(),
                        arguments: "{\"path\"".to_string(),
                    },
                ])),
                Ok(ProviderEvent::Finished("length".to_string())),
            ],
            vec![
                Ok(ProviderEvent::Text("pronto".to_string())),
                Ok(ProviderEvent::Finished("stop".to_string())),
            ],
        ]);
        let hits = Arc::new(AtomicUsize::new(0));
        let mut tools = ToolRegistry::new();
        tools.register(Arc::new(EchoTool {
            hits: Arc::clone(&hits),
        }));
        let cancel = Arc::new(AtomicBool::new(false));

        let outcome = run_agent(fixture(&provider, &tools, 4, cancel), "brief").await;

        assert!(outcome.ok, "{:?}", outcome.error);
        assert_eq!(
            hits.load(Ordering::SeqCst),
            0,
            "tool de uma resposta truncada não pode rodar"
        );
        assert_eq!(outcome.text, "pronto");
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
    async fn agent_passes_back_reasoning_content_of_previous_steps() {
        let (provider, requests) = Scripted::new_tracked(vec![
            vec![
                Ok(ProviderEvent::Reasoning("penso no passo".to_string())),
                call_event("c1"),
            ],
            vec![text_event("feito")],
        ]);
        let mut tools = ToolRegistry::new();
        tools.register(Arc::new(EchoTool {
            hits: Arc::new(AtomicUsize::new(0)),
        }));
        let cancel = Arc::new(AtomicBool::new(false));

        let outcome = run_agent(fixture(&provider, &tools, 4, cancel), "brief").await;

        assert!(outcome.ok, "{:?}", outcome.error);
        let requests = requests.lock().unwrap();
        let assistant = requests[1]
            .messages
            .iter()
            .find(|message| message.role == Role::Assistant)
            .expect("mensagem do assistente no segundo passo");
        assert_eq!(
            assistant.reasoning_content.as_deref(),
            Some("penso no passo"),
            "o thinking mode exige o reasoning_content de volta quando há tools"
        );
    }

    #[tokio::test]
    async fn agent_keeps_reasoning_content_after_a_truncated_answer() {
        let (provider, requests) = Scripted::new_tracked(vec![
            vec![
                Ok(ProviderEvent::Reasoning("cortado".to_string())),
                Ok(ProviderEvent::Text("vou editar".to_string())),
                Ok(ProviderEvent::ToolCalls(vec![ToolCall {
                    id: "c1".to_string(),
                    name: "echo".to_string(),
                    arguments: "{\"path\"".to_string(),
                }])),
                Ok(ProviderEvent::Finished("length".to_string())),
            ],
            vec![text_event("pronto")],
        ]);
        let mut tools = ToolRegistry::new();
        tools.register(Arc::new(EchoTool {
            hits: Arc::new(AtomicUsize::new(0)),
        }));
        let cancel = Arc::new(AtomicBool::new(false));

        let outcome = run_agent(fixture(&provider, &tools, 4, cancel), "brief").await;

        assert!(outcome.ok, "{:?}", outcome.error);
        let requests = requests.lock().unwrap();
        let assistant = requests[1]
            .messages
            .iter()
            .find(|message| message.role == Role::Assistant)
            .expect("mensagem do assistente após o corte");
        assert_eq!(assistant.reasoning_content.as_deref(), Some("cortado"));
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
            ..Default::default()
        }
    }

    fn run_context(
        store: Arc<dyn SddStore>,
        provider: Arc<dyn Provider>,
        tools: ToolRegistry,
        cancel: Arc<AtomicBool>,
    ) -> RunContext {
        RunContext {
            store,
            repo_key: "repo".to_string(),
            tools,
            agent_tools: Arc::new(|| ToolRegistry::new()),
            docs_agent: None,
            provider,
            model: "m".to_string(),
            agent_prefix: "sys".to_string(),
            max_parallel: 4,
            agent_max_steps: 4,
            max_tokens: 0,
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
        let store = Arc::new(tupi_tools::sdd::FsSddStore::new(dir.path()));
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
        let result = std::fs::read_to_string(dir.path().join(".tupi/sdd/auth/result.md")).unwrap();
        assert!(result.contains("## Conclusão"), "{result}");
    }

    #[tokio::test]
    async fn a_task_without_subtasks_runs_one_agent_and_closes_the_task() {
        let store = Arc::new(InMemorySddStore::default());
        store.save(&closed_plan(vec![task("T1", vec![])])).unwrap();
        let (provider, requests) = Scripted::new_tracked(vec![vec![text_event("feito T1")]]);
        let ctx = run_context(
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
        let store = Arc::new(InMemorySddStore::default());
        let mut first = subtask("T1.1");
        first.status = SddSubtaskStatus::Done;
        store
            .save(&closed_plan(vec![task("T1", vec![first, subtask("T1.2")])]))
            .unwrap();
        let (provider, requests) = Scripted::new_tracked(vec![vec![text_event("feito T1.2")]]);
        let ctx = run_context(
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
        let store = Arc::new(InMemorySddStore::default());
        let mut t1 = task("T1", vec![subtask("T1.1"), subtask("T1.2")]);
        t1.parallel = true;
        store
            .save(&closed_plan(vec![t1, task("T2", vec![subtask("T2.1")])]))
            .unwrap();
        let (provider, requests) = SlowProvider::new();
        let ctx = run_context(
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
        let store = Arc::new(InMemorySddStore::default());
        let mut t1 = task("T1", vec![subtask("T1.1")]);
        t1.parallel = true;
        store.save(&closed_plan(vec![t1])).unwrap();
        let (provider, _requests) = SlowProvider::new();
        let ctx = run_context(
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
        assert_eq!(saved.status, SddPlanStatus::Failed);
        assert!(
            saved
                .activities
                .iter()
                .any(|activity| activity.action == "acceptance_failed")
        );
    }

    #[tokio::test]
    async fn acceptance_test_with_zero_tests_is_inconclusive() {
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
            store.clone(),
            provider,
            ToolRegistry::new(),
            Arc::new(AtomicBool::new(false)),
        );

        let outcome = run_plan(&ctx, "auth").await;

        assert!(!outcome.ok);
        assert_eq!(requests.lock().unwrap().len(), 1);
        let saved = store.load("repo", "auth").unwrap().unwrap();
        assert_eq!(saved.status, SddPlanStatus::Failed);
    }

    #[tokio::test]
    async fn cancel_stops_the_stage_and_keeps_the_next_task_open() {
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
        let ctx = run_context(store.clone(), provider, ToolRegistry::new(), cancel);

        let outcome = run_plan(&ctx, "auth").await;

        assert!(!outcome.ok);
        assert_eq!(requests.lock().unwrap().len(), 1);
        let saved = store.load("repo", "auth").unwrap().unwrap();
        assert_eq!(saved.tasks[0].subtasks[1].status, SddSubtaskStatus::Open);
        assert_eq!(saved.tasks[1].status, SddTaskStatus::Open);
    }
}
