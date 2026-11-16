//! Entrada pública para rodar um plano SDD com agentes de implementação.
//!
//! O binário implementa `SddExecutor` e chama `execute` de dentro do
//! `SddTool::close`; o App não precisa conhecer o orquestrador.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::time::Duration;

use tupi_kernel::{EventBus, Provider, ToolRegistry};
use tupi_tools::sdd::SddStore;

use crate::sdd_run::{RunContext, run_plan};

/// Prefixo mínimo do agente de implementação.
pub const AGENT_PREFIX: &str = r#"Você é um agente de implementação do tupi.

Implemente exatamente a subtarefa descrita. Não edite o plano nem chame sdd.
Outros agentes podem estar trabalhando em paralelo em outras partes do
repositório: trate o que já existe como verdade e não toque no que pertence a
outra subtarefa. Use read/edit/exec como de costume.

Ao terminar, responda em até 3 linhas: o que mudou, arquivos e como verificou."#;

/// Pedido de execução de um plano fechado.
pub struct SddExecRequest {
    pub root: PathBuf,
    pub store: Arc<dyn SddStore>,
    pub repo_key: String,
    pub tools: ToolRegistry,
    pub agent_tools: Arc<dyn Fn() -> ToolRegistry + Send + Sync>,
    pub docs_agent: Option<crate::sdd_run::DocsAgentFactory>,
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
pub struct SddExecOutcome {
    pub ok: bool,
    pub report: String,
}

/// Executa o plano fechado. Devolve o relatório mesmo em falha.
pub async fn execute(request: SddExecRequest, slug: &str) -> SddExecOutcome {
    // A execução roda em `spawn_blocking` — o abort da task do stream não a
    // alcança —, então o cancelamento tem de vir da tool em curso: é a mesma
    // flag que o ESC do turno aciona.
    let cancel = tupi_kernel::current_tool_cancel().unwrap_or(request.cancel);
    let ctx = RunContext {
        store: request.store,
        repo_key: request.repo_key,
        tools: request.tools,
        agent_tools: request.agent_tools,
        docs_agent: request.docs_agent,
        provider: request.provider,
        model: request.model,
        agent_prefix: request.agent_prefix,
        max_parallel: request.max_parallel,
        agent_max_steps: request.agent_max_steps,
        max_tokens: request.max_tokens,
        agent_timeout: request.agent_timeout,
        events: request.events,
        request_id: request.request_id,
        cancel,
        in_flight: request.in_flight,
        in_flight_count: request.in_flight_count,
    };
    let outcome = run_plan(&ctx, slug).await;
    SddExecOutcome {
        ok: outcome.ok,
        report: outcome.report,
    }
}
