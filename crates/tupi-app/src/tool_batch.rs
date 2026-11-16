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

/// Leitura barata e sem efeito próprio: pode rodar junto das vizinhas do mesmo
/// passo sem mudar a ordem observável da história.
const BATCHABLE_READS: &[&str] = &["read", "read_many", "peek", "search", "docs", "assert"];

/// `true` quando a call pode rodar junto das vizinhas do mesmo passo.
pub(crate) fn call_is_parallel(tools: &ToolRegistry, call: &ToolCall, mode: AgentMode) -> bool {
    if !BATCHABLE_READS.contains(&call.name.as_str()) {
        return false;
    }
    tools
        .get(&call.name)
        .is_some_and(|tool| !tool.mutates() && tool.mode_mask().contains(mode))
}
