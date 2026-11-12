use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use futures_util::StreamExt;
use tupi_domain::{Event, Message, RequestId, Role, Session, ToolCall};
use tupi_kernel::{
    ChatRequest, Command, CommandError, CommandHandler, CommandOutcome, EventBus, ProviderEvent,
    ProviderRegistry, Query, QueryError, QueryHandler, QueryResult, ToolRegistry, WorkflowIndex,
};
use tupi_store::{MODELS_TTL_SECS, Store};

use crate::store::SessionStore;

const SESSION_ID: u64 = 1;

pub const MAX_TOOL_STEPS: usize = 30;

pub(crate) async fn run_tool(
    tools: &ToolRegistry,
    call: &ToolCall,
    cancel_flag: &Arc<AtomicBool>,
    in_flight: &Arc<tokio::sync::Notify>,
    in_flight_count: &Arc<AtomicUsize>,
) -> Result<String, String> {
    let arguments = serde_json::from_str::<serde_json::Value>(&call.arguments)
        .map_err(|error| format!("argumento inválido: {error}"))?;
    let tool = tools
        .get(&call.name)
        .ok_or_else(|| format!("tool desconhecida: {}", call.name))?;

    let flag = Arc::clone(cancel_flag);
    let notify = Arc::clone(in_flight);
    let count = Arc::clone(in_flight_count);
    count.fetch_add(1, Ordering::SeqCst);

    struct InFlightGuard {
        count: Arc<AtomicUsize>,
        notify: Arc<tokio::sync::Notify>,
    }
    impl Drop for InFlightGuard {
        fn drop(&mut self) {
            if self.count.fetch_sub(1, Ordering::SeqCst) == 1 {
                self.notify.notify_waiters();
            }
        }
    }

    tokio::task::spawn_blocking(move || {
        let _guard = InFlightGuard { count, notify };
        tupi_kernel::with_tool_cancel(flag, || tool.call(arguments))
    })
    .await
    .map_err(|error| format!("falha ao executar tool: {error}"))?
    .map_err(|error| error.to_string())
}

pub(crate) fn describe_call(tools: &ToolRegistry, call: &ToolCall) -> String {
    serde_json::from_str::<serde_json::Value>(&call.arguments)
        .ok()
        .and_then(|arguments| {
            tools
                .get(&call.name)
                .map(|tool| tool.describe_call(&arguments))
        })
        .unwrap_or_else(|| call.arguments.clone())
}

fn close_unanswered(session: &SessionStore, pending: &Mutex<Vec<ToolCall>>, reason: &str) {
    let calls = std::mem::take(&mut *pending.lock().unwrap());
    for call in calls {
        session.push_message(Message::tool(call.id, reason));
    }
}

pub(crate) fn summarize(text: &str) -> String {
    let line = text.lines().next().unwrap_or("").trim();
    let mut summary: String = line.chars().take(80).collect();
    if line.chars().count() > 80 {
        summary.push('…');
    }
    summary
}

/// Soma tokens reportados pelo provedor preservando `None` como "não
/// reportado": o primeiro valor visto abre o total, os seguintes acumulam.
fn add_tokens(total: Option<u64>, value: Option<u64>) -> Option<u64> {
    match (total, value) {
        (Some(total), Some(value)) => Some(total + value),
        (None, value) => value,
        (total, None) => total,
    }
}

/// Escala entre o que o provider cobra e o que o app estima, em partes por
/// milhão: `SCALE_ONE` significa estimativa exata. O template do provedor
/// cobra tokens que o histórico não mostra (envelope de tool call, envelope de
/// mensagem), então o único número confiável é o `prompt_tokens` devolvido por
/// ele. A escala usa esse número como verdade e encolhe o orçamento de
/// planejamento quando o app subestima — sem constante de calibração chutada.
const SCALE_ONE: u64 = 1_000_000;
const SCALE_MIN: u64 = 500_000;
const SCALE_MAX: u64 = 4_000_000;

fn observed_scale(estimated: usize, charged: u64) -> Option<u64> {
    if estimated == 0 || charged == 0 {
        return None;
    }
    let scale = (charged as u128 * SCALE_ONE as u128 / estimated as u128) as u64;
    Some(scale.clamp(SCALE_MIN, SCALE_MAX))
}

fn next_scale(current: u64, observed: Option<u64>) -> u64 {
    match observed {
        Some(observed) => (current + observed) / 2,
        None => current,
    }
}

fn scaled_budget(budget: usize, scale: u64) -> usize {
    ((budget as u128 * SCALE_ONE as u128) / scale.max(1) as u128) as usize
}

fn plan_context(
    messages: &[Message],
    context_budget: usize,
    context_target: usize,
    recent_results: usize,
    tool_tokens: usize,
) -> tupi_domain::context::Plan {
    if context_budget == 0 {
        return tupi_domain::context::plan_amortized_with(messages, 0, 0, recent_results);
    }

    let message_budget = context_budget.saturating_sub(tool_tokens).max(1);
    let message_target = match context_target {
        0 => message_budget.saturating_mul(2) / 5,
        target => {
            ((message_budget as u128 * target.min(context_budget) as u128) / context_budget as u128)
                as usize
        }
    };

    tupi_domain::context::plan_amortized_with(
        messages,
        message_budget,
        message_target,
        recent_results,
    )
}

fn tool_spec_tokens(specs: &[tupi_kernel::ToolSpec]) -> usize {
    let payload = tupi_kernel::tools_payload(specs);
    let json = serde_json::to_string(&payload).unwrap_or_default();
    tupi_domain::tokens::estimate_tokens(&json)
}

struct StreamHandle {
    request_id: RequestId,
    task: tokio::task::JoinHandle<()>,
    partial: Arc<Mutex<String>>,
    cancel_flag: Arc<AtomicBool>,
    in_flight: Arc<tokio::sync::Notify>,
    in_flight_count: Arc<AtomicUsize>,
}

pub struct App {
    session: Arc<SessionStore>,
    registry: ProviderRegistry,
    provider_id: String,
    model: Mutex<String>,
    events: EventBus,
    store: Option<Arc<Store>>,
    tools: ToolRegistry,
    pending: Arc<Mutex<Vec<ToolCall>>>,
    next_request: AtomicU64,
    active: Arc<AtomicU64>,
    current: Arc<Mutex<Option<StreamHandle>>>,
    prefix: String,
    show_prompts: AtomicBool,
    max_tool_steps: usize,
    /// Teto de tokens estimados do contexto enviado ao modelo. Zero desliga a
    /// poda e volta ao comportamento antigo (histórico inteiro, sempre).
    context_budget: AtomicUsize,
    /// Alvo da poda amortizada. Zero pede o padrão de 2/5 do teto.
    context_target: AtomicUsize,
    /// Quantos resultados de tool recentes ficam inteiros na poda.
    context_recent_results: AtomicUsize,
    /// Correção do orçamento observada no provider (real/estimado em ppm).
    token_scale: Arc<AtomicU64>,
    /// Workflows que /workflows/<nome> pode invocar. Vazio por padrão para
    /// que os construtores existentes não dependam do disco.
    workflows: Arc<dyn WorkflowIndex>,
    /// Telemetria estruturada. Desligada por padrão; main liga explicitamente.
    telemetry: crate::telemetry::Telemetry,
}

impl App {
    pub fn new(
        registry: ProviderRegistry,
        provider_id: impl Into<String>,
        model: String,
        events: EventBus,
    ) -> Arc<Self> {
        Self::with_store(registry, provider_id, model, events, None)
    }

    pub fn with_store(
        registry: ProviderRegistry,
        provider_id: impl Into<String>,
        model: String,
        events: EventBus,
        store: Option<Arc<Store>>,
    ) -> Arc<Self> {
        Self::with_tools(
            registry,
            provider_id,
            model,
            events,
            store,
            ToolRegistry::new(),
            String::new(),
        )
    }

    pub fn with_tools(
        registry: ProviderRegistry,
        provider_id: impl Into<String>,
        model: String,
        events: EventBus,
        store: Option<Arc<Store>>,
        tools: ToolRegistry,
        prefix: String,
    ) -> Arc<Self> {
        let show_prompts = store
            .as_ref()
            .and_then(
                |store| match store.setting(crate::debug::SHOW_PROMPTS_KEY) {
                    Ok(value) => value,
                    Err(error) => {
                        tracing::warn!(%error, "falha ao ler a configuração de debug");
                        None
                    }
                },
            )
            .is_some_and(|value| crate::debug::enabled(Some(value.as_str())));

        Arc::new(Self {
            session: Arc::new(SessionStore::new(SESSION_ID)),
            registry,
            provider_id: provider_id.into(),
            model: Mutex::new(model),
            events,
            next_request: AtomicU64::new(1),
            active: Arc::new(AtomicU64::new(0)),
            current: Arc::new(Mutex::new(None)),
            store,
            tools,
            pending: Arc::new(Mutex::new(Vec::new())),
            prefix,
            show_prompts: AtomicBool::new(show_prompts),
            max_tool_steps: MAX_TOOL_STEPS,
            context_budget: AtomicUsize::new(tupi_domain::context::DEFAULT_CONTEXT_TOKENS),
            context_target: AtomicUsize::new(0),
            context_recent_results: AtomicUsize::new(
                tupi_domain::context::DEFAULT_RECENT_RESULTS_KEPT,
            ),
            token_scale: Arc::new(AtomicU64::new(SCALE_ONE)),
            workflows: Arc::new(crate::workflow::EmptyIndex),
            telemetry: crate::telemetry::Telemetry::disabled(),
        })
    }

    /// Liga o catálogo de workflows lido do projeto, em `tupi-tools`.
    pub fn with_workflows(mut self: Arc<Self>, workflows: Arc<dyn WorkflowIndex>) -> Arc<Self> {
        Arc::get_mut(&mut self).expect("App recém-criado").workflows = workflows;
        self
    }

    /// `/workflows/<nome> [pedido]`: injeta os passos como turno do usuário.
    ///
    /// Nome desconhecido não vira chamada ao modelo: devolve o erro com a lista
    /// do que existe, que a TUI mostra sem gastar tokens.
    fn run_workflow(&self, name: &str, extra: &str) -> Result<CommandOutcome, CommandError> {
        let name = name.trim();
        if name.is_empty() {
            return Err(CommandError::Internal(
                "uso: /workflows/<nome> [pedido]".to_string(),
            ));
        }
        let Some(workflow) = self.workflows.find(name) else {
            let names: Vec<String> = self
                .workflows
                .list()
                .iter()
                .map(|workflow| workflow.name.clone())
                .collect();
            return Err(CommandError::Internal(if names.is_empty() {
                format!("workflow `{name}` não existe e nenhum está disponível")
            } else {
                format!(
                    "workflow `{name}` não existe. Disponíveis: {}",
                    names.join(", ")
                )
            }));
        };

        self.send_message(workflow.invocation(extra))
    }

