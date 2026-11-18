//! Telemetria estruturada em JSONL.
//!
//! Cada linha é um objeto JSON completo. O writer roda em thread própria,
//! recebe linhas já serializadas por um canal limitado e nunca bloqueia o
//! chamador. Falha de disco desliga a telemetria; o app continua.

use serde::Serialize;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, TrySendError, sync_channel};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use tupi_kernel::{
    Command, CommandError, CommandOutcome, ProviderError, Query, QueryError, QueryResult,
};

const SCHEMA: u32 = 3;
const CHANNEL_CAPACITY: usize = 1024;
const MAX_TEXT_BYTES: usize = 512 * 1024;
const PROMPT_PREVIEW_BYTES: usize = 4 * 1024;
const ROTATE_BYTES: u64 = 32 * 1024 * 1024;
const ROTATE_KEEP: usize = 3;
/// Tool cujas execuções ganham arquivo próprio (bifurcação).
const SHELL_TOOL: &str = "shell";
/// Nome do arquivo irmão que recebe só as execuções da tool `shell`.
pub const SHELL_FILE: &str = "telemetry.shell.jsonl";
/// Variável que troca o destino do arquivo bifurcado.
const SHELL_PATH_ENV: &str = "TUPI_TELEMETRY_SHELL_PATH";

/// Handle barato e clonável para a telemetria. Quando desligado, todos os
/// métodos são praticamente no-op.
#[derive(Clone)]
pub struct Telemetry {
    inner: Arc<Inner>,
    shell: Option<Arc<Inner>>,
}

struct Inner {
    sender: Option<SyncSender<Vec<u8>>>,
    joiner: Mutex<Option<JoinHandle<()>>>,
    seq: Mutex<u64>,
    run: String,
    enabled: AtomicBool,
    path: Option<PathBuf>,
    stats: Stats,
}

impl Drop for Inner {
    fn drop(&mut self) {
        // Fecha o canal e espera a thread drenar o que já está na fila.
        self.sender.take();
        if let Ok(mut joiner) = self.joiner.lock()
            && let Some(handle) = joiner.take()
        {
            let _ = handle.join();
        }
    }
}

#[derive(Default)]
struct Stats {
    commands: AtomicU64,
    queries: AtomicU64,
    turns: AtomicU64,
    provider_requests: AtomicU64,
    tool_calls: AtomicU64,
    tool_failures: AtomicU64,
    prompt_tokens: AtomicU64,
    cached_tokens: AtomicU64,
    completion_tokens: AtomicU64,
    /// Tempo gasto dentro do provedor (soma de `ProviderResponse.duration_us`).
    provider_us: AtomicU64,
    /// Tempo até o primeiro chunk de cada requisição que produziu algum.
    first_token_us: AtomicU64,
    /// Tempo gasto dentro das tools (soma de `ToolFinished.duration_us`).
    tool_us: AtomicU64,
    /// Passos de turno encerrados; divide `provider_us` para dar o custo por passo.
    steps: AtomicU64,
    errors: AtomicU64,
    dropped: AtomicU64,
}

#[derive(Clone, Copy, Default, Serialize)]
pub struct StatsSnapshot {
    pub commands: u64,
    pub queries: u64,
    pub turns: u64,
    pub provider_requests: u64,
    pub tool_calls: u64,
    pub tool_failures: u64,
    pub prompt_tokens: u64,
    pub cached_tokens: u64,
    pub completion_tokens: u64,
    /// Tempo total dentro do provedor, somando todas as requisições.
    pub provider_us: u64,
    /// Tempo total até o primeiro chunk (só requisições que produziram algum).
    pub first_token_us: u64,
    /// Tempo total dentro das tools.
    pub tool_us: u64,
    /// Passos somados dos turnos encerrados; `provider_us / steps` é o custo médio por passo.
    pub steps: u64,
    pub errors: u64,
    pub dropped: u64,
}

impl Telemetry {
    /// Telemetria desligada; usada como padrão em testes e quando a abertura
    /// do arquivo falha.
    pub fn disabled() -> Self {
        Self {
            inner: Arc::new(Inner {
                sender: None,
                joiner: Mutex::new(None),
                seq: Mutex::new(0),
                run: run_id(),
                enabled: AtomicBool::new(false),
                path: None,
                stats: Stats::default(),
            }),
            shell: None,
        }
    }

    /// Abre o caminho em modo append. Nunca falha: em erro, devolve uma
    /// telemetria desligada e registra um warning no log textual.
    pub fn open(path: impl AsRef<Path>) -> Self {
        Self::open_paired(path.as_ref(), None)
    }

    /// Abre o arquivo principal e bifurca as execuções da tool `shell` para
    /// `shell_path`. Os dois arquivos compartilham o `run` e numeram `seq` de
    /// forma independente.
    pub fn open_with_shell(path: impl AsRef<Path>, shell_path: impl AsRef<Path>) -> Self {
        Self::open_paired(path.as_ref(), Some(shell_path.as_ref()))
    }

    /// Fecha o par: erro ao abrir o principal desliga tudo; erro só no arquivo
    /// bifurcado desliga apenas a bifurcação.
    fn open_paired(path: &Path, shell_path: Option<&Path>) -> Self {
        let run = run_id();
        let Some(inner) = open_sink(path, &run, "tupi-telemetry") else {
            return Self::disabled();
        };

        let shell = shell_path
            .filter(|shell| *shell != path)
            .and_then(|shell| open_sink(shell, &run, "tupi-telemetry-shell"));

        Self { inner, shell }
    }

    /// Respeita TUPI_TELEMETRY (0/off/false desliga), TUPI_TELEMETRY_PATH
    /// (troca o destino), TUPI_TELEMETRY_BODY (1/on/true/yes guarda o corpo do
    /// prompt; desligado por padrão) e TUPI_TELEMETRY_SHELL_PATH (troca o
    /// arquivo bifurcado das execuções da tool `shell`).
    pub fn from_env(default_path: PathBuf) -> Self {
        if let Ok(value) = std::env::var("TUPI_TELEMETRY")
            && matches!(value.to_ascii_lowercase().as_str(), "0" | "off" | "false")
        {
            return Self::disabled();
        }

        let path = std::env::var_os("TUPI_TELEMETRY_PATH")
            .map(PathBuf::from)
            .unwrap_or(default_path);
        let shell = std::env::var_os(SHELL_PATH_ENV)
            .map(PathBuf::from)
            .unwrap_or_else(|| shell_path_for(&path));

        Self::open_with_shell(path, shell)
    }

