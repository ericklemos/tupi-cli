//! Config do modo SDD, embutida no binário em tempo de compilação.
//!
//! A fonte é `configs/modes/sdd.ron`, parseada pelo `build.rs` deste crate. O
//! resultado daqui é sempre `&'static`: nenhuma leitura de arquivo acontece no
//! caminho quente do turno.

use std::sync::OnceLock;

/// Prompt e comportamento do modo SDD em quatro blocos.
pub struct SddConfig {
    pub planning: SddPlanning,
    pub execution: SddExecution,
    pub tool: SddToolConfig,
    pub runner: SddRunner,
}

/// Bloco `planning`: postura, allowlist de tools e trava do modo.
pub struct SddPlanning {
    /// Postura anexada ao system prompt no modo SDD. Vazio: nada é anexado.
    pub posture: &'static str,
    /// Allowlist por nome. Vazia: vale só a máscara de modo de cada tool.
    pub tools: &'static [&'static str],
    /// Trava o modo ao abrir o plano.
    pub lock_mode_on_open: bool,
    /// Budget default das tools do modo. `None` usa o padrão global.
    pub tool_budget_tokens: Option<usize>,
}

/// Bloco `execution`: textos dos agentes e do handoff.
pub struct SddExecution {
    pub agent_prefix: &'static str,
    pub docs_agent_prefix: &'static str,
    pub subtask_brief: &'static str,
    pub docs_brief: &'static str,
    pub handoff_open: &'static str,
    pub handoff_done: &'static str,
    pub run_report: &'static str,
}

/// Bloco `tool`: contrato da tool `sdd` por modo.
pub struct SddToolConfig {
    pub planning_actions: &'static [&'static str],
    pub execution_actions: &'static [&'static str],
    pub planning_description: &'static str,
    pub execution_description: &'static str,
}

/// Bloco `runner`: parâmetros do runner automático de subtasks.
pub struct SddRunner {
    pub max_parallel: usize,
    pub agent_max_steps: usize,
    pub agent_timeout_secs: u64,
    pub auto_execute: bool,
    pub docs_agent: bool,
    pub mirror_markdown: bool,
}

mod embedded {
    include!(concat!(env!("OUT_DIR"), "/sdd_config_generated.rs"));
}

static CONFIG: OnceLock<SddConfig> = OnceLock::new();

/// Config do modo SDD, resolvida uma única vez por processo.
pub fn sdd_config() -> &'static SddConfig {
    CONFIG.get_or_init(embedded::embedded)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_embedded_config_is_loaded() {
        let config = sdd_config();
        assert!(config.planning.posture.contains("Modo SDD"));
        assert!(config.execution.agent_prefix.contains("agente de implementação"));
        assert!(config.tool.planning_actions.contains(&"set_tasks"));
        assert!(config.runner.max_parallel >= 1);
    }

    #[test]
    fn the_sdd_tool_is_in_the_mode_allowlist() {
        assert!(sdd_config().planning.tools.contains(&"sdd"));
        assert!(sdd_config().planning.tools.contains(&"read"));
        assert!(!sdd_config().planning.tools.contains(&"edit"));
    }
}