    /// Ajusta o limite de passos de tool call por turno.
    pub fn with_max_tool_steps(mut self: Arc<Self>, max_tool_steps: usize) -> Arc<Self> {
        Arc::get_mut(&mut self)
            .expect("App recém-criado")
            .max_tool_steps = max_tool_steps;
        self
    }

    /// Teto de tokens estimados do contexto enviado ao modelo. Zero desliga a
    /// poda e volta a mandar o histórico inteiro.
    pub fn with_context_budget(mut self: Arc<Self>, context_budget: usize) -> Arc<Self> {
        Arc::get_mut(&mut self)
            .expect("App recém-criado")
            .context_budget = AtomicUsize::new(context_budget);
        self
    }

    pub fn with_telemetry(
        mut self: Arc<Self>,
        telemetry: crate::telemetry::Telemetry,
    ) -> Arc<Self> {
        Arc::get_mut(&mut self).expect("App recém-criado").telemetry = telemetry;
        self
    }

    /// Teto alvo depois do corte amortizado. Zero usa 2/5 do teto.
    pub fn with_context_target(mut self: Arc<Self>, context_target: usize) -> Arc<Self> {
        Arc::get_mut(&mut self)
            .expect("App recém-criado")
            .context_target = AtomicUsize::new(context_target);
        self
    }

    /// Quantos resultados de tool recentes a poda mantém inteiros.
    pub fn with_context_recent_results(mut self: Arc<Self>, recent: usize) -> Arc<Self> {
        Arc::get_mut(&mut self)
            .expect("App recém-criado")
            .context_recent_results = AtomicUsize::new(recent);
        self
    }

    pub fn session(&self) -> Session {
        self.session.snapshot()
    }

    fn provider(&self) -> Result<std::sync::Arc<dyn tupi_kernel::Provider>, CommandError> {
        self.registry.get(&self.provider_id).ok_or_else(|| {
            CommandError::Internal(format!("provedor '{}' não registrado", self.provider_id))
        })
    }

    fn set_model(&self, model: String) -> Result<CommandOutcome, CommandError> {
        if self.active.load(Ordering::SeqCst) != 0 {
            return Err(CommandError::Busy);
        }

        *self.model.lock().unwrap() = model.clone();
        Ok(CommandOutcome::ModelChanged { model })
    }

    fn send_message(&self, text: String) -> Result<CommandOutcome, CommandError> {
        if self
            .active
            .compare_exchange(0, u64::MAX, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            return Err(CommandError::Busy);
        }

        let provider = match self.provider() {
            Ok(provider) => provider,
            Err(error) => {
                self.active.store(0, Ordering::SeqCst);
                return Err(error);
            }
        };

        let request_id = self.next_request.fetch_add(1, Ordering::SeqCst);
        self.active.store(request_id, Ordering::SeqCst);

        let model = self.model.lock().unwrap().clone();
        let specs = self.tools.specs();
        let tools = self.tools.clone();
        let prefix = self.prefix.clone();
        let show_prompts = self.show_prompts.load(Ordering::SeqCst);
        let max_tool_steps = self.max_tool_steps;
        let context_budget = self.context_budget.load(Ordering::SeqCst);
        let context_target = self.context_target.load(Ordering::SeqCst);
        let context_recent_results = self.context_recent_results.load(Ordering::SeqCst);
        let token_scale = Arc::clone(&self.token_scale);
        let tool_tokens = tool_spec_tokens(&specs);
        let prefix_tokens = if prefix.is_empty() {
            0
        } else {
            tupi_domain::tokens::estimate_message(&Message::new(Role::System, prefix.clone()))
        };
        let telemetry = self.telemetry.clone();
        let turn_started_at = std::time::Instant::now();

        if telemetry.is_enabled() {
            let history = self.session.messages();
            let estimate = history
                .iter()
                .map(tupi_domain::tokens::estimate_message)
                .sum::<usize>();
            telemetry.record(crate::telemetry::TelemetryEvent::TurnStarted(
                crate::telemetry::TurnStarted {
                    request: request_id,
                    text_chars: text.chars().count(),
                    text: text.clone(),
                    model: model.clone(),
                    history_messages: history.len(),
                    history_estimate: estimate,
                    context_budget,
                },
            ));
        }

        self.session.push(Role::User, text);

        let partial = Arc::new(Mutex::new(String::new()));
        let cancel_flag = Arc::new(AtomicBool::new(false));
        let in_flight = Arc::new(tokio::sync::Notify::new());
        let in_flight_count = Arc::new(AtomicUsize::new(0));
        let events = self.events.clone();
        let session = Arc::clone(&self.session);
        let active = Arc::clone(&self.active);
        let current = Arc::clone(&self.current);
        let pending = Arc::clone(&self.pending);
        let task_partial = Arc::clone(&partial);
        let task_cancel = Arc::clone(&cancel_flag);
        let task_in_flight = Arc::clone(&in_flight);
        let task_in_flight_count = Arc::clone(&in_flight_count);

        let task = tokio::spawn(async move {
            let mut failure = None;
            let mut failure_kind: Option<&'static str> = None;
            let mut failure_http: Option<u16> = None;
            let mut step = 0;
            let mut tool_calls_count = 0usize;
            let mut prompt_tokens = None;
            let mut cached_tokens = None;
            let mut completion_tokens = None;
            let warn_at = max_tool_steps.saturating_mul(3) / 5;

            while step < max_tool_steps {
                step += 1;
                let final_step = step >= max_tool_steps;
                let remaining = max_tool_steps.saturating_sub(step);
                task_partial.lock().unwrap().clear();

                let warning_message = if step >= warn_at.max(1) {
                    let warning = if final_step {
                        "Freio de passos: última chamada, sem tools. Responda agora, em texto, com o que já foi descoberto. Se faltar dado essencial, diga o que falta.".to_string()
                    } else {
                        format!(
                            "Freio de passos: restam {remaining} de {max_tool_steps} chamadas. Pare de explorar e comece a responder. Só chame tool se for indispensável."
                        )
                    };
                    Some(Message::new(Role::System, warning))
                } else {
                    None
                };
                let warning_tokens = warning_message
                    .as_ref()
                    .map(tupi_domain::tokens::estimate_message)
                    .unwrap_or(0);

                // Poda amortizada: corta fundo quando estoura o teto, para que o
                // prefixo cacheado do provedor sobreviva aos próximos passos.
                // O orçamento é encolhido pela escala observada: se o provider
                // cobrou mais do que estimamos, planejamos com menos espaço.
                let request_tool_tokens = if final_step { 0 } else { tool_tokens };
                let fixed_tokens = request_tool_tokens
                    .saturating_add(prefix_tokens)
                    .saturating_add(warning_tokens);
                let scale = token_scale.load(Ordering::SeqCst);
                let planned = plan_context(
                    &session.messages(),
                    scaled_budget(context_budget, scale),
                    scaled_budget(context_target, scale),
                    context_recent_results,
                    fixed_tokens,
                );
                let trimmed = planned.trimmed();
                let omitted_units = planned.omitted_units;
                let shrunk_results = planned.shrunk_results;
                if trimmed {
                    session.replace_messages(planned.messages.clone());
                }

                let mut messages = Vec::new();
                if !prefix.is_empty() {
                    messages.push(Message::new(Role::System, prefix.clone()));
                }
                messages.extend(planned.messages);
                if let Some(warning) = warning_message {
                    messages.push(warning);
                }

                // A estimativa é do que sai no request, não do que o plano
                // pretendia: prefixo, aviso de freio e chamadas entram aqui.
                let prompt_estimate = messages
                    .iter()
                    .map(tupi_domain::tokens::estimate_message)
                    .sum::<usize>()
                    .saturating_add(request_tool_tokens);
                if trimmed {
                    events.publish(Event::ContextTrimmed {
                        request_id,
                        estimate: prompt_estimate,
                        omitted_units,
                        shrunk_results,
                    });
                    if telemetry.is_enabled() {
                        telemetry.record(crate::telemetry::TelemetryEvent::ContextTrimmed(
                            crate::telemetry::ContextTrimmed {
                                request: request_id,
                                step,
                                estimate: prompt_estimate,
                                omitted_units,
                                shrunk_results,
                            },
                        ));
                    }
                }
                let request = ChatRequest::new(model.clone(), messages).with_tools(if final_step {
                    Vec::new()
                } else {
                    specs.clone()
                });

                if show_prompts {
                    events.publish(Event::PromptMessages {
                        request_id,
                        body: crate::debug::render_prompt(&request),
                    });
                }

                if telemetry.is_enabled() {
                    let body = crate::debug::render_prompt(&request);
                    let clipped = crate::telemetry::clip_preview(&body);
                    telemetry.record(crate::telemetry::TelemetryEvent::PromptSent(
                        crate::telemetry::PromptSent {
                            request: request_id,
                            step,
                            model: model.clone(),
                            body: clipped.text,
                            body_hash: crate::telemetry::text_hash(&body),
                            body_chars: clipped.chars,
                            body_bytes: clipped.bytes,
                            truncated: clipped.truncated,
                            estimate_tokens: prompt_estimate,
                            messages: request.messages.len(),
                            tools: request.tools.len(),
                        },
                    ));
                }

                let mut streamed_text = String::new();
                let mut calls = Vec::new();
                let mut stream_failed = false;
                let provider_started = std::time::Instant::now();
                let mut first_token_at: Option<std::time::Instant> = None;
                let mut delta_count = 0usize;
                let mut delta_chars = 0usize;
                let mut step_prompt_tokens = None;
                let mut step_cached_tokens = None;
                let mut step_completion_tokens = None;
                let mut step_tool_names: Vec<String> = Vec::new();

                match provider.stream_chat(request).await {
                    Err(error) => {
                        failure_kind = Some(crate::telemetry::provider_error_kind(&error));
                        failure_http = crate::telemetry::provider_http_status(&error);
                        failure = Some(error.to_string());
                    }
                    Ok(mut stream) => {
                        while let Some(item) = stream.next().await {
                            match item {
                                Ok(ProviderEvent::Text(text)) => {
                                    if telemetry.is_enabled() {
                                        if first_token_at.is_none() {
                                            first_token_at = Some(std::time::Instant::now());
                                        }
                                        delta_count += 1;
                                        delta_chars += text.chars().count();
                                    }
                                    streamed_text.push_str(&text);
                                    task_partial.lock().unwrap().push_str(&text);
                                    events.publish(Event::Token { request_id, text });
                                }
                                Ok(ProviderEvent::ToolCalls(tool_calls)) => {
                                    if telemetry.is_enabled() {
                                        step_tool_names = tool_calls
                                            .iter()
                                            .map(|call| call.name.clone())
                                            .collect();
                                    }
                                    calls = tool_calls;
                                }
                                Ok(ProviderEvent::Usage {
                                    prompt_tokens: prompt,
                                    cached_tokens: cached,
                                    completion_tokens: completion,
                                }) => {
                                    step_prompt_tokens = add_tokens(step_prompt_tokens, prompt);
                                    step_cached_tokens = add_tokens(step_cached_tokens, cached);
                                    step_completion_tokens =
                                        add_tokens(step_completion_tokens, completion);
                                    prompt_tokens = add_tokens(prompt_tokens, prompt);
                                    cached_tokens = add_tokens(cached_tokens, cached);
                                    completion_tokens = add_tokens(completion_tokens, completion);
                                }
                                Err(error) => {
                                    failure_kind =
                                        Some(crate::telemetry::provider_error_kind(&error));
                                    failure_http = crate::telemetry::provider_http_status(&error);
                                    failure = Some(error.to_string());
                                    stream_failed = true;
                                    break;
                                }
                            }
                        }
                    }
                }

                if let Some(charged) = step_prompt_tokens {
                    let observed = observed_scale(prompt_estimate, charged);
                    let current = token_scale.load(Ordering::SeqCst);
                    token_scale.store(next_scale(current, observed), Ordering::SeqCst);
                }

                if telemetry.is_enabled() {
                    telemetry.record(crate::telemetry::TelemetryEvent::ProviderResponse(
                        crate::telemetry::ProviderResponse {
                            request: request_id,
                            step,
                            estimate_tokens: prompt_estimate,
                            scale_ppm: scale,
                            duration_us: crate::telemetry::micros(provider_started.elapsed()),
                            first_token_us: first_token_at.map(|first| {
                                crate::telemetry::micros(first.duration_since(provider_started))
                            }),
                            delta_count,
                            delta_chars,
                            tool_calls: calls.len(),
                            tool_call_names: step_tool_names,
                            prompt_tokens: step_prompt_tokens,
                            cached_tokens: step_cached_tokens,
                            completion_tokens: step_completion_tokens,
                            ok: failure.is_none(),
                            error: failure.clone(),
                            error_kind: failure_kind.map(|kind| kind.to_string()),
                            http_status: failure_http,
                        },
                    ));
                }

                if final_step {
                    if !streamed_text.is_empty() {
                        session.push_message(Message::assistant(streamed_text));
                    }
                } else if !streamed_text.is_empty() || !calls.is_empty() {
                    session
                        .push_message(Message::assistant_with_calls(streamed_text, calls.clone()));
                    *pending.lock().unwrap() = calls.clone();
                }

                if stream_failed || failure.is_some() {
                    close_unanswered(
                        &session,
                        &pending,
                        "turno interrompido: chamada não executada",
                    );
                    break;
                }
                if final_step || calls.is_empty() {
                    break;
                }

                for call in calls {
                    if active.load(Ordering::SeqCst) != request_id {
                        break;
                    }

                    tool_calls_count += 1;
                    let summary = describe_call(&tools, &call);
                    events.publish(Event::ToolStarted {
                        request_id,
                        call_id: call.id.clone(),
                        name: call.name.clone(),
                        arguments: call.arguments.clone(),
                        summary: summary.clone(),
                    });

                    if telemetry.is_enabled() {
                        let arguments = crate::telemetry::clip(&call.arguments);
                        telemetry.record(crate::telemetry::TelemetryEvent::ToolStarted(
                            crate::telemetry::ToolStarted {
                                request: request_id,
                                call: call.id.clone(),
                                name: call.name.clone(),
                                arguments: arguments.text,
                                arguments_chars: arguments.chars,
                                arguments_bytes: arguments.bytes,
                                truncated: arguments.truncated,
                                summary: summary.clone(),
                            },
                        ));
                    }

                    let tool_started = std::time::Instant::now();
                    let (content, ok) = match if call.name == tupi_kernel::pipeline::PIPELINE_TOOL {
                        crate::pipeline::execute(
                            &tools,
                            &events,
                            request_id,
                            &call,
                            &telemetry,
                            &task_cancel,
                            &task_in_flight,
                            &task_in_flight_count,
                        )
                        .await
                    } else {
                        run_tool(
                            &tools,
                            &call,
                            &task_cancel,
                            &task_in_flight,
                            &task_in_flight_count,
                        )
                        .await
                    } {
                        Ok(result) => (result, true),
                        Err(error) => (error, false),
                    };
                    let duration = tool_started.elapsed();
                    let summary = summarize(&content);

                    events.publish(Event::ToolFinished {
                        request_id,
                        call_id: call.id.clone(),
                        name: call.name.clone(),
                        ok,
                        summary: summary.clone(),
                    });

                    if telemetry.is_enabled() {
                        let clipped = crate::telemetry::clip(&content);
                        let (result, error) = if ok {
                            (Some(clipped.text), None)
                        } else {
                            (None, Some(clipped.text))
                        };
                        telemetry.record(crate::telemetry::TelemetryEvent::ToolFinished(
                            crate::telemetry::ToolFinished {
                                request: request_id,
                                call: call.id.clone(),
                                name: call.name.clone(),
                                duration_us: crate::telemetry::micros(duration),
                                ok,
                                result,
                                result_chars: clipped.chars,
                                result_bytes: clipped.bytes,
                                truncated: clipped.truncated,
                                summary,
                                error,
                            },
                        ));
                    }

                    // O resultado e a baixa no pending são atômicos sob o mesmo
                    // lock que o cancel usa: nenhuma call recebe dois resultados.
                    let mut pending_calls = pending.lock().unwrap();
                    session.push_message(Message::tool(call.id.clone(), content));
                    pending_calls.retain(|pending| pending.id != call.id);
                }
                pending.lock().unwrap().clear();
            }

            let mut slot = current.lock().unwrap();
            if slot.as_ref().map(|handle| handle.request_id) == Some(request_id) {
                *slot = None;
            }
            drop(slot);

            let cancelled = active.load(Ordering::SeqCst) != request_id;

            // A TUI pode enfileirar e disparar o próximo turno assim que vê
            // o evento: liberar antes do publish evita um Busy espúrio.
            active.store(0, Ordering::SeqCst);

            match failure.clone() {
                Some(message) => {
                    events.publish(Event::StreamFailed {
                        request_id,
                        message,
                    });
                }
                None => {
                    events.publish(Event::StreamFinished {
                        request_id,
                        prompt_tokens,
                        cached_tokens,
                        completion_tokens,
                    });
                }
            }

            if telemetry.is_enabled() {
                telemetry.record(crate::telemetry::TelemetryEvent::TurnFinished(
                    crate::telemetry::TurnFinished {
                        request: request_id,
                        duration_us: crate::telemetry::micros(turn_started_at.elapsed()),
                        ok: failure.is_none(),
                        cancelled,
                        steps: step,
                        tool_calls: tool_calls_count,
                        prompt_tokens,
                        cached_tokens,
                        completion_tokens,
                        error: failure.clone(),
                        error_kind: failure_kind.map(|kind| kind.to_string()),
                    },
                ));
            }
        });

        *self.current.lock().unwrap() = Some(StreamHandle {
            request_id,
            task,
            partial,
            cancel_flag,
            in_flight,
            in_flight_count,
        });

        Ok(CommandOutcome::Started { request_id })
    }