    pub fn is_enabled(&self) -> bool {
        self.inner.enabled.load(Ordering::Relaxed)
    }

    pub fn path(&self) -> Option<&Path> {
        self.inner.path.as_deref()
    }

    /// Caminho do arquivo bifurcado das execuções da tool `shell`, quando ligado.
    pub fn shell_path(&self) -> Option<&Path> {
        self.shell
            .as_deref()
            .and_then(|shell| shell.path.as_deref())
    }

    pub fn stats(&self) -> StatsSnapshot {
        StatsSnapshot {
            commands: self.inner.stats.commands.load(Ordering::Relaxed),
            queries: self.inner.stats.queries.load(Ordering::Relaxed),
            turns: self.inner.stats.turns.load(Ordering::Relaxed),
            provider_requests: self.inner.stats.provider_requests.load(Ordering::Relaxed),
            tool_calls: self.inner.stats.tool_calls.load(Ordering::Relaxed),
            tool_failures: self.inner.stats.tool_failures.load(Ordering::Relaxed),
            prompt_tokens: self.inner.stats.prompt_tokens.load(Ordering::Relaxed),
            cached_tokens: self.inner.stats.cached_tokens.load(Ordering::Relaxed),
            completion_tokens: self.inner.stats.completion_tokens.load(Ordering::Relaxed),
            provider_us: self.inner.stats.provider_us.load(Ordering::Relaxed),
            first_token_us: self.inner.stats.first_token_us.load(Ordering::Relaxed),
            tool_us: self.inner.stats.tool_us.load(Ordering::Relaxed),
            steps: self.inner.stats.steps.load(Ordering::Relaxed),
            errors: self.inner.stats.errors.load(Ordering::Relaxed),
            dropped: self.inner.stats.dropped.load(Ordering::Relaxed),
        }
    }

    /// Serializa e enfileira um evento no arquivo principal. As execuções da
    /// tool `shell` são copiadas também para o arquivo bifurcado. Nunca
    /// bloqueia; se a fila encher, a linha é contada como descartada.
    pub fn record(&self, event: TelemetryEvent) {
        if !self.is_enabled() {
            return;
        }

        event.absorb(&self.inner.stats);

        let value = match serde_json::to_value(&event) {
            Ok(value) => value,
            Err(_) => return,
        };
        let ts_ms = now_ms().min(u64::MAX as u128) as u64;

        // O principal recebe tudo; o bifurcado só as execuções da tool `shell`.
        write_line(&self.inner, &value, ts_ms);

        if is_shell_tool(&event)
            && let Some(shell) = self.shell.as_ref()
        {
            write_line(shell, &value, ts_ms);
        }
    }

    /// Evento de encerramento com os totais acumulados.
    pub fn run_finished(
        &self,
        started: Instant,
        reason: &'static str,
        error: Option<String>,
    ) -> TelemetryEvent {
        let stats = self.stats();
        let error_kind = error.as_ref().map(|_| "run".to_string());
        TelemetryEvent::RunFinished(RunFinished {
            duration_us: micros(started.elapsed()),
            reason: reason.to_string(),
            error,
            error_kind,
            total_commands: stats.commands,
            total_queries: stats.queries,
            total_turns: stats.turns,
            total_provider_requests: stats.provider_requests,
            total_tool_calls: stats.tool_calls,
            total_tool_failures: stats.tool_failures,
            total_prompt_tokens: stats.prompt_tokens,
            total_cached_tokens: stats.cached_tokens,
            total_completion_tokens: stats.completion_tokens,
            total_errors: stats.errors,
            dropped: stats.dropped,
        })
    }
}

#[derive(Serialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum TelemetryEvent {
    RunStarted(RunStarted),
    RunFinished(RunFinished),
    CommandStarted(CommandStarted),
    CommandFinished(CommandFinished),
    QueryStarted(QueryStarted),
    QueryFinished(QueryFinished),
    TurnStarted(TurnStarted),
    TurnFinished(TurnFinished),
    PromptSent(PromptSent),
    ProviderResponse(ProviderResponse),
    ContextTrimmed(ContextTrimmed),
    ToolStarted(ToolStarted),
    ToolFinished(ToolFinished),
    ModelsRefreshed(ModelsRefreshed),
}

impl TelemetryEvent {
    fn absorb(&self, stats: &Stats) {
        match self {
            TelemetryEvent::CommandStarted(_) => {
                stats.commands.fetch_add(1, Ordering::Relaxed);
            }
            TelemetryEvent::CommandFinished(event) if !event.ok => {
                stats.errors.fetch_add(1, Ordering::Relaxed);
            }
            TelemetryEvent::QueryStarted(_) => {
                stats.queries.fetch_add(1, Ordering::Relaxed);
            }
            TelemetryEvent::QueryFinished(event) if !event.ok => {
                stats.errors.fetch_add(1, Ordering::Relaxed);
            }
            TelemetryEvent::TurnStarted(_) => {
                stats.turns.fetch_add(1, Ordering::Relaxed);
            }
            TelemetryEvent::TurnFinished(event) => {
                stats.steps.fetch_add(event.steps as u64, Ordering::Relaxed);
            }
            TelemetryEvent::ProviderResponse(event) => {
                stats.provider_requests.fetch_add(1, Ordering::Relaxed);
                // O split provider×tool sai daqui: tempo do modelo separado do tempo de tool.
                stats
                    .provider_us
                    .fetch_add(event.duration_us, Ordering::Relaxed);
                add(&stats.first_token_us, event.first_token_us);
                add(&stats.prompt_tokens, event.prompt_tokens);
                add(&stats.cached_tokens, event.cached_tokens);
                add(&stats.completion_tokens, event.completion_tokens);
                if !event.ok {
                    stats.errors.fetch_add(1, Ordering::Relaxed);
                }
            }
            TelemetryEvent::ToolStarted(_) => {
                stats.tool_calls.fetch_add(1, Ordering::Relaxed);
            }
            TelemetryEvent::ToolFinished(event) => {
                if !event.ok {
                    stats.tool_failures.fetch_add(1, Ordering::Relaxed);
                }
                stats
                    .tool_us
                    .fetch_add(event.duration_us, Ordering::Relaxed);
            }
            TelemetryEvent::ModelsRefreshed(event) if !event.ok => {
                stats.errors.fetch_add(1, Ordering::Relaxed);
            }
            _ => {}
        }
    }
}

