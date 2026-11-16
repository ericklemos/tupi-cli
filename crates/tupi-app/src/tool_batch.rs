//! Regras de execução de um passo que traz várias tool calls de uma vez.
//!
//! Rodar todas em série paga N vezes uma execução local que não tem dependência
//! entre si; rodar todas juntas troca a ordem declarada, que é contrato
//! (história, eventos da TUI e testes). A regra fica no meio e é conservadora:
//! só entra no lote o que é leitura barata. O que muta serializa — a mesma regra
//! do `pipeline` — e `exec`/`map` ficam de fora porque disputam `target/` e
//! podem pagar a construção do índice, além de esconder trabalho longo da linha
//! viva da TUI.

use tupi_domain::ToolCall;
use tupi_kernel::{AgentMode, ToolRegistry};

/// `true` quando a call declara efeito de leitura no modo atual. A declaração
/// vem da própria tool: nome na lista não é contrato.
pub(crate) fn call_is_parallel(tools: &ToolRegistry, call: &ToolCall, mode: AgentMode) -> bool {
    let Some(tool) = tools.get(&call.name) else {
        return false;
    };
    let arguments: serde_json::Value =
        serde_json::from_str(&call.arguments).unwrap_or(serde_json::Value::Null);
    tool.effect(&arguments).is_read_only() && tool.mode_mask().contains(mode)
}