    async fn cancel(&self, request_id: RequestId) -> CommandOutcome {
        if self
            .active
            .compare_exchange(request_id, 0, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            return CommandOutcome::Noop;
        }

        let handle = self.current.lock().unwrap().take();
        let (task, partial, cancel_flag, in_flight, in_flight_count) = match handle {
            Some(handle) if handle.request_id == request_id => (
                Some(handle.task),
                Some(handle.partial),
                Some(handle.cancel_flag),
                Some(handle.in_flight),
                Some(handle.in_flight_count),
            ),
            _ => (None, None, None, None, None),
        };

        if let Some(flag) = &cancel_flag {
            flag.store(true, Ordering::SeqCst);
        }

        // Espera a task parar de verdade: depois disso ela não escreve mais no
        // histórico, e um novo turno pode começar com o histórico consistente.
        if let Some(task) = task {
            task.abort();
            let _ = task.await;
        }

        if let (Some(notify), Some(count)) = (in_flight, in_flight_count) {
            loop {
                let notified = notify.notified();
                if count.load(Ordering::SeqCst) == 0 {
                    break;
                }
                notified.await;
            }
        }

        let had_pending = !self.pending.lock().unwrap().is_empty();
        close_unanswered(&self.session, &self.pending, "cancelado pelo usuário");

        if !had_pending {
            let partial_text = partial
                .map(|partial| partial.lock().unwrap().clone())
                .unwrap_or_default();
            if !partial_text.is_empty() {
                self.session.push_assistant_once(&partial_text);
            }
        }

        self.events.publish(Event::StreamFinished {
            request_id,
            prompt_tokens: None,
            cached_tokens: None,
            completion_tokens: None,
        });
        CommandOutcome::Cancelled
    }
}