fn add(counter: &AtomicU64, value: Option<u64>) {
    if let Some(value) = value {
        counter.fetch_add(value, Ordering::Relaxed);
    }
}

#[derive(Serialize)]
pub struct RunStarted {
    pub version: String,
    pub model: String,
    pub provider: String,
    pub cwd: Option<String>,
    pub root: String,
    pub state_dir: String,
    pub telemetry_path: Option<String>,
    pub telemetry_shell_path: Option<String>,
    pub log_filter: String,
    pub max_tokens: usize,
    pub max_tool_steps: usize,
    pub context_budget: usize,
    pub prompt_chars: usize,
    pub tool_count: usize,
    pub tools: Vec<String>,
    pub workflow_count: usize,
    pub workflows: Vec<String>,
    pub store_opened: bool,
    pub store_path: String,
    pub store_error: Option<String>,
    pub store_error_kind: Option<String>,
}

#[derive(Serialize)]
pub struct RunFinished {
    pub duration_us: u64,
    pub reason: String,
    pub error: Option<String>,
    pub error_kind: Option<String>,
    pub total_commands: u64,
    pub total_queries: u64,
    pub total_turns: u64,
    pub total_provider_requests: u64,
    pub total_tool_calls: u64,
    pub total_tool_failures: u64,
    pub total_prompt_tokens: u64,
    pub total_cached_tokens: u64,
    pub total_completion_tokens: u64,
    pub total_errors: u64,
    pub dropped: u64,
}

#[derive(Serialize)]
pub struct CommandStarted {
    pub command: String,
    pub request: Option<u64>,
    pub text: Option<String>,
    pub text_chars: Option<usize>,
    pub workflow: Option<String>,
    pub extra: Option<String>,
    pub model: Option<String>,
}

#[derive(Serialize)]
pub struct CommandFinished {
    pub command: String,
    pub duration_us: u64,
    pub ok: bool,
    pub outcome: Option<String>,
    pub request: Option<u64>,
    pub model: Option<String>,
    /// Modo efetivo depois de `set_mode` (`execution` ou `sdd`).
    pub mode: Option<String>,
    pub enabled: Option<bool>,
    pub error: Option<String>,
    pub error_kind: Option<String>,
    pub http_status: Option<u16>,
}

#[derive(Serialize)]
pub struct QueryStarted {
    pub query: String,
}

#[derive(Serialize)]
pub struct QueryFinished {
    pub query: String,
    pub duration_us: u64,
    pub ok: bool,
    pub items: Option<usize>,
    pub error: Option<String>,
    pub error_kind: Option<String>,
}

#[derive(Serialize)]
pub struct TurnStarted {
    pub request: u64,
    pub text: String,
    pub text_chars: usize,
    pub model: String,
    pub history_messages: usize,
    pub history_estimate: usize,
    pub context_budget: usize,
}

#[derive(Serialize)]
pub struct TurnFinished {
    pub request: u64,
    pub duration_us: u64,
    pub ok: bool,
    pub cancelled: bool,
    pub steps: usize,
    pub tool_calls: usize,
    /// Soma das requisições do turno, não o contexto final.
    pub total_prompt_tokens: Option<u64>,
    /// Soma das requisições do turno, não o contexto final.
    pub total_cached_tokens: Option<u64>,
    /// Soma das requisições do turno, não o contexto final.
    pub total_completion_tokens: Option<u64>,
    pub error: Option<String>,
    pub error_kind: Option<String>,
}

#[derive(Serialize)]
pub struct PromptSent {
    pub request: u64,
    pub step: usize,
    pub model: String,
    pub body_hash: String,
    pub body: String,
    pub body_chars: usize,
    pub body_bytes: usize,
    /// `body` foi cortado pelo limite de preview.
    pub truncated: bool,
    /// `body` não foi gravado (flag de debug desligada).
    pub body_omitted: bool,
    pub estimate_tokens: usize,
    /// `estimate_tokens` corrigido pela escala observada; é o valor que se
    /// compara com `prompt_tokens`.
    pub estimate_scaled: usize,
    pub messages: usize,
    pub tools: usize,
    /// Nomes efetivamente enviados (o filtro por modo pode reduzir `tools`).
    pub tool_names: Vec<String>,
}

#[derive(Serialize)]
pub struct ProviderResponse {
    pub request: u64,
    pub step: usize,
    /// Estimativa do prompt feita antes do envio. Comparada com
    /// `prompt_tokens`, mostra quanto o template do provedor cobra além do
    /// que o histórico declara.
    pub estimate_tokens: usize,
    /// `estimate_tokens` corrigido pela escala observada; é o valor que se
    /// compara com `prompt_tokens`.
    pub estimate_scaled: usize,
    /// Correção de orçamento em vigor no passo (1_000_000 = sem correção).
    pub scale_ppm: u64,
    pub duration_us: u64,
    /// Tempo até o primeiro chunk do stream (`ProviderEvent::Started`).
    /// `None` quando o stream falha antes de produzir qualquer evento.
    pub first_token_us: Option<u64>,
    pub delta_count: usize,
    pub delta_chars: usize,
    pub tool_calls: usize,
    pub tool_call_names: Vec<String>,
    pub prompt_tokens: Option<u64>,
    pub cached_tokens: Option<u64>,
    pub completion_tokens: Option<u64>,
    /// Por que o provedor encerrou a resposta. `length`/`max_tokens` significa
    /// que ela foi cortada no teto; sem isso a truncagem fica invisível.
    pub finish_reason: Option<String>,
    pub ok: bool,
    pub error: Option<String>,
    pub error_kind: Option<String>,
    pub http_status: Option<u16>,
}

#[derive(Serialize)]
pub struct ContextTrimmed {
    pub request: u64,
    pub step: usize,
    pub estimate: usize,
    pub omitted_units: usize,
    pub shrunk_results: usize,
}

#[derive(Serialize)]
pub struct ToolStarted {
    pub request: u64,
    pub call: String,
    pub name: String,
    pub arguments: String,
    pub arguments_chars: usize,
    pub arguments_bytes: usize,
    pub truncated: bool,
    pub summary: String,
}

#[derive(Serialize)]
pub struct ToolFinished {
    pub request: u64,
    pub call: String,
    pub name: String,
    pub duration_us: u64,
    pub ok: bool,
    pub result: Option<String>,
    pub result_chars: usize,
    pub result_bytes: usize,
    pub truncated: bool,
    pub summary: String,
    pub error: Option<String>,
}

#[derive(Serialize)]
pub struct ModelsRefreshed {
    pub source: String,
    pub count: usize,
    pub models: Vec<String>,
    pub duration_us: u64,
    pub ok: bool,
    pub error: Option<String>,
    pub error_kind: Option<String>,
    pub http_status: Option<u16>,
}

fn mode_label(mode: tupi_kernel::AgentMode) -> &'static str {
    match mode {
        tupi_kernel::AgentMode::Execution => "execution",
        tupi_kernel::AgentMode::Sdd => "sdd",
        tupi_kernel::AgentMode::Docs => "docs",
    }
}

/// Dados do comando que precisam existir antes e depois do dispatch.
pub struct CommandInfo {
    command: String,
    request: Option<u64>,
    text: Option<String>,
    workflow: Option<String>,
    extra: Option<String>,
    model: Option<String>,
}

impl CommandInfo {
    pub fn new(command: &Command) -> Self {
        match command {
            Command::SendMessage { text } => Self {
                command: "send_message".to_string(),
                request: None,
                text: Some(text.clone()),
                workflow: None,
                extra: None,
                model: None,
            },
            Command::CancelStream { request_id } => Self {
                command: "cancel_stream".to_string(),
                request: Some(*request_id),
                text: None,
                workflow: None,
                extra: None,
                model: None,
            },
            Command::SetModel { model } => Self {
                command: "set_model".to_string(),
                request: None,
                text: None,
                workflow: None,
                extra: None,
                model: Some(model.clone()),
            },
            Command::ToggleShowPrompts => Self {
                command: "toggle_show_prompts".to_string(),
                request: None,
                text: None,
                workflow: None,
                extra: None,
                model: None,
            },
            Command::ToggleMode | Command::SetMode { .. } => Self {
                command: "set_mode".to_string(),
                request: None,
                text: None,
                workflow: None,
                extra: None,
                model: None,
            },
            Command::RefreshModels => Self {
                command: "refresh_models".to_string(),
                request: None,
                text: None,
                workflow: None,
                extra: None,
                model: None,
            },
            Command::NewSession => Self {
                command: "new_session".to_string(),
                request: None,
                text: None,
                workflow: None,
                extra: None,
                model: None,
            },
            Command::RunWorkflow { name, extra } => Self {
                command: "run_workflow".to_string(),
                request: None,
                text: None,
                workflow: Some(name.clone()),
                extra: Some(extra.clone()),
                model: None,
            },
            Command::Connect { provider } => Self {
                command: "connect".to_string(),
                request: None,
                text: None,
                workflow: None,
                extra: Some(provider.clone()),
                model: None,
            },
            Command::Memory { .. } => Self {
                command: "memory".to_string(),
                request: None,
                text: None,
                workflow: None,
                extra: None,
                model: None,
            },
            Command::ResumeSdd { slug } => Self {
                command: "resume_sdd".to_string(),
                request: None,
                text: None,
                workflow: None,
                extra: Some(slug.clone()),
                model: None,
            },
            Command::ListSessions => Self {
                command: "list_sessions".to_string(),
                request: None,
                text: None,
                workflow: None,
                extra: None,
                model: None,
            },
            Command::ResumeSession { id } => Self {
                command: "resume_session".to_string(),
                request: None,
                text: None,
                workflow: None,
                extra: Some(id.clone()),
                model: None,
            },
            Command::DeleteSession { id } => Self {
                command: "delete_session".to_string(),
                request: None,
                text: None,
                workflow: None,
                extra: Some(id.clone()),
                model: None,
            },
        }
    }

    pub fn started(&self) -> TelemetryEvent {
        TelemetryEvent::CommandStarted(CommandStarted {
            command: self.command.clone(),
            request: self.request,
            text: self.text.clone(),
            text_chars: self.text.as_ref().map(|text| text.chars().count()),
            workflow: self.workflow.clone(),
            extra: self.extra.clone(),
            model: self.model.clone(),
        })
    }

    pub fn finished(
        &self,
        duration: Duration,
        result: &Result<CommandOutcome, CommandError>,
    ) -> TelemetryEvent {
        let mut finished = CommandFinished {
            command: self.command.clone(),
            duration_us: micros(duration),
            ok: true,
            outcome: None,
            request: None,
            model: None,
            mode: None,
            enabled: None,
            error: None,
            error_kind: None,
            http_status: None,
        };

        match result {
            Ok(outcome) => {
                finished.outcome = Some(
                    match outcome {
                        CommandOutcome::Started { .. } => "started",
                        CommandOutcome::Cancelled => "cancelled",
                        CommandOutcome::ModelChanged { .. } => "model_changed",
                        CommandOutcome::ShowPrompts { .. } => "show_prompts",
                        CommandOutcome::ModeChanged { .. } => "mode_changed",
                        CommandOutcome::SessionReset => "session_reset",
                        CommandOutcome::Memory { .. } => "memory",
                        CommandOutcome::MemoryToggled { .. } => "memory_toggled",
                        CommandOutcome::MemoryForgotten { .. } => "memory_forgotten",
                        CommandOutcome::SddResumed { .. } => "sdd_resumed",
                        CommandOutcome::SessionsList { .. } => "sessions_list",
                        CommandOutcome::SessionResumed { .. } => "session_resumed",
                        CommandOutcome::SessionDeleted { .. } => "session_deleted",
                        CommandOutcome::Noop => "noop",
                    }
                    .to_string(),
                );
                match outcome {
                    CommandOutcome::Started { request_id } => {
                        finished.request = Some(*request_id);
                    }
                    CommandOutcome::ModelChanged { model } => {
                        finished.model = Some(model.clone());
                    }
                    CommandOutcome::ShowPrompts { enabled } => {
                        finished.enabled = Some(*enabled);
                    }
                    CommandOutcome::MemoryToggled { enabled } => {
                        finished.enabled = Some(*enabled);
                    }
                    CommandOutcome::ModeChanged { mode } => {
                        finished.mode = Some(mode_label(*mode).to_string());
                    }
                    CommandOutcome::SddResumed { mode, .. } => {
                        finished.mode = Some(mode_label(*mode).to_string());
                    }
                    CommandOutcome::SessionResumed { mode, .. } => {
                        finished.mode = Some(mode_label(*mode).to_string());
                    }
                    CommandOutcome::Cancelled
                    | CommandOutcome::Memory { .. }
                    | CommandOutcome::MemoryForgotten { .. }
                    | CommandOutcome::SessionReset
                    | CommandOutcome::SessionsList { .. }
                    | CommandOutcome::SessionDeleted { .. }
                    | CommandOutcome::Noop => {}
                }
            }
            Err(error) => {
                finished.ok = false;
                finished.error = Some(error.to_string());
                finished.error_kind = Some(command_error_kind(error).to_string());
                finished.http_status = command_http_status(error);
            }
        }

        TelemetryEvent::CommandFinished(finished)
    }
}