#[async_trait]
impl CommandHandler for App {
    async fn handle(&self, command: Command) -> Result<CommandOutcome, CommandError> {
        let info = self
            .telemetry
            .is_enabled()
            .then(|| crate::telemetry::CommandInfo::new(&command));
        if let Some(info) = &info {
            self.telemetry.record(info.started());
        }

        let started = std::time::Instant::now();
        let result = async {
            match command {
                Command::SendMessage { text } => self.send_message(text),
                Command::RunWorkflow { name, extra } => self.run_workflow(&name, &extra),
                Command::CancelStream { request_id } => Ok(self.cancel(request_id).await),
                Command::SetModel { model } => self.set_model(model),
                Command::NewSession => {
                    let request_id = self.active.swap(0, Ordering::SeqCst);
                    if request_id != 0 {
                        self.cancel(request_id).await;
                    }

                    close_unanswered(&self.session, &self.pending, "nova sessão iniciada");
                    self.session.replace_messages(Vec::new());

                    Ok(CommandOutcome::SessionReset)
                }
                Command::ToggleShowPrompts => {
                    let enabled = !self.show_prompts.load(Ordering::SeqCst);

                    if let Some(store) = self.store.as_ref()
                        && let Err(error) = store
                            .save_setting(crate::debug::SHOW_PROMPTS_KEY, crate::debug::value(enabled))
                    {
                        return Err(CommandError::Internal(format!(
                            "falha ao gravar configuração: {error}"
                        )));
                    }

                    self.show_prompts.store(enabled, Ordering::SeqCst);
                    Ok(CommandOutcome::ShowPrompts { enabled })
                }
                Command::RefreshModels => {
                    // A TUI só reage a eventos: banco e provedor rodam fora do loop.
                    let provider = self.provider()?;
                    let events = self.events.clone();
                    let store = self.store.clone();
                    let provider_id = self.provider_id.clone();
                    let telemetry = self.telemetry.clone();

                    tokio::spawn(async move {
                        let started = std::time::Instant::now();
                        let now_unix = SystemTime::now()
                            .duration_since(UNIX_EPOCH)
                            .map(|elapsed| elapsed.as_secs())
                            .unwrap_or(0);

                        let cached = match store.as_ref() {
                            Some(store) => {
                                let store = Arc::clone(store);
                                let id = provider_id.clone();
                                tokio::task::spawn_blocking(move || store.models(&id))
                                    .await
                                    .ok()
                                    .and_then(Result::ok)
                                    .flatten()
                            }
                            None => None,
                        };

                        if let Some(entry) = cached.as_ref()
                            && entry.is_fresh(now_unix, MODELS_TTL_SECS)
                        {
                            telemetry.record(crate::telemetry::TelemetryEvent::ModelsRefreshed(
                                crate::telemetry::ModelsRefreshed {
                                    source: "cache".to_string(),
                                    count: entry.models.len(),
                                    models: entry.models.clone(),
                                    duration_us: crate::telemetry::micros(started.elapsed()),
                                    ok: true,
                                    error: None,
                                    error_kind: None,
                                    http_status: None,
                                },
                            ));
                            events.publish(Event::ModelsLoaded {
                                models: entry.models.clone(),
                            });
                            return;
                        }

                        match provider.list_models().await {
                            Ok(models) => {
                                telemetry.record(crate::telemetry::TelemetryEvent::ModelsRefreshed(
                                    crate::telemetry::ModelsRefreshed {
                                        source: "provider".to_string(),
                                        count: models.len(),
                                        models: models.clone(),
                                        duration_us: crate::telemetry::micros(started.elapsed()),
                                        ok: true,
                                        error: None,
                                        error_kind: None,
                                        http_status: None,
                                    },
                                ));

                                if let Some(store) = store.as_ref() {
                                    let store = Arc::clone(store);
                                    let id = provider_id.clone();
                                    let to_save = models.clone();

                                    match tokio::task::spawn_blocking(move || {
                                        store.save_models(&id, &to_save, now_unix)
                                    })
                                    .await
                                    {
                                        Ok(Ok(())) => {}
                                        Ok(Err(error)) => {
                                            tracing::warn!(%error, "falha ao gravar catálogo local");
                                        }
                                        Err(error) => {
                                            tracing::warn!(%error, "falha ao agendar gravação");
                                        }
                                    }
                                }

                                events.publish(Event::ModelsLoaded { models });
                            }
                            Err(error) => {
                                let models = cached
                                    .as_ref()
                                    .map(|entry| entry.models.clone())
                                    .unwrap_or_default();
                                telemetry.record(crate::telemetry::TelemetryEvent::ModelsRefreshed(
                                    crate::telemetry::ModelsRefreshed {
                                        source: "provider".to_string(),
                                        count: models.len(),
                                        models,
                                        duration_us: crate::telemetry::micros(started.elapsed()),
                                        ok: false,
                                        error: Some(error.to_string()),
                                        error_kind: Some(
                                            crate::telemetry::provider_error_kind(&error).to_string(),
                                        ),
                                        http_status: crate::telemetry::provider_http_status(&error),
                                    },
                                ));

                                if let Some(entry) = cached {
                                    tracing::warn!(%error, "usando catálogo salvo");
                                    events.publish(Event::ModelsLoaded {
                                        models: entry.models,
                                    });
                                } else {
                                    events.publish(Event::ModelsFailed {
                                        message: error.to_string(),
                                    });
                                }
                            }
                        }
                    });

                    Ok(CommandOutcome::Noop)
                }
            }
        }
        .await;

        if let Some(info) = &info {
            self.telemetry
                .record(info.finished(started.elapsed(), &result));
        }

        result
    }
}

#[async_trait]
impl QueryHandler for App {
    async fn query(&self, query: Query) -> Result<QueryResult, QueryError> {
        let info = self
            .telemetry
            .is_enabled()
            .then(|| crate::telemetry::QueryInfo::new(&query));
        if let Some(info) = &info {
            self.telemetry.record(info.started());
        }

        let started = std::time::Instant::now();
        let result = async {
            match query {
                Query::History => Ok(QueryResult::History(self.session.messages())),
                Query::ListProviders => Ok(QueryResult::Providers(self.registry.ids())),
                Query::ListModels => {
                    let provider = self.registry.get(&self.provider_id).ok_or_else(|| {
                        QueryError::Internal(format!(
                            "provedor '{}' não registrado",
                            self.provider_id
                        ))
                    })?;
                    Ok(QueryResult::Models(provider.list_models().await?))
                }
                Query::ListWorkflows => Ok(QueryResult::Workflows(self.workflows.list())),
            }
        }
        .await;

        if let Some(info) = &info {
            self.telemetry
                .record(info.finished(started.elapsed(), &result));
        }

        result
    }
}

#[cfg(test)]
mod prefix_tests {
    use super::*;

    use async_trait::async_trait;
    use tupi_kernel::{ChatRequest, Provider, ProviderError, ProviderEvent, ProviderStream};

    struct Scripted {
        scripts: Mutex<Vec<Vec<Result<ProviderEvent, ProviderError>>>>,
        requests: Mutex<Vec<ChatRequest>>,
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
            let next = self.scripts.lock().unwrap().remove(0);
            Ok(Box::pin(futures_util::stream::iter(next)))
        }
    }

    fn registry(provider: Arc<dyn Provider>) -> ProviderRegistry {
        let mut registry = ProviderRegistry::new();
        registry.register(provider);
        registry
    }

    fn text(text: &str) -> Vec<Result<ProviderEvent, ProviderError>> {
        vec![Ok(ProviderEvent::Text(text.to_string()))]
    }

    fn app_with_prefix(provider: Arc<dyn Provider>, bus: EventBus, prefix: &str) -> Arc<App> {
        App::with_tools(
            registry(provider),
            "scripted",
            "modelo".into(),
            bus,
            None,
            ToolRegistry::new(),
            prefix.to_string(),
        )
    }

    async fn wait_for_finished(rx: &mut tokio::sync::broadcast::Receiver<Event>) {
        while let Ok(event) = rx.recv().await {
            if matches!(event, Event::StreamFinished { .. }) {
                return;
            }
        }
    }

    #[tokio::test]
    async fn system_prefix_is_the_first_message_of_every_request() {
        let bus = EventBus::new(32);
        let mut rx = bus.subscribe();
        let provider = Arc::new(Scripted {
            scripts: Mutex::new(vec![text("um"), text("dois")]),
            requests: Mutex::new(Vec::new()),
        });
        let app = app_with_prefix(provider.clone(), bus, "PREFIXO");

        app.handle(Command::SendMessage { text: "a".into() })
            .await
            .unwrap();
        wait_for_finished(&mut rx).await;
        app.handle(Command::SendMessage { text: "b".into() })
            .await
            .unwrap();
        wait_for_finished(&mut rx).await;

        let requests = provider.requests.lock().unwrap();
        assert_eq!(requests[0].messages[0].role, Role::System);
        assert_eq!(requests[0].messages[0].content, "PREFIXO");
        assert_eq!(requests[1].messages[0], requests[0].messages[0]);
    }

    #[tokio::test]
    async fn empty_prefix_sends_no_system_message() {
        let bus = EventBus::new(32);
        let mut rx = bus.subscribe();
        let provider = Arc::new(Scripted {
            scripts: Mutex::new(vec![text("ok")]),
            requests: Mutex::new(Vec::new()),
        });
        let app = app_with_prefix(provider.clone(), bus, "");

        app.handle(Command::SendMessage { text: "a".into() })
            .await
            .unwrap();
        wait_for_finished(&mut rx).await;

        let requests = provider.requests.lock().unwrap();
        assert!(
            requests[0]
                .messages
                .iter()
                .all(|message| message.role != Role::System)
        );
    }

    #[tokio::test]
    async fn a_provider_that_charges_more_shrinks_the_next_budget() {
        use tupi_kernel::{Tool, ToolError, ToolSpec};

        struct BigEcho;

        impl Tool for BigEcho {
            fn spec(&self) -> ToolSpec {
                ToolSpec {
                    name: "big".to_string(),
                    description: "devolve muito texto".to_string(),
                    schema: serde_json::json!({"type": "object"}),
                }
            }

            fn call(&self, _arguments: serde_json::Value) -> Result<String, ToolError> {
                Ok("x".repeat(20_000))
            }
        }

        fn charged(tokens: u64) -> Vec<Result<ProviderEvent, ProviderError>> {
            vec![Ok(ProviderEvent::Usage {
                prompt_tokens: Some(tokens),
                cached_tokens: None,
                completion_tokens: None,
            })]
        }

        let bus = EventBus::new(128);
        let mut rx = bus.subscribe();
        let provider = Arc::new(Scripted {
            scripts: Mutex::new(vec![
                vec![
                    Ok(ProviderEvent::ToolCalls(vec![ToolCall {
                        id: "c1".to_string(),
                        name: "big".to_string(),
                        arguments: "{}".to_string(),
                    }])),
                    Ok(ProviderEvent::Usage {
                        prompt_tokens: Some(400_000),
                        cached_tokens: None,
                        completion_tokens: None,
                    }),
                ],
                {
                    let mut step = text("ok");
                    step.extend(charged(400_000));
                    step
                },
            ]),
            requests: Mutex::new(Vec::new()),
        });

        let mut tools = ToolRegistry::new();
        tools.register(Arc::new(BigEcho));
        let app = App::with_tools(
            registry(provider),
            "scripted",
            "modelo".into(),
            bus,
            None,
            tools,
            String::new(),
        )
        .with_context_budget(10_000)
        .with_context_target(0)
        .with_context_recent_results(0);

        app.handle(Command::SendMessage { text: "a".into() })
            .await
            .unwrap();

        let mut trimmed = false;
        while let Ok(event) = rx.recv().await {
            if matches!(event, Event::ContextTrimmed { .. }) {
                trimmed = true;
            }
            if matches!(event, Event::StreamFinished { .. }) {
                break;
            }
        }

        assert!(
            trimmed,
            "o provider cobrou 400k e o orçamento do passo seguinte não encolheu"
        );
    }

    #[tokio::test]
    async fn the_prompt_estimate_counts_the_system_prefix() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("telemetry.jsonl");
        let telemetry = crate::telemetry::Telemetry::open(&path);
        let bus = EventBus::new(32);
        let mut rx = bus.subscribe();
        let prefix = "PALAVRA ".repeat(200);
        let expected = tupi_domain::tokens::estimate_tokens(&prefix);
        let provider = Arc::new(Scripted {
            scripts: Mutex::new(vec![text("ok")]),
            requests: Mutex::new(Vec::new()),
        });
        let app = app_with_prefix(provider, bus, &prefix).with_telemetry(telemetry);

        app.handle(Command::SendMessage { text: "a".into() })
            .await
            .unwrap();
        wait_for_finished(&mut rx).await;

        let mut body = String::new();
        for _ in 0..200 {
            body = std::fs::read_to_string(&path).unwrap_or_default();
            if body.contains("\"prompt_sent\"") {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        let estimate = body
            .lines()
            .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
            .find(|value| value["event"] == "prompt_sent")
            .and_then(|value| value["estimate_tokens"].as_u64())
            .expect("prompt_sent");

        assert!(
            estimate >= expected as u64,
            "estimate {estimate} não contou o prefixo ({expected})"
        );
    }
}

#[cfg(test)]
mod budget_tests {
    use super::*;

    #[test]
    fn the_observed_scale_doubles_when_the_provider_charges_double() {
        assert_eq!(observed_scale(1_000, 2_000), Some(SCALE_ONE * 2));
    }

    #[test]
    fn the_observed_scale_is_ignored_without_numbers() {
        assert_eq!(observed_scale(0, 100), None);
        assert_eq!(observed_scale(100, 0), None);
    }

    #[test]
    fn the_observed_scale_is_bounded() {
        assert_eq!(observed_scale(1_000, 100_000), Some(SCALE_MAX));
        assert_eq!(observed_scale(1_000, 1), Some(SCALE_MIN));
    }

    #[test]
    fn a_scaled_budget_shrinks_when_the_app_underestimates() {
        assert_eq!(scaled_budget(32_000, SCALE_ONE * 2), 16_000);
        assert_eq!(scaled_budget(32_000, SCALE_ONE), 32_000);
    }

    #[test]
    fn the_scale_moves_toward_the_observation() {
        assert_eq!(next_scale(SCALE_ONE, Some(SCALE_ONE * 3)), SCALE_ONE * 2);
        assert_eq!(next_scale(SCALE_ONE * 2, None), SCALE_ONE * 2);
    }

    #[test]
    fn tool_tokens_measure_the_json_the_provider_sends() {
        let specs = vec![tupi_kernel::ToolSpec {
            name: "read".into(),
            description: "lê um arquivo".into(),
            schema: serde_json::json!({"type": "object", "properties": {"path": {"type": "string"}}}),
        }];
        let json = serde_json::to_string(&tupi_kernel::tools_payload(&specs)).unwrap();

        assert_eq!(
            tool_spec_tokens(&specs),
            tupi_domain::tokens::estimate_tokens(&json)
        );
    }

    #[test]
    fn plan_context_scales_target_with_message_budget_when_fixed_tokens_are_large() {
        let mut messages = Vec::new();
        for index in 0..16 {
            messages.push(Message::user(format!(
                "turno {index}: {}",
                "palavra ".repeat(400)
            )));
            messages.push(Message::assistant(format!(
                "resposta {index}: {}",
                "palavra ".repeat(400)
            )));
        }

        // budget=20_000, target=10_000, fixed_tokens=10_000 -> message_budget=10_000.
        // Without scaling target proportionally to message_budget, message_target=10_000 == message_budget,
        // which disables amortized pruning and leaves estimate right near 10_000 instead of cutting to ~5_000.
        let planned = plan_context(&messages, 20_000, 10_000, 3, 10_000);

        assert!(planned.trimmed());
        assert!(
            planned.estimate <= 5_500,
            "expected amortized cut to <= 5_500 message tokens, got {}",
            planned.estimate
        );
    }

    #[test]
    fn plan_context_scales_target_smoothly_when_message_budget_is_just_above_target() {
        let mut messages = Vec::new();
        for index in 0..16 {
            messages.push(Message::user(format!(
                "turno {index}: {}",
                "palavra ".repeat(400)
            )));
            messages.push(Message::assistant(format!(
                "resposta {index}: {}",
                "palavra ".repeat(400)
            )));
        }

        // budget=32_000, target=16_000, fixed_tokens=15_500 -> message_budget=16_500.
        // Proportional target is 16_500 * 16_000 / 32_000 = 8_250 (leaving an 8_250-token amortization window),
        // NOT 16_000 (which would leave only a 500-token window before thrashing again).
        let planned = plan_context(&messages, 32_000, 16_000, 3, 15_500);

        assert!(planned.trimmed());
        assert_eq!(planned.budget, 8_250);
        assert!(
            planned.estimate <= 8_250,
            "expected amortized cut to <= 8_250 message tokens, got {}",
            planned.estimate
        );
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    use async_trait::async_trait;
    use futures_util::StreamExt;
    use tupi_domain::{Event, Role, ToolCall};
    use tupi_kernel::{
        ChatRequest, Command, CommandError, CommandHandler, CommandOutcome, EventBus, Provider,
        ProviderError, ProviderEvent, ProviderRegistry, ProviderStream, Query, QueryHandler,
        QueryResult, Tool, ToolError, ToolRegistry, ToolSpec,
    };

    struct FakeProvider {
        tokens: Vec<&'static str>,
    }

    #[async_trait]
    impl Provider for FakeProvider {
        fn id(&self) -> &str {
            "fake"
        }

        async fn list_models(&self) -> Result<Vec<String>, ProviderError> {
            Ok(Vec::new())
        }

        async fn stream_chat(
            &self,
            _request: ChatRequest,
        ) -> Result<ProviderStream, ProviderError> {
            let items: Vec<Result<ProviderEvent, ProviderError>> = self
                .tokens
                .iter()
                .map(|token| Ok(ProviderEvent::Text((*token).to_string())))
                .collect();
            Ok(Box::pin(futures_util::stream::iter(items)))
        }
    }

    struct HangingProvider;

    #[async_trait]
    impl Provider for HangingProvider {
        fn id(&self) -> &str {
            "hanging"
        }

        async fn list_models(&self) -> Result<Vec<String>, ProviderError> {
            Ok(Vec::new())
        }

        async fn stream_chat(
            &self,
            _request: ChatRequest,
        ) -> Result<ProviderStream, ProviderError> {
            let first = futures_util::stream::iter(vec![Ok(ProviderEvent::Text("a".to_string()))]);
            Ok(Box::pin(first.chain(futures_util::stream::pending::<
                Result<ProviderEvent, ProviderError>,
            >())))
        }
    }

    struct FailingProvider;

    #[async_trait]
    impl Provider for FailingProvider {
        fn id(&self) -> &str {
            "failing"
        }

        async fn list_models(&self) -> Result<Vec<String>, ProviderError> {
            Ok(Vec::new())
        }

        async fn stream_chat(
            &self,
            _request: ChatRequest,
        ) -> Result<ProviderStream, ProviderError> {
            let items = vec![
                Ok(ProviderEvent::Text("a".to_string())),
                Err(ProviderError::Network("caiu".to_string())),
            ];
            Ok(Box::pin(futures_util::stream::iter(items)))
        }
    }

    struct ModelProvider;

    #[async_trait]
    impl Provider for ModelProvider {
        fn id(&self) -> &str {
            "models"
        }

        async fn list_models(&self) -> Result<Vec<String>, ProviderError> {
            Ok(vec![
                "deepseek-flash".to_string(),
                "deepseek-v4-pro".to_string(),
            ])
        }

        async fn stream_chat(
            &self,
            _request: ChatRequest,
        ) -> Result<ProviderStream, ProviderError> {
            Ok(Box::pin(futures_util::stream::empty::<
                Result<ProviderEvent, ProviderError>,
            >()))
        }
    }

    fn registry(provider: Arc<dyn Provider>) -> ProviderRegistry {
        let mut registry = ProviderRegistry::new();
        registry.register(provider);
        registry
    }

    #[tokio::test]
    async fn list_models_query_returns_provider_models() {
        let app = App::new(
            registry(Arc::new(ModelProvider)),
            "models",
            "modelo".into(),
            EventBus::new(4),
        );

        let QueryResult::Models(models) = app.query(Query::ListModels).await.unwrap() else {
            panic!("esperava Models");
        };

        assert_eq!(
            models,
            vec!["deepseek-flash".to_string(), "deepseek-v4-pro".to_string()]
        );
    }

    /// Catálogo fixo de workflows para os testes de `/workflows/<nome>`.
    fn fixed_workflows(names: &[&str]) -> Arc<crate::workflow::FixedIndex> {
        Arc::new(crate::workflow::FixedIndex {
            workflows: names
                .iter()
                .map(|name| tupi_domain::Workflow {
                    name: (*name).to_string(),
                    description: String::new(),
                    source: tupi_domain::WorkflowSource::LocalTupi,
                    path: None,
                    body: format!("# {name}\n\n1. enquadre\n"),
                    shadowed: Vec::new(),
                })
                .collect(),
        })
    }

    #[tokio::test]
    async fn a_workflow_turn_sends_the_steps_to_the_model() {
        let bus = EventBus::new(16);
        let mut rx = bus.subscribe();
        let provider = Arc::new(FakeProvider { tokens: vec!["ok"] });
        let app = App::new(registry(provider), "fake", "modelo".into(), bus)
            .with_workflows(fixed_workflows(&["feature"]));

        let outcome = app
            .handle(Command::RunWorkflow {
                name: "feature".into(),
                extra: "no parser".into(),
            })
            .await
            .unwrap();
        assert!(matches!(outcome, CommandOutcome::Started { .. }));
        wait_for_end(&mut rx).await;

        let QueryResult::History(history) = app.query(Query::History).await.unwrap() else {
            panic!("esperava History");
        };
        assert_eq!(history[0].role, Role::User);
        assert!(history[0].content.contains("Siga o workflow `feature`"));
        assert!(history[0].content.contains("1. enquadre"));
        assert!(history[0].content.contains("Pedido do usuário: no parser"));
        assert_eq!(history[1].content, "ok");
    }

    #[tokio::test]
    async fn an_unknown_workflow_never_reaches_the_model() {
        let bus = EventBus::new(16);
        let provider = Arc::new(FakeProvider { tokens: vec!["ok"] });
        let app = App::new(registry(provider), "fake", "modelo".into(), bus)
            .with_workflows(fixed_workflows(&["feature"]));

        let error = app
            .handle(Command::RunWorkflow {
                name: "deploy".into(),
                extra: String::new(),
            })
            .await
            .unwrap_err();
        assert!(error.to_string().contains("deploy"));
        assert!(error.to_string().contains("feature"));

        let QueryResult::History(history) = app.query(Query::History).await.unwrap() else {
            panic!("esperava History");
        };
        assert!(history.is_empty());
    }

    #[tokio::test]
    async fn a_workflow_without_a_name_explains_the_usage() {
        let bus = EventBus::new(8);
        let app = App::new(
            registry(Arc::new(FakeProvider { tokens: vec!["ok"] })),
            "fake",
            "modelo".into(),
            bus,
        );

        let error = app
            .handle(Command::RunWorkflow {
                name: "  ".into(),
                extra: String::new(),
            })
            .await
            .unwrap_err();
        assert!(error.to_string().contains("/workflows/"));
    }

    #[tokio::test]
    async fn send_message_publishes_tokens_and_appends_history() {
        let bus = EventBus::new(16);
        let mut rx = bus.subscribe();
        let app = App::new(
            registry(Arc::new(FakeProvider {
                tokens: vec!["a", "b"],
            })),
            "fake",
            "modelo".into(),
            bus,
        );

        let outcome = app
            .handle(Command::SendMessage { text: "oi".into() })
            .await
            .unwrap();
        let CommandOutcome::Started { request_id } = outcome else {
            panic!("esperava Started");
        };

        assert_eq!(
            rx.recv().await.unwrap(),
            Event::Token {
                request_id,
                text: "a".into()
            }
        );
        assert_eq!(
            rx.recv().await.unwrap(),
            Event::Token {
                request_id,
                text: "b".into()
            }
        );
        assert_eq!(
            rx.recv().await.unwrap(),
            Event::StreamFinished {
                request_id,
                prompt_tokens: None,
                cached_tokens: None,
                completion_tokens: None,
            }
        );

        let QueryResult::History(history) = app.query(Query::History).await.unwrap() else {
            panic!("esperava History");
        };
        assert_eq!(history.len(), 2);
        assert_eq!(history[0].content, "oi");
        assert_eq!(history[1].role, Role::Assistant);
        assert_eq!(history[1].content, "ab");
    }

    #[tokio::test]
    async fn second_send_while_streaming_is_busy() {
        let app = App::new(
            registry(Arc::new(HangingProvider)),
            "hanging",
            "modelo".into(),
            EventBus::new(16),
        );

        let first = app
            .handle(Command::SendMessage { text: "1".into() })
            .await
            .unwrap();
        assert!(matches!(first, CommandOutcome::Started { .. }));

        let second = app.handle(Command::SendMessage { text: "2".into() }).await;
        assert!(matches!(second, Err(CommandError::Busy)));
    }

    #[tokio::test]
    async fn cancel_appends_partial_and_finishes() {
        let bus = EventBus::new(16);
        let mut rx = bus.subscribe();
        let app = App::new(
            registry(Arc::new(HangingProvider)),
            "hanging",
            "modelo".into(),
            bus,
        );

        let CommandOutcome::Started { request_id } = app
            .handle(Command::SendMessage { text: "oi".into() })
            .await
            .unwrap()
        else {
            panic!("esperava Started");
        };

        assert_eq!(
            rx.recv().await.unwrap(),
            Event::Token {
                request_id,
                text: "a".into()
            }
        );

        let outcome = app
            .handle(Command::CancelStream { request_id })
            .await
            .unwrap();
        assert_eq!(outcome, CommandOutcome::Cancelled);
        assert_eq!(
            rx.recv().await.unwrap(),
            Event::StreamFinished {
                request_id,
                prompt_tokens: None,
                cached_tokens: None,
                completion_tokens: None,
            }
        );

        let QueryResult::History(history) = app.query(Query::History).await.unwrap() else {
            panic!("esperava History");
        };
        assert_eq!(history.len(), 2);
        assert_eq!(history[1].role, Role::Assistant);
        assert_eq!(history[1].content, "a");
    }

    #[tokio::test]
    async fn failure_appends_partial_and_reports_error() {
        let bus = EventBus::new(16);
        let mut rx = bus.subscribe();
        let app = App::new(
            registry(Arc::new(FailingProvider)),
            "failing",
            "modelo".into(),
            bus,
        );

        let CommandOutcome::Started { request_id } = app
            .handle(Command::SendMessage { text: "oi".into() })
            .await
            .unwrap()
        else {
            panic!("esperava Started");
        };

        assert_eq!(
            rx.recv().await.unwrap(),
            Event::Token {
                request_id,
                text: "a".into()
            }
        );
        assert_eq!(
            rx.recv().await.unwrap(),
            Event::StreamFailed {
                request_id,
                message: "falha de rede: caiu".into()
            }
        );

        let QueryResult::History(history) = app.query(Query::History).await.unwrap() else {
            panic!("esperava History");
        };
        assert_eq!(history.len(), 2);
        assert_eq!(history[1].role, Role::Assistant);
        assert_eq!(history[1].content, "a");
    }

    fn tool_call(id: &str, name: &str, arguments: &str) -> ToolCall {
        ToolCall {
            id: id.into(),
            name: name.into(),
            arguments: arguments.into(),
        }
    }

    struct ScriptedProvider {
        scripts: Mutex<VecDeque<Vec<Result<ProviderEvent, ProviderError>>>>,
        requests: Mutex<Vec<ChatRequest>>,
    }

    impl ScriptedProvider {
        fn new(scripts: Vec<Vec<Result<ProviderEvent, ProviderError>>>) -> Self {
            Self {
                scripts: Mutex::new(scripts.into()),
                requests: Mutex::new(Vec::new()),
            }
        }
    }

    #[async_trait]
    impl Provider for ScriptedProvider {
        fn id(&self) -> &str {
            "scripted"
        }

        async fn list_models(&self) -> Result<Vec<String>, ProviderError> {
            Ok(Vec::new())
        }

        async fn stream_chat(&self, request: ChatRequest) -> Result<ProviderStream, ProviderError> {
            self.requests.lock().unwrap().push(request);
            let items = self.scripts.lock().unwrap().pop_front().expect("script");
            Ok(Box::pin(futures_util::stream::iter(items)))
        }
    }

    struct EchoTool {
        calls: AtomicUsize,
    }

    impl EchoTool {
        fn new() -> Self {
            Self {
                calls: AtomicUsize::new(0),
            }
        }
    }

    impl Tool for EchoTool {
        fn spec(&self) -> ToolSpec {
            ToolSpec {
                name: "echo".into(),
                description: "ecoa os argumentos".into(),
                schema: serde_json::json!({"type": "object"}),
            }
        }

        fn call(&self, arguments: serde_json::Value) -> Result<String, ToolError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(format!("eco: {arguments}"))
        }
    }