pub struct QueryInfo {
    query: String,
}

impl QueryInfo {
    pub fn new(query: &Query) -> Self {
        Self {
            query: match query {
                Query::History => "history",
                Query::ListProviders => "list_providers",
                Query::ListModels => "list_models",
                Query::ListWorkflows => "list_workflows",
                Query::ListSddSpecs => "list_sdd_specs",
            }
            .to_string(),
        }
    }

    pub fn started(&self) -> TelemetryEvent {
        TelemetryEvent::QueryStarted(QueryStarted {
            query: self.query.clone(),
        })
    }

    pub fn finished(
        &self,
        duration: Duration,
        result: &Result<QueryResult, QueryError>,
    ) -> TelemetryEvent {
        let mut finished = QueryFinished {
            query: self.query.clone(),
            duration_us: micros(duration),
            ok: true,
            items: None,
            error: None,
            error_kind: None,
        };

        match result {
            Ok(result) => {
                finished.items = Some(match result {
                    QueryResult::History(messages) => messages.len(),
                    QueryResult::Providers(providers) => providers.len(),
                    QueryResult::Models(models) => models.len(),
                    QueryResult::Workflows(workflows) => workflows.len(),
                    QueryResult::SddSpecs(specs) => specs.len(),
                });
            }
            Err(error) => {
                finished.ok = false;
                finished.error = Some(error.to_string());
                finished.error_kind = Some(query_error_kind(error).to_string());
            }
        }

        TelemetryEvent::QueryFinished(finished)
    }
}

pub fn command_error_kind(error: &CommandError) -> &'static str {
    match error {
        CommandError::Busy => "busy",
        CommandError::UnknownRequest => "unknown_request",
        CommandError::Provider(_) => "provider",
        CommandError::Internal(_) => "internal",
    }
}

pub fn query_error_kind(error: &QueryError) -> &'static str {
    match error {
        QueryError::Provider(_) => "provider",
        QueryError::Internal(_) => "internal",
    }
}

pub fn command_http_status(error: &CommandError) -> Option<u16> {
    match error {
        CommandError::Provider(error) => provider_http_status(error),
        _ => None,
    }
}

pub fn provider_error_kind(error: &ProviderError) -> &'static str {
    match error {
        ProviderError::Network(_) => "network",
        ProviderError::Http { .. } => "http",
        ProviderError::Protocol(_) => "protocol",
        ProviderError::RateLimited { .. } => "rate_limited",
        ProviderError::QuotaExhausted { .. } => "quota_exhausted",
        ProviderError::InvalidArgument { .. } => "invalid_argument",
    }
}

pub fn provider_http_status(error: &ProviderError) -> Option<u16> {
    match error {
        ProviderError::Http { status, .. }
        | ProviderError::RateLimited { status, .. }
        | ProviderError::QuotaExhausted { status, .. }
        | ProviderError::InvalidArgument { status, .. } => Some(*status),
        _ => None,
    }
}

pub fn store_error_kind(error: &tupi_store::StoreError) -> &'static str {
    match error {
        tupi_store::StoreError::Io(_) => "io",
        tupi_store::StoreError::Database(_) => "database",
        tupi_store::StoreError::Transaction(_) => "transaction",
        tupi_store::StoreError::Table(_) => "table",
        tupi_store::StoreError::FutureSchema { .. } => "future_schema",
        tupi_store::StoreError::Corrupt(_) => "corrupt",
    }
}

pub struct Clipped {
    pub text: String,
    pub chars: usize,
    pub bytes: usize,
    pub truncated: bool,
    /// O corpo não foi gravado (flag de debug desligada), não apenas cortado.
    pub omitted: bool,
}

/// Corta um texto em MAX_TEXT_BYTES, sempre em limite de caractere UTF-8.
pub fn clip(text: &str) -> Clipped {
    clip_to(text, MAX_TEXT_BYTES)
}

/// Corpo do prompt na telemetria. Por padrão fica vazio: reescrever o
/// histórico inteiro a cada passo domina o arquivo e o rodízio apaga o começo
/// das sessões longas. Ligue com `TUPI_TELEMETRY_BODY=1` para depurar o
/// payload literal.
pub fn prompt_body(text: &str) -> Clipped {
    let requested = std::env::var("TUPI_TELEMETRY_BODY")
        .ok()
        .is_some_and(|value| body_requested(&value));
    prompt_body_with(text, requested)
}

fn body_requested(value: &str) -> bool {
    matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "1" | "on" | "true" | "yes"
    )
}

fn prompt_body_with(text: &str, include: bool) -> Clipped {
    if include {
        return clip_preview(text);
    }

    Clipped {
        text: String::new(),
        chars: text.chars().count(),
        bytes: text.len(),
        truncated: false,
        omitted: !text.is_empty(),
    }
}

pub fn clip_preview(text: &str) -> Clipped {
    clip_to(text, PROMPT_PREVIEW_BYTES)
}

/// O envelope de truncamento das tools conta como resultado truncado,
/// mesmo quando o payload inteiro coube no clip.
pub fn truncated(content: &str, clipped: bool) -> bool {
    clipped || content.contains("Resposta truncada")
}