    fn app_with_tools(provider: Arc<dyn Provider>, bus: EventBus, tool: Arc<dyn Tool>) -> Arc<App> {
        let mut tools = ToolRegistry::new();
        tools.register(tool);
        App::with_tools(
            registry(provider),
            "scripted",
            "modelo".into(),
            bus,
            None,
            tools,
            String::new(),
        )
    }

    fn text_event(text: &str) -> Result<ProviderEvent, ProviderError> {
        Ok(ProviderEvent::Text(text.to_string()))
    }

    fn calls_event(calls: Vec<ToolCall>) -> Result<ProviderEvent, ProviderError> {
        Ok(ProviderEvent::ToolCalls(calls))
    }

    async fn wait_for_end(rx: &mut tokio::sync::broadcast::Receiver<Event>) {
        loop {
            match rx.recv().await.unwrap() {
                Event::StreamFinished { .. } | Event::StreamFailed { .. } => return,
                _ => {}
            }
        }
    }

    #[tokio::test]
    async fn tool_call_is_executed_and_the_model_is_called_again() {
        let bus = EventBus::new(32);
        let mut rx = bus.subscribe();
        let provider = Arc::new(ScriptedProvider::new(vec![
            vec![calls_event(vec![tool_call("c1", "echo", "{\"n\":1}")])],
            vec![text_event("fim")],
        ]));
        let app = app_with_tools(provider.clone(), bus, Arc::new(EchoTool::new()));

        let CommandOutcome::Started { request_id } = app
            .handle(Command::SendMessage {
                text: "leia".into(),
            })
            .await
            .unwrap()
        else {
            panic!("esperava Started");
        };

        match rx.recv().await.unwrap() {
            Event::ToolStarted {
                call_id,
                name,
                summary,
                ..
            } => {
                assert_eq!(call_id, "c1");
                assert_eq!(name, "echo");
                assert_eq!(summary, "{\"n\":1}");
            }
            other => panic!("esperava ToolStarted, veio {other:?}"),
        }
        assert!(matches!(
            rx.recv().await.unwrap(),
            Event::ToolFinished { call_id, ok: true, .. } if call_id == "c1"
        ));
        assert_eq!(
            rx.recv().await.unwrap(),
            Event::Token {
                request_id,
                text: "fim".into()
            }
        );
        assert_eq!(
            rx.recv().await.unwrap(),
            Event::StreamFinished {
                request_id,
                prompt_tokens: None,
                cached_tokens: None,
                completion_tokens: None,
            }
        );

        let QueryResult::History(history) = app.query(Query::History).await.unwrap() else {
            panic!("esperava History");
        };
        assert_eq!(history.len(), 4);
        assert_eq!(history[1].role, Role::Assistant);
        assert_eq!(history[1].tool_calls.len(), 1);
        assert_eq!(history[2].role, Role::Tool);
        assert_eq!(history[2].tool_call_id.as_deref(), Some("c1"));
        assert!(history[2].content.starts_with("eco:"));
        assert_eq!(history[3].content, "fim");

        let requests = provider.requests.lock().unwrap();
        assert_eq!(requests.len(), 2);
        assert!(
            requests[1]
                .messages
                .iter()
                .any(|message| message.role == Role::Tool)
        );
    }

    #[tokio::test]
    async fn multiple_tool_calls_run_in_order() {
        let bus = EventBus::new(32);
        let mut rx = bus.subscribe();
        let provider = Arc::new(ScriptedProvider::new(vec![
            vec![calls_event(vec![
                tool_call("c1", "echo", "{\"n\":1}"),
                tool_call("c2", "echo", "{\"n\":2}"),
            ])],
            vec![text_event("fim")],
        ]));
        let app = app_with_tools(provider, bus, Arc::new(EchoTool::new()));

        app.handle(Command::SendMessage {
            text: "leia".into(),
        })
        .await
        .unwrap();
        wait_for_end(&mut rx).await;

        let QueryResult::History(history) = app.query(Query::History).await.unwrap() else {
            panic!("esperava History");
        };
        assert_eq!(history[2].tool_call_id.as_deref(), Some("c1"));
        assert_eq!(history[3].tool_call_id.as_deref(), Some("c2"));
    }

    #[tokio::test]
    async fn malformed_arguments_do_not_invoke_the_tool() {
        let bus = EventBus::new(32);
        let mut rx = bus.subscribe();
        let provider = Arc::new(ScriptedProvider::new(vec![
            vec![calls_event(vec![tool_call("c1", "echo", "{")])],
            vec![text_event("fim")],
        ]));
        let tool = Arc::new(EchoTool::new());
        let app = app_with_tools(provider, bus, tool.clone());

        app.handle(Command::SendMessage {
            text: "leia".into(),
        })
        .await
        .unwrap();

        wait_for_end(&mut rx).await;
        assert_eq!(tool.calls.load(Ordering::SeqCst), 0);
        let QueryResult::History(history) = app.query(Query::History).await.unwrap() else {
            panic!("esperava History");
        };
        assert!(history[2].content.contains("argumento inválido"));
    }

    #[tokio::test]
    async fn unknown_tool_returns_an_explicit_tool_message() {
        let bus = EventBus::new(32);
        let mut rx = bus.subscribe();
        let provider = Arc::new(ScriptedProvider::new(vec![
            vec![calls_event(vec![tool_call("c1", "nope", "{}")])],
            vec![text_event("fim")],
        ]));
        let app = app_with_tools(provider, bus, Arc::new(EchoTool::new()));

        app.handle(Command::SendMessage {
            text: "leia".into(),
        })
        .await
        .unwrap();

        wait_for_end(&mut rx).await;
        let QueryResult::History(history) = app.query(Query::History).await.unwrap() else {
            panic!("esperava History");
        };
        assert!(history[2].content.contains("tool desconhecida"));
    }