fn clip_to(text: &str, max_bytes: usize) -> Clipped {
    let bytes = text.len();
    let chars = text.chars().count();

    if bytes <= max_bytes {
        return Clipped {
            text: text.to_string(),
            chars,
            bytes,
            truncated: false,
            omitted: false,
        };
    }

    let mut end = max_bytes;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }

    Clipped {
        text: text[..end].to_string(),
        chars,
        bytes,
        truncated: true,
        omitted: false,
    }
}

pub fn text_hash(text: &str) -> String {
    let hex = blake3::hash(text.as_bytes()).to_hex();
    hex.as_str()[..12].to_string()
}

pub fn micros(duration: Duration) -> u64 {
    duration.as_micros().min(u64::MAX as u128) as u64
}

fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis())
        .unwrap_or(0)
}

fn run_id() -> String {
    format!("{}-{}", now_ms(), std::process::id())
}

/// Arquivo bifurcado que acompanha o principal: `telemetry.jsonl` vira
/// `telemetry.shell.jsonl` no mesmo diretório.
pub fn shell_path_for(main: &Path) -> PathBuf {
    main.with_file_name(SHELL_FILE)
}

fn create_dir(path: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        let mut builder = std::fs::DirBuilder::new();
        builder.recursive(true).mode(0o700);
        builder.create(path)
    }
    #[cfg(not(unix))]
    {
        std::fs::create_dir_all(path)
    }
}

/// Um evento só vai para o arquivo bifurcado quando vem da tool `shell`.
fn is_shell_tool(event: &TelemetryEvent) -> bool {
    match event {
        TelemetryEvent::ToolStarted(started) => started.name == SHELL_TOOL,
        TelemetryEvent::ToolFinished(finished) => finished.name == SHELL_TOOL,
        _ => false,
    }
}

/// Serializa `base` com o envelope daquele sink e enfileira. O `seq` é por
/// arquivo; `ts_ms` e `run` são iguais nas duas cópias.
fn write_line(inner: &Inner, base: &serde_json::Value, ts_ms: u64) {
    let Some(sender) = inner.sender.as_ref() else {
        return;
    };

    let mut seq = inner
        .seq
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let next = *seq + 1;

    let mut value = base.clone();
    if let Some(map) = value.as_object_mut() {
        map.insert("schema".to_string(), serde_json::json!(SCHEMA));
        map.insert("ts_ms".to_string(), serde_json::json!(ts_ms));
        map.insert("seq".to_string(), serde_json::json!(next));
        map.insert("run".to_string(), serde_json::json!(inner.run));
    }

    let mut bytes = match serde_json::to_vec(&value) {
        Ok(bytes) => bytes,
        Err(_) => return,
    };
    bytes.push(b'\n');

    match sender.try_send(bytes) {
        Ok(()) => *seq = next,
        Err(TrySendError::Full(_)) => {
            inner.stats.dropped.fetch_add(1, Ordering::Relaxed);
        }
        Err(TrySendError::Disconnected(_)) => {
            inner.enabled.store(false, Ordering::Relaxed);
            inner.stats.dropped.fetch_add(1, Ordering::Relaxed);
        }
    }
}

fn open_file(path: &Path) -> std::io::Result<File> {
    let mut options = OpenOptions::new();
    options.create(true).append(true);

    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }

    let file = options.open(path)?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    }

    Ok(file)
}

/// Cria o diretório, corta o arquivo e sobe a thread de escrita de um sink.
/// Qualquer falha devolve `None` e desliga só aquele arquivo.
fn open_sink(path: &Path, run: &str, thread: &str) -> Option<Arc<Inner>> {
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
        && let Err(error) = create_dir(parent)
    {
        tracing::warn!(%error, path = %parent.display(), "telemetria desligada");
        return None;
    }

    if let Err(error) = trim(path) {
        tracing::warn!(%error, path = %path.display(), "falha ao cortar telemetria");
    }

    let file = match open_file(path) {
        Ok(file) => file,
        Err(error) => {
            tracing::warn!(%error, path = %path.display(), "telemetria desligada");
            return None;
        }
    };

    let (sender, receiver) = sync_channel(CHANNEL_CAPACITY);
    let writer_path = path.to_path_buf();
    let joiner = std::thread::Builder::new()
        .name(thread.to_string())
        .spawn(move || writer(file, writer_path, receiver));

    let joiner = match joiner {
        Ok(joiner) => joiner,
        Err(error) => {
            tracing::warn!(%error, "telemetria desligada");
            return None;
        }
    };

    Some(Arc::new(Inner {
        sender: Some(sender),
        joiner: Mutex::new(Some(joiner)),
        seq: Mutex::new(0),
        run: run.to_string(),
        enabled: AtomicBool::new(true),
        path: Some(path.to_path_buf()),
        stats: Stats::default(),
    }))
}

fn writer(file: File, path: PathBuf, receiver: Receiver<Vec<u8>>) {
    let mut file = file;
    let mut size = file.metadata().map(|metadata| metadata.len()).unwrap_or(0);
    // teto do arquivo: 96 MiB (32 MiB × 3), abaixo dos 100 MB pedidos
    let cap = ROTATE_BYTES * ROTATE_KEEP as u64;

    while let Ok(line) = receiver.recv() {
        if let Err(error) = file.write_all(&line).and_then(|_| file.flush()) {
            tracing::warn!(%error, "falha ao gravar telemetria; desligando");
            break;
        }

        size += line.len() as u64;
        if size <= cap {
            continue;
        }

        // passou do teto: descarta os registros mais antigos e reabre
        match trim(&path) {
            Ok(trimmed) => size = trimmed,
            Err(error) => {
                tracing::warn!(%error, path = %path.display(), "falha ao cortar telemetria");
                break;
            }
        }

        match open_file(&path) {
            Ok(reopened) => file = reopened,
            Err(error) => {
                tracing::warn!(%error, path = %path.display(), "telemetria desligada");
                break;
            }
        }
    }
}

/// Corta os registros mais antigos quando o `telemetry.jsonl` passa do teto de
/// 96 MiB (32 MiB × 3), mantendo o arquivo abaixo dos 100 MB.
fn trim(path: &Path) -> std::io::Result<u64> {
    trim_at(path, ROTATE_BYTES * ROTATE_KEEP as u64)
}

/// Mantém o arquivo abaixo de `max_bytes` descartando os registros mais antigos.
///
/// Devolve o tamanho final do arquivo.
fn trim_at(path: &Path, max_bytes: u64) -> std::io::Result<u64> {
    use std::io::{Read, Seek, SeekFrom};

    let len = match std::fs::metadata(path) {
        Ok(metadata) => metadata.len(),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(error) => return Err(error),
    };

    if len <= max_bytes {
        return Ok(len);
    }

    // sobram só os últimos `max_bytes / 2` bytes, para não reescrever o arquivo
    // a cada evento gravado
    let keep = max_bytes / 2;
    let mut file = File::open(path)?;
    file.seek(SeekFrom::Start(len - keep))?;
    let mut tail = Vec::with_capacity(keep as usize);
    file.read_to_end(&mut tail)?;

    // o recorte pode cair no meio de uma linha: descarta a primeira
    let start = tail
        .iter()
        .position(|byte| *byte == b'\n')
        .map_or(tail.len(), |at| at + 1);
    let tail = &tail[start..];

    let mut temporary = path.as_os_str().to_owned();
    temporary.push(".tmp");
    let temporary = PathBuf::from(temporary);
    std::fs::write(&temporary, tail)?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&temporary, std::fs::Permissions::from_mode(0o600));
    }

    std::fs::rename(&temporary, path)?;
    Ok(tail.len() as u64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    fn line_events(path: &Path) -> Vec<serde_json::Value> {
        let mut body = String::new();
        File::open(path)
            .expect("arquivo de telemetria")
            .read_to_string(&mut body)
            .expect("leitura");
        body.lines()
            .map(|line| serde_json::from_str(line).expect("linha json"))
            .collect()
    }

    fn event(name: &str) -> TelemetryEvent {
        TelemetryEvent::QueryStarted(QueryStarted {
            query: name.to_string(),
        })
    }

    #[test]
    fn writes_one_json_object_per_line_with_envelope() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("telemetry.jsonl");
        let telemetry = Telemetry::open(&path);

        telemetry.record(event("history"));
        telemetry.record(event("list_models"));
        drop(telemetry);

        let lines = line_events(&path);
        assert_eq!(lines.len(), 2);
        for (index, line) in lines.iter().enumerate() {
            assert_eq!(line["schema"], SCHEMA);
            assert!(line["ts_ms"].as_u64().is_some());
            assert_eq!(line["seq"], index as u64 + 1);
            assert!(line["run"].as_str().is_some_and(|run| !run.is_empty()));
            assert_eq!(line["event"], "query_started");
        }
    }

    #[test]
    fn disabled_writes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("telemetry.jsonl");
        let telemetry = Telemetry::disabled();

        telemetry.record(event("history"));
        drop(telemetry);

        assert!(!path.exists());
    }

    #[test]
    fn clip_keeps_utf8_boundary_and_reports_original_size() {
        let text = "á".repeat(MAX_TEXT_BYTES);
        let clipped = clip(&text);

        assert!(clipped.truncated);
        assert_eq!(clipped.bytes, text.len());
        assert_eq!(clipped.chars, text.chars().count());
        assert!(clipped.text.len() <= MAX_TEXT_BYTES);
        assert!(text.starts_with(&clipped.text));
    }

    #[test]
    fn prompt_preview_is_smaller_than_the_full_body() {
        let text = "x".repeat(PROMPT_PREVIEW_BYTES * 2);
        let clipped = clip_preview(&text);

        assert!(clipped.truncated);
        assert_eq!(clipped.bytes, text.len());
        assert_eq!(clipped.text.len(), PROMPT_PREVIEW_BYTES);
    }

    #[test]
    fn prompt_body_is_omitted_unless_requested() {
        let text = "sistema\nusuário";
        let hidden = prompt_body_with(text, false);
        assert!(hidden.text.is_empty());
        assert_eq!(hidden.chars, text.chars().count());
        assert_eq!(hidden.bytes, text.len());
        assert!(hidden.omitted);
        assert!(!hidden.truncated);

        let shown = prompt_body_with(text, true);
        assert_eq!(shown.text, text);
        assert!(!shown.omitted);
        assert!(!shown.truncated);
    }

    #[test]
    fn a_mode_change_records_the_mode() {
        let info = CommandInfo::new(&Command::SetMode {
            mode: tupi_kernel::AgentMode::Sdd,
        });
        let event = info.finished(
            Duration::ZERO,
            &Ok(CommandOutcome::ModeChanged {
                mode: tupi_kernel::AgentMode::Sdd,
            }),
        );

        let value = serde_json::to_value(event).unwrap();
        assert_eq!(value["mode"], "sdd");
    }

    #[test]
    fn body_flag_accepts_truthy_values() {
        for value in ["1", "on", "TRUE", "yes"] {
            assert!(body_requested(value), "{value}");
        }
        for value in ["0", "off", "false", ""] {
            assert!(!body_requested(value), "{value}");
        }
    }

    #[test]
    fn text_hash_is_short_and_deterministic() {
        let a = text_hash("abc");

        assert_eq!(a, text_hash("abc"));
        assert_eq!(a.len(), 12);
        assert_ne!(a, text_hash("abd"));
    }

    #[test]
    fn an_invalid_path_degrades_to_disabled_instead_of_panicking() {
        let dir = tempfile::tempdir().unwrap();
        let telemetry = Telemetry::open(dir.path());

        assert!(!telemetry.is_enabled());
        telemetry.record(event("history"));
    }

    #[test]
    fn trimming_drops_the_oldest_records() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("telemetry.jsonl");
        let mut body = String::new();
        while body.len() < 4_000 {
            body.push_str(&format!("{{\"n\":{}}}\n", body.len()));
        }
        std::fs::write(&path, &body).unwrap();

        let size = trim_at(&path, 600).unwrap();
        let kept = std::fs::read_to_string(&path).unwrap();

        assert!(size <= 300, "cortou até a metade do teto");
        assert_eq!(size, kept.len() as u64);
        assert!(
            body.ends_with(&kept),
            "sobram só os registros mais recentes"
        );
        assert!(!dir.path().join("telemetry.jsonl.tmp").exists());

        let first = kept.lines().next().unwrap();
        assert!(serde_json::from_str::<serde_json::Value>(first).is_ok());

        // arquivo abaixo do teto fica intacto
        let small = dir.path().join("small.jsonl");
        std::fs::write(&small, "{\"a\":1}\n").unwrap();
        assert_eq!(trim_at(&small, 600).unwrap(), 8);
        assert_eq!(std::fs::read_to_string(&small).unwrap(), "{\"a\":1}\n");

        // caminho inexistente não é erro
        assert_eq!(trim_at(&dir.path().join("nada.jsonl"), 600).unwrap(), 0);
    }

    #[test]
    fn stats_count_commands_queries_tools_and_errors() {
        let dir = tempfile::tempdir().unwrap();
        let telemetry = Telemetry::open(dir.path().join("telemetry.jsonl"));

        telemetry.record(event("history"));
        telemetry.record(TelemetryEvent::ToolFinished(ToolFinished {
            request: 1,
            call: "c1".to_string(),
            name: "read".to_string(),
            duration_us: 1,
            ok: false,
            result: None,
            result_chars: 0,
            result_bytes: 0,
            truncated: false,
            summary: String::new(),
            error: Some("falhou".to_string()),
        }));
        telemetry.record(TelemetryEvent::ProviderResponse(ProviderResponse {
            request: 1,
            step: 1,
            estimate_tokens: 10,
            estimate_scaled: 10,
            scale_ppm: 1_000_000,
            duration_us: 1,
            first_token_us: None,
            delta_count: 0,
            delta_chars: 0,
            tool_calls: 0,
            tool_call_names: Vec::new(),
            prompt_tokens: Some(10),
            cached_tokens: Some(2),
            completion_tokens: Some(3),
            finish_reason: Some("stop".to_string()),
            ok: true,
            error: None,
            error_kind: None,
            http_status: None,
        }));

        let stats = telemetry.stats();
        assert_eq!(stats.queries, 1);
        assert_eq!(stats.tool_failures, 1);
        assert_eq!(stats.provider_requests, 1);
        assert_eq!(stats.prompt_tokens, 10);
        assert_eq!(stats.cached_tokens, 2);
        assert_eq!(stats.completion_tokens, 3);
    }

    #[test]
    fn stats_sum_provider_and_tool_time_across_events() {
        let dir = tempfile::tempdir().unwrap();
        let telemetry = Telemetry::open(dir.path().join("telemetry.jsonl"));

        let response = |duration_us: u64, first_token_us: Option<u64>| {
            TelemetryEvent::ProviderResponse(ProviderResponse {
                request: 1,
                step: 1,
                estimate_tokens: 10,
                estimate_scaled: 10,
                scale_ppm: 1_000_000,
                duration_us,
                first_token_us,
                delta_count: 0,
                delta_chars: 0,
                tool_calls: 0,
                tool_call_names: Vec::new(),
                prompt_tokens: None,
                cached_tokens: None,
                completion_tokens: None,
                finish_reason: Some("stop".to_string()),
                ok: true,
                error: None,
                error_kind: None,
                http_status: None,
            })
        };
        let tool = |duration_us: u64, ok: bool| {
            TelemetryEvent::ToolFinished(ToolFinished {
                request: 1,
                call: "c1".to_string(),
                name: "read".to_string(),
                duration_us,
                ok,
                result: None,
                result_chars: 0,
                result_bytes: 0,
                truncated: false,
                summary: String::new(),
                error: (!ok).then(|| "falhou".to_string()),
            })
        };

        telemetry.record(response(1_000, Some(200)));
        telemetry.record(response(3_000, Some(400)));
        // Requisição que falhou antes do primeiro chunk não soma `first_token_us`.
        telemetry.record(response(500, None));
        telemetry.record(tool(150, true));
        telemetry.record(tool(50, false));
        telemetry.record(TelemetryEvent::TurnFinished(TurnFinished {
            request: 1,
            duration_us: 5_000,
            ok: true,
            cancelled: false,
            steps: 4,
            tool_calls: 2,
            total_prompt_tokens: None,
            total_cached_tokens: None,
            total_completion_tokens: None,
            error: None,
            error_kind: None,
        }));

        let stats = telemetry.stats();
        assert_eq!(stats.provider_requests, 3);
        assert_eq!(stats.provider_us, 4_500);
        assert_eq!(stats.first_token_us, 600);
        assert_eq!(stats.tool_us, 200);
        assert_eq!(stats.tool_failures, 1);
        assert_eq!(stats.steps, 4);
    }

    #[test]
    fn the_truncation_envelope_counts_as_truncated() {
        assert!(truncated("> Resposta truncada: 2 seções omitidas", false));
        assert!(truncated("texto", true));
        assert!(!truncated("texto", false));
    }

    fn tool_started(name: &str) -> TelemetryEvent {
        TelemetryEvent::ToolStarted(ToolStarted {
            request: 1,
            call: "c1".to_string(),
            name: name.to_string(),
            arguments: "{}".to_string(),
            arguments_chars: 2,
            arguments_bytes: 2,
            truncated: false,
            summary: String::new(),
        })
    }

    #[test]
    fn shell_executions_are_forked_to_their_own_file() {
        let dir = tempfile::tempdir().unwrap();
        let main = dir.path().join("telemetry.jsonl");
        let shell = dir.path().join(SHELL_FILE);
        let telemetry = Telemetry::open_with_shell(&main, &shell);

        telemetry.record(event("history"));
        telemetry.record(tool_started(SHELL_TOOL));
        telemetry.record(tool_started("read"));
        drop(telemetry);

        let main = line_events(&main);
        let forked = line_events(&shell);
        assert_eq!(main.len(), 3);
        assert_eq!(forked.len(), 1);
        assert_eq!(forked[0]["event"], "tool_started");
        assert_eq!(forked[0]["seq"], 1);
        assert_eq!(forked[0]["schema"], SCHEMA);
        assert_eq!(forked[0]["run"], main[0]["run"]);
    }

    #[test]
    fn the_shell_fork_is_a_sibling_of_the_main_file() {
        let path = shell_path_for(Path::new("/repo/.agents/tupi/debug/telemetry.jsonl"));
        assert_eq!(
            path,
            PathBuf::from("/repo/.agents/tupi/debug/telemetry.shell.jsonl")
        );
    }
}