    #[tokio::test]
    async fn app_runs_the_real_read_tool_end_to_end() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("auth.rs"),
            "pub fn verify_token(raw: &str) -> bool {\n    !raw.is_empty()\n}\n",
        )
        .unwrap();

        let bus = EventBus::new(32);
        let mut rx = bus.subscribe();
        let provider = Arc::new(ScriptedProvider::new(vec![
            vec![calls_event(vec![tool_call(
                "c1",
                "read",
                "{\"path\":\"auth.rs\",\"symbol\":\"verify_token\"}",
            )])],
            vec![text_event("li")],
        ]));
        let mut tools = ToolRegistry::new();
        tupi_tools::register(dir.path().to_path_buf(), &mut tools);
        let app = App::with_tools(
            registry(provider),
            "scripted",
            "modelo".into(),
            bus,
            None,
            tools,
            String::new(),
        );

        app.handle(Command::SendMessage {
            text: "leia".into(),
        })
        .await
        .unwrap();
        wait_for_end(&mut rx).await;

        let QueryResult::History(history) = app.query(Query::History).await.unwrap() else {
            panic!("esperava History");
        };
        assert_eq!(history[2].role, Role::Tool);
        assert!(
            history[2].content.contains("!raw.is_empty()"),
            "{:?}",
            history[2]
        );
    }

    #[tokio::test]
    async fn app_runs_the_real_search_tool_end_to_end() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("auth.rs"),
            "pub fn verify_token(raw: &str) -> bool {\n    !raw.is_empty()\n}\n",
        )
        .unwrap();

        let bus = EventBus::new(32);
        let mut rx = bus.subscribe();
        let provider = Arc::new(ScriptedProvider::new(vec![
            vec![calls_event(vec![tool_call(
                "c1",
                "search",
                "{\"query\":\"onde o token é validado\"}",
            )])],
            vec![text_event("li")],
        ]));
        let mut tools = ToolRegistry::new();
        tupi_tools::register(dir.path().to_path_buf(), &mut tools);
        let app = App::with_tools(
            registry(provider),
            "scripted",
            "modelo".into(),
            bus,
            None,
            tools,
            String::new(),
        );

        app.handle(Command::SendMessage {
            text: "procure".into(),
        })
        .await
        .unwrap();

        let mut summary = None;
        loop {
            match rx.recv().await.unwrap() {
                Event::ToolStarted { summary: value, .. } => summary = Some(value),
                Event::StreamFinished { .. } => break,
                Event::StreamFailed { message, .. } => panic!("stream falhou: {message}"),
                _ => {}
            }
        }

        assert!(
            summary
                .as_deref()
                .is_some_and(|value| value.contains("query=")),
            "{summary:?}"
        );
        let QueryResult::History(history) = app.query(Query::History).await.unwrap() else {
            panic!("esperava History");
        };
        assert_eq!(history[2].role, Role::Tool);
        assert!(
            history[2].content.contains("verify_token"),
            "{:?}",
            history[2]
        );
        assert!(
            history[2].content.contains("!raw.is_empty()"),
            "{:?}",
            history[2]
        );
    }

    #[tokio::test]
    async fn app_runs_edit_then_read_end_to_end() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("auth.rs");
        let content = "pub fn verify_token(raw: &str) -> bool {\n    !raw.is_empty()\n}\n";
        std::fs::write(&path, content).unwrap();
        let hash = blake3::hash(content.as_bytes()).to_hex()[..12].to_string();

        let bus = EventBus::new(64);
        let mut rx = bus.subscribe();
        let provider = Arc::new(ScriptedProvider::new(vec![
            vec![calls_event(vec![tool_call(
                "c1",
                "edit",
                &serde_json::json!({
                    "path": "auth.rs",
                    "symbol": "verify_token",
                    "expected_hash": hash,
                    "new_body": "{\n    raw.len() > 3\n}"
                })
                .to_string(),
            )])],
            vec![calls_event(vec![tool_call(
                "c2",
                "read",
                "{\"path\":\"auth.rs\",\"symbol\":\"verify_token\"}",
            )])],
            vec![text_event("ok")],
        ]));
        let mut tools = ToolRegistry::new();
        tupi_tools::register(dir.path().to_path_buf(), &mut tools);
        let app = App::with_tools(
            registry(provider),
            "scripted",
            "modelo".into(),
            bus,
            None,
            tools,
            String::new(),
        );

        app.handle(Command::SendMessage {
            text: "edite".into(),
        })
        .await
        .unwrap();

        let mut summaries = Vec::new();
        loop {
            match rx.recv().await.unwrap() {
                Event::ToolStarted { summary, .. } => summaries.push(summary),
                Event::StreamFinished { .. } => break,
                Event::StreamFailed { message, .. } => panic!("stream falhou: {message}"),
                _ => {}
            }
        }

        assert!(
            summaries
                .iter()
                .any(|summary| summary.contains("symbol=verify_token")),
            "{summaries:?}"
        );

        let QueryResult::History(history) = app.query(Query::History).await.unwrap() else {
            panic!("esperava History");
        };
        assert_eq!(history[2].role, Role::Tool);
        assert!(history[2].content.contains("editado"), "{:?}", history[2]);
        assert_eq!(history[4].role, Role::Tool);
        assert!(
            history[4].content.contains("raw.len() > 3"),
            "{:?}",
            history[4]
        );
        assert!(
            !history[4].content.contains("!raw.is_empty()"),
            "{:?}",
            history[4]
        );
    }

    struct SlowTool;

    impl Tool for SlowTool {
        fn spec(&self) -> ToolSpec {
            ToolSpec {
                name: "echo".into(),
                description: "demora".into(),
                schema: serde_json::json!({"type": "object"}),
            }
        }

        fn call(&self, _arguments: serde_json::Value) -> Result<String, ToolError> {
            std::thread::sleep(std::time::Duration::from_millis(50));
            Ok("lento".into())
        }
    }

    #[tokio::test]
    async fn stream_error_after_tool_calls_closes_pending_calls() {
        let bus = EventBus::new(32);
        let mut rx = bus.subscribe();
        let provider = Arc::new(ScriptedProvider::new(vec![vec![
            calls_event(vec![tool_call("c1", "echo", "{}")]),
            Err(ProviderError::Network("caiu".into())),
        ]]));
        let app = app_with_tools(provider, bus, Arc::new(EchoTool::new()));

        app.handle(Command::SendMessage {
            text: "leia".into(),
        })
        .await
        .unwrap();
        wait_for_end(&mut rx).await;

        let QueryResult::History(history) = app.query(Query::History).await.unwrap() else {
            panic!("esperava History");
        };
        assert_eq!(history.len(), 3);
        assert_eq!(history[1].tool_calls.len(), 1);
        assert_eq!(history[2].role, Role::Tool);
        assert_eq!(history[2].tool_call_id.as_deref(), Some("c1"));
        assert!(history[2].content.contains("interrompido"));
    }

    #[tokio::test]
    async fn cancel_during_tool_execution_closes_every_pending_call() {
        let _ = SlowTool;
        let finished = Arc::new(AtomicBool::new(false));
        struct TrackedSlowTool(Arc<AtomicBool>);
        impl Tool for TrackedSlowTool {
            fn spec(&self) -> ToolSpec {
                ToolSpec {
                    name: "echo".to_string(),
                    description: "demora".to_string(),
                    schema: serde_json::json!({"type": "object"}),
                }
            }
            fn call(&self, _arguments: serde_json::Value) -> Result<String, ToolError> {
                std::thread::sleep(std::time::Duration::from_millis(120));
                self.0.store(true, Ordering::SeqCst);
                Ok("tarde".to_string())
            }
        }

        let bus = EventBus::new(64);
        let mut rx = bus.subscribe();
        let provider = Arc::new(ScriptedProvider::new(vec![
            vec![calls_event(vec![
                tool_call("c1", "echo", "{}"),
                tool_call("c2", "echo", "{}"),
            ])],
            vec![text_event("depois")],
        ]));
        let app = app_with_tools(
            provider,
            bus,
            Arc::new(TrackedSlowTool(Arc::clone(&finished))),
        );

        let CommandOutcome::Started { request_id } = app
            .handle(Command::SendMessage {
                text: "leia".into(),
            })
            .await
            .unwrap()
        else {
            panic!("esperava Started");
        };

        // Espera a primeira tool começar e cancela enquanto ela roda.
        loop {
            match rx.recv().await.unwrap() {
                Event::ToolStarted { call_id, .. } if call_id == "c1" => break,
                Event::StreamFinished { .. } | Event::StreamFailed { .. } => {
                    panic!("turno terminou antes do cancelamento")
                }
                _ => {}
            }
        }

        let outcome = app
            .handle(Command::CancelStream { request_id })
            .await
            .unwrap();
        assert_eq!(outcome, CommandOutcome::Cancelled);
        assert!(
            finished.load(Ordering::SeqCst),
            "cancel returned while the blocking tool thread from the cancelled turn was still running!"
        );
        wait_for_end(&mut rx).await;

        let QueryResult::History(history) = app.query(Query::History).await.unwrap() else {
            panic!("esperava History");
        };
        let tool_ids: Vec<&str> = history
            .iter()
            .filter(|message| message.role == Role::Tool)
            .filter_map(|message| message.tool_call_id.as_deref())
            .collect();
        assert_eq!(tool_ids, vec!["c1", "c2"]);
        assert_eq!(history[1].tool_calls.len(), 2);

        // O histórico está consistente: um novo turno é aceito e completa.
        let CommandOutcome::Started { .. } = app
            .handle(Command::SendMessage {
                text: "de novo".into(),
            })
            .await
            .unwrap()
        else {
            panic!("esperava Started");
        };
        wait_for_end(&mut rx).await;

        let QueryResult::History(history) = app.query(Query::History).await.unwrap() else {
            panic!("esperava History");
        };
        assert_eq!(history.last().unwrap().content, "depois");
    }

    struct LoopingProvider {
        calls: AtomicUsize,
    }

    #[async_trait]
    impl Provider for LoopingProvider {
        fn id(&self) -> &str {
            "looping"
        }

        async fn list_models(&self) -> Result<Vec<String>, ProviderError> {
            Ok(Vec::new())
        }

        async fn stream_chat(
            &self,
            _request: ChatRequest,
        ) -> Result<ProviderStream, ProviderError> {
            let n = self.calls.fetch_add(1, Ordering::SeqCst);
            let items = vec![calls_event(vec![tool_call(&format!("c{n}"), "echo", "{}")])];
            Ok(Box::pin(futures_util::stream::iter(items)))
        }
    }

    #[tokio::test]
    async fn the_step_limit_finishes_with_a_final_synthesis_instead_of_failing() {
        let bus = EventBus::new(64);
        let mut rx = bus.subscribe();
        let provider = Arc::new(LoopingProvider {
            calls: AtomicUsize::new(0),
        });
        let mut tools = ToolRegistry::new();
        tools.register(Arc::new(EchoTool::new()));
        let app = App::with_tools(
            registry(provider),
            "looping",
            "modelo".into(),
            bus,
            None,
            tools,
            String::new(),
        );

        let CommandOutcome::Started { request_id } = app
            .handle(Command::SendMessage {
                text: "leia".into(),
            })
            .await
            .unwrap()
        else {
            panic!("esperava Started");
        };

        let mut finished = false;
        for _ in 0..(MAX_TOOL_STEPS * 2 + 4) {
            match rx.recv().await.unwrap() {
                Event::StreamFinished { .. } => {
                    finished = true;
                    break;
                }
                Event::StreamFailed { message, .. } => panic!("não esperava falha: {message}"),
                _ => {}
            }
        }
        assert!(finished, "esperava StreamFinished no limite de passos");

        let QueryResult::History(history) = app.query(Query::History).await.unwrap() else {
            panic!("esperava History");
        };
        assert_eq!(history.len(), 1 + (MAX_TOOL_STEPS - 1) * 2);
        assert!(
            history
                .iter()
                .all(|message| !message.content.contains("limite de passos"))
        );
        let _ = request_id;
    }

    fn usage_event(tokens: u64) -> Result<ProviderEvent, ProviderError> {
        Ok(ProviderEvent::Usage {
            prompt_tokens: None,
            cached_tokens: None,
            completion_tokens: Some(tokens),
        })
    }

    #[tokio::test]
    async fn usage_from_every_step_is_summed() {
        let bus = EventBus::new(32);
        let mut rx = bus.subscribe();
        let provider = Arc::new(ScriptedProvider::new(vec![
            vec![
                text_event("vou ler"),
                usage_event(10),
                calls_event(vec![tool_call("c1", "echo", "{\"n\":1}")]),
            ],
            vec![text_event("fim"), usage_event(5)],
        ]));
        let app = app_with_tools(provider, bus, Arc::new(EchoTool::new()));

        app.handle(Command::SendMessage {
            text: "leia".into(),
        })
        .await
        .unwrap();

        let mut total = None;
        for _ in 0..32 {
            if let Event::StreamFinished {
                completion_tokens, ..
            } = rx.recv().await.unwrap()
            {
                total = completion_tokens;
                break;
            }
        }

        assert_eq!(total, Some(15));
    }

    fn prompt_usage(prompt: u64, cached: u64) -> Result<ProviderEvent, ProviderError> {
        Ok(ProviderEvent::Usage {
            prompt_tokens: Some(prompt),
            cached_tokens: Some(cached),
            completion_tokens: Some(1),
        })
    }

    #[tokio::test]
    async fn prompt_and_cached_tokens_are_summed_across_steps() {
        let bus = EventBus::new(32);
        let mut rx = bus.subscribe();
        let provider = Arc::new(ScriptedProvider::new(vec![
            vec![
                text_event("vou ler"),
                prompt_usage(1_000, 800),
                calls_event(vec![tool_call("c1", "echo", "{\"n\":1}")]),
            ],
            vec![text_event("fim"), prompt_usage(1_200, 900)],
        ]));
        let app = app_with_tools(provider, bus, Arc::new(EchoTool::new()));

        app.handle(Command::SendMessage {
            text: "leia".into(),
        })
        .await
        .unwrap();

        let mut usage = None;
        for _ in 0..32 {
            if let Event::StreamFinished {
                prompt_tokens,
                cached_tokens,
                completion_tokens,
                ..
            } = rx.recv().await.unwrap()
            {
                usage = Some((prompt_tokens, cached_tokens, completion_tokens));
                break;
            }
        }

        assert_eq!(usage, Some((Some(2_200), Some(1_700), Some(2))));
    }

    #[tokio::test]
    async fn a_tight_context_budget_trims_the_history_and_reports_it() {
        let bus = EventBus::new(64);
        let mut rx = bus.subscribe();
        let provider = Arc::new(ScriptedProvider::new(vec![
            vec![text_event("ok")],
            vec![text_event("ok")],
        ]));
        let app =
            app_with_tools(provider, bus, Arc::new(EchoTool::new())).with_context_budget(1_000);

        app.handle(Command::SendMessage {
            text: "a ".repeat(3_000),
        })
        .await
        .unwrap();
        wait_for_end(&mut rx).await;

        app.handle(Command::SendMessage {
            text: "b ".repeat(3_000),
        })
        .await
        .unwrap();

        let mut omitted = None;
        for _ in 0..64 {
            match rx.recv().await.unwrap() {
                Event::ContextTrimmed { omitted_units, .. } => {
                    omitted = Some(omitted_units);
                    break;
                }
                Event::StreamFinished { .. } => break,
                _ => {}
            }
        }

        assert!(omitted.unwrap_or(0) >= 1, "esperava o histórico podado");
    }

    fn history_with_user_turns(turns: usize, words: usize) -> Vec<Message> {
        (0..turns)
            .map(|turn| Message::new(Role::User, format!("turno {turn} {}", "a ".repeat(words))))
            .collect()
    }

    #[test]
    fn tool_specs_are_subtracted_from_the_message_budget() {
        let messages = history_with_user_turns(6, 2_000);
        let plan = plan_context(&messages, 5_000, 5_000, 3, 2_000);

        assert_eq!(plan.budget, 3_000);
        assert!(
            plan.estimate <= 3_000,
            "estimativa {} passou do teto de mensagens",
            plan.estimate
        );
    }

    #[test]
    fn the_configured_target_wins_over_the_default_two_fifths() {
        let messages = history_with_user_turns(6, 4_000);
        let plan = plan_context(&messages, 10_000, 2_000, 3, 0);

        assert_eq!(plan.budget, 2_000);
    }

    #[test]
    fn zero_target_falls_back_to_two_fifths_of_the_message_budget() {
        let messages = history_with_user_turns(6, 4_000);
        let plan = plan_context(&messages, 10_000, 0, 3, 2_000);

        assert_eq!(plan.budget, 3_200);
    }

    #[test]
    fn the_configured_recent_results_are_forwarded_to_the_plan() {
        let mut messages = history_with_user_turns(2, 4_000);
        messages.push(Message::tool("c1", "x".repeat(16_000)));
        messages.push(Message::new(Role::User, "depois ".repeat(4_000)));

        let plan = plan_context(&messages, 500, 0, 0, 0);

        assert_eq!(plan.shrunk_results, 1);
    }

    #[test]
    fn a_tool_spec_costs_more_than_its_name() {
        let spec = ToolSpec {
            name: "read".to_string(),
            description: "palavra ".repeat(100),
            schema: serde_json::json!({"type": "object"}),
        };

        assert!(tool_spec_tokens(&[spec]) >= 100);
    }

    #[tokio::test]
    async fn the_step_limit_forces_a_synthesis_without_tools() {
        let bus = EventBus::new(64);
        let mut rx = bus.subscribe();
        let provider = Arc::new(ScriptedProvider::new(vec![
            vec![calls_event(vec![tool_call("c1", "echo", "{}")])],
            vec![calls_event(vec![tool_call("c2", "echo", "{}")])],
            vec![text_event("síntese final")],
        ]));
        let app =
            app_with_tools(provider.clone(), bus, Arc::new(EchoTool::new())).with_max_tool_steps(3);

        app.handle(Command::SendMessage {
            text: "investigue".into(),
        })
        .await
        .unwrap();
        wait_for_end(&mut rx).await;

        let requests = provider.requests.lock().unwrap();
        assert_eq!(requests.len(), 3);
        assert!(!requests[0].tools.is_empty());
        assert!(!requests[1].tools.is_empty());
        assert!(
            requests[2].tools.is_empty(),
            "a última chamada deve ser sem tools"
        );
        drop(requests);

        assert!(
            app.session()
                .messages
                .iter()
                .any(|message| message.content.contains("síntese final"))
        );
    }

    #[tokio::test]
    async fn the_model_is_warned_before_the_hard_step_limit() {
        let bus = EventBus::new(64);
        let mut rx = bus.subscribe();
        let provider = Arc::new(ScriptedProvider::new(vec![
            vec![calls_event(vec![tool_call("c1", "echo", "{}")])],
            vec![calls_event(vec![tool_call("c2", "echo", "{}")])],
            vec![calls_event(vec![tool_call("c3", "echo", "{}")])],
            vec![calls_event(vec![tool_call("c4", "echo", "{}")])],
            vec![text_event("fim")],
        ]));
        let app =
            app_with_tools(provider.clone(), bus, Arc::new(EchoTool::new())).with_max_tool_steps(5);

        app.handle(Command::SendMessage {
            text: "investigue".into(),
        })
        .await
        .unwrap();
        wait_for_end(&mut rx).await;

        let requests = provider.requests.lock().unwrap();
        assert_eq!(requests.len(), 5);
        assert!(
            !requests[1]
                .messages
                .iter()
                .any(|message| message.content.contains("Freio de passos"))
        );
        assert!(
            requests[2]
                .messages
                .iter()
                .any(|message| message.content.contains("restam 2 de 5"))
        );
        assert!(
            requests[4]
                .messages
                .iter()
                .any(|message| message.content.contains("última chamada"))
        );
        let last_msg = requests[2].messages.last().unwrap();
        assert_eq!(last_msg.role, Role::System);
        assert!(
            last_msg.content.contains("Freio de passos"),
            "o aviso deve ficar no fim da lista de mensagens para preservar o KV-cache do histórico"
        );
    }

    #[tokio::test]
    async fn amortized_trim_persists_in_session_so_next_turn_is_append_only() {
        let bus = EventBus::new(64);
        let mut rx = bus.subscribe();
        let provider = Arc::new(ScriptedProvider::new(vec![
            vec![text_event("ok 1")],
            vec![text_event("ok 2")],
            vec![text_event("ok 3")],
        ]));
        let app = app_with_tools(provider.clone(), bus, Arc::new(EchoTool::new()))
            .with_context_budget(2_000);

        app.handle(Command::SendMessage {
            text: "a ".repeat(800),
        })
        .await
        .unwrap();
        wait_for_end(&mut rx).await;

        app.handle(Command::SendMessage {
            text: "b ".repeat(1_400),
        })
        .await
        .unwrap();
        let mut trimmed_turn_2 = false;
        loop {
            match rx.recv().await.unwrap() {
                Event::ContextTrimmed { .. } => trimmed_turn_2 = true,
                Event::StreamFinished { .. } => break,
                _ => {}
            }
        }
        assert!(trimmed_turn_2, "turn 2 should trigger amortized trim");

        app.handle(Command::SendMessage {
            text: "c ".repeat(50),
        })
        .await
        .unwrap();
        let mut trimmed_turn_3 = false;
        loop {
            match rx.recv().await.unwrap() {
                Event::ContextTrimmed { .. } => trimmed_turn_3 = true,
                Event::StreamFinished { .. } => break,
                _ => {}
            }
        }
        assert!(
            !trimmed_turn_3,
            "turn 3 must grow append-only without re-trimming"
        );
    }
}
