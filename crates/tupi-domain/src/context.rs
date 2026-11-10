//! Poda do histórico por orçamento de tokens.
//!
//! O histórico era reenviado inteiro a cada passo de tool call, então o custo
//! de um turno crescia com o quadrado do número de passos e nada nunca saía.
//! Aqui o contexto é ajustado em duas etapas, da menos agressiva para a mais:
//!
//! 1. **encolher**: resultados de tool antigos viram um ponteiro de uma linha,
//!    mantendo a mensagem no lugar (a chamada e o resultado continuam casados,
//!    o que é exigência do protocolo do provedor);
//! 2. **cortar unidades**: uma unidade é um turno do usuário com tudo o que
//!    veio depois dele. Unidades antigas inteiras saem, da mais velha para a
//!    mais nova, e a mais recente nunca sai.
//!
//! Nenhuma das etapas quebra o pareamento `assistant.tool_calls` ↔
//! `role: tool`, porque uma unidade contém os dois lados.

use crate::tokens::{estimate_message, estimate_tokens};
use crate::{Message, Role};

/// Orçamento de contexto padrão, em tokens estimados.
pub const DEFAULT_CONTEXT_TOKENS: usize = 24_000;

/// Quantos resultados de tool recentes ficam sempre inteiros.
pub const RECENT_RESULTS_KEPT: usize = 3;

const POINTER_PREFIX: &str = "[resultado de tool omitido do contexto";

/// O que sobrou do histórico depois da poda.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    pub messages: Vec<Message>,
    pub estimate: usize,
    pub budget: usize,
    pub omitted_units: usize,
    pub shrunk_results: usize,
    /// A última unidade sozinha já estoura o orçamento. Nada seguro a cortar.
    pub over_budget: bool,
}

impl Plan {
    pub fn trimmed(&self) -> bool {
        self.omitted_units > 0 || self.shrunk_results > 0
    }

    /// Linha curta para o log e para o rodapé de debug.
    pub fn summary(&self) -> String {
        let mut text = format!("contexto: ~{} de {} tokens", self.estimate, self.budget);
        if self.omitted_units > 0 {
            text.push_str(&format!(
                " · {} turno(s) antigo(s) omitido(s)",
                self.omitted_units
            ));
        }
        if self.shrunk_results > 0 {
            text.push_str(&format!(
                " · {} resultado(s) encurtado(s)",
                self.shrunk_results
            ));
        }
        if self.over_budget {
            text.push_str(" · acima do orçamento mesmo após a poda");
        }
        text
    }
}

/// Planeja o contexto a enviar. `budget == 0` desliga a poda.
pub fn plan(messages: &[Message], budget: usize) -> Plan {
    let untouched = |messages: &[Message]| Plan {
        messages: messages.to_vec(),
        estimate: total(messages),
        budget,
        omitted_units: 0,
        shrunk_results: 0,
        over_budget: false,
    };

    if budget == 0 {
        return untouched(messages);
    }

    if total(messages) <= budget {
        return untouched(messages);
    }

    let mut kept = shrink_old_results(messages);
    let shrunk_results = kept
        .iter()
        .filter(|message| is_pointer(&message.content))
        .count();

    let mut omitted_units = 0;
    let mut estimate = total(&kept);
    while estimate > budget && units(&kept) > 1 {
        let cut = first_unit_len(&kept);
        kept.drain(..cut);
        omitted_units += 1;
        estimate = total(&kept);
    }

    Plan {
        messages: kept,
        estimate,
        budget,
        omitted_units,
        shrunk_results,
        over_budget: estimate > budget,
    }
}

/// Poda amortizada: só age quando o histórico passa do teto e, quando age, vai
/// até o alvo. Cortes rasos a cada chamada reescreveriam o começo da conversa e
/// invalidariam o prefixo que o provedor mantém em cache; cortes fundos e raros
/// preservam mais cache por token gasto.
pub fn plan_amortized(messages: &[Message], ceiling: usize, target: usize) -> Plan {
    let planned = plan(messages, ceiling);
    if !planned.trimmed() || target >= ceiling {
        return planned;
    }

    let mut deeper = plan(&planned.messages, target);
    // A segunda passada não vê o que a primeira já cortou: soma para o resumo
    // refletir o total, e fica com a contagem de ponteiros mais alta.
    deeper.omitted_units += planned.omitted_units;
    deeper.shrunk_results = deeper.shrunk_results.max(planned.shrunk_results);
    deeper
}

fn total(messages: &[Message]) -> usize {
    messages.iter().map(estimate_message).sum()
}

fn is_pointer(content: &str) -> bool {
    content.starts_with(POINTER_PREFIX)
}

/// Substitui o conteúdo de resultados antigos por um ponteiro barato.
fn shrink_old_results(messages: &[Message]) -> Vec<Message> {
    let tool_indices: Vec<usize> = messages
        .iter()
        .enumerate()
        .filter(|(_, message)| message.role == Role::Tool)
        .map(|(index, _)| index)
        .collect();

    let shrinkable = tool_indices.len().saturating_sub(RECENT_RESULTS_KEPT);
    let mut kept = messages.to_vec();

    for index in tool_indices.into_iter().take(shrinkable) {
        let message = &mut kept[index];
        if is_pointer(&message.content) {
            continue;
        }
        let tokens = estimate_tokens(&message.content);
        message.content = format!(
            "{POINTER_PREFIX}: ~{tokens} tokens. Chame a tool de novo se precisar do conteúdo — o ledger devolve ponteiro ou diff.]"
        );
    }

    kept
}

/// Quantas unidades (turnos do usuário) o histórico tem.
fn units(messages: &[Message]) -> usize {
    let starts = messages
        .iter()
        .filter(|message| message.role == Role::User)
        .count();
    starts.max(1)
}

/// Tamanho da unidade mais antiga: tudo até o próximo turno do usuário.
fn first_unit_len(messages: &[Message]) -> usize {
    messages
        .iter()
        .enumerate()
        .skip(1)
        .find(|(_, message)| message.role == Role::User)
        .map(|(index, _)| index)
        .unwrap_or(messages.len())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ToolCall;

    fn user(text: &str) -> Message {
        Message::user(text)
    }

    fn call(id: &str) -> ToolCall {
        ToolCall {
            id: id.into(),
            name: "read".into(),
            arguments: "{\"path\":\"a.rs\"}".into(),
        }
    }

    fn big_result(id: &str, tokens: usize) -> Message {
        Message::tool(id, "x".repeat(tokens * 4))
    }

    #[test]
    fn a_small_history_is_untouched() {
        let messages = vec![user("oi"), Message::assistant("olá")];
        let planned = plan(&messages, DEFAULT_CONTEXT_TOKENS);

        assert_eq!(planned.messages, messages);
        assert!(!planned.trimmed());
    }

    #[test]
    fn a_zero_budget_disables_pruning() {
        let messages = vec![user(&"a ".repeat(10_000))];
        let planned = plan(&messages, 0);

        assert_eq!(planned.messages, messages);
    }

    #[test]
    fn old_results_become_pointers_before_anything_is_dropped() {
        let messages = vec![
            user("leia"),
            Message::assistant_with_calls("", vec![call("c1")]),
            big_result("c1", 2_000),
            Message::assistant_with_calls("", vec![call("c2")]),
            big_result("c2", 2_000),
            Message::assistant_with_calls("", vec![call("c3")]),
            big_result("c3", 2_000),
            Message::assistant_with_calls("", vec![call("c4")]),
            big_result("c4", 2_000),
            Message::assistant_with_calls("", vec![call("c5")]),
            big_result("c5", 2_000),
        ];

        let planned = plan(&messages, 4_000);

        assert!(planned.shrunk_results > 0);
        assert!(
            planned
                .messages
                .iter()
                .filter(|message| is_pointer(&message.content))
                .count()
                <= planned.shrunk_results
        );
        assert!(planned.estimate < total(&messages));
    }

    #[test]
    fn the_most_recent_results_stay_whole() {
        let messages = vec![
            user("leia"),
            Message::assistant_with_calls("", vec![call("c1")]),
            big_result("c1", 2_000),
            Message::assistant_with_calls("", vec![call("c2")]),
            big_result("c2", 2_000),
        ];

        let planned = plan(&messages, 1_000);
        let last = planned.messages.last().unwrap();

        assert!(!is_pointer(&last.content));
    }

    #[test]
    fn a_large_history_loses_its_oldest_units() {
        let mut messages = Vec::new();
        for turn in 0..6 {
            messages.push(user(&format!("turno {turn} {}", "a ".repeat(2_000))));
            messages.push(Message::assistant("feito"));
        }

        let planned = plan(&messages, 8_000);

        assert!(planned.omitted_units > 0);
        assert_eq!(planned.messages[0].role, Role::User);
        assert!(
            !planned.messages[0].content.contains("turno 0"),
            "o turno mais velho deveria ter saído"
        );
        assert_eq!(
            planned.messages.last().unwrap().content,
            "feito",
            "o turno mais novo deveria continuar inteiro"
        );
    }

    #[test]
    fn the_last_unit_never_leaves_and_is_reported_as_over_budget() {
        let messages = vec![user(&"a ".repeat(20_000))];
        let planned = plan(&messages, 100);

        assert_eq!(planned.messages.len(), 1);
        assert_eq!(planned.omitted_units, 0);
        assert!(planned.over_budget);
    }

    #[test]
    fn pruning_never_orphans_a_tool_message() {
        let mut messages = Vec::new();
        for turn in 0..4 {
            messages.push(user(&format!("turno {turn} {}", "a ".repeat(3_000))));
            messages.push(Message::assistant_with_calls("", vec![call("c1")]));
            messages.push(Message::tool("c1", "y".repeat(4_000)));
        }

        let planned = plan(&messages, 6_000);

        assert_eq!(planned.messages[0].role, Role::User);
        for (index, message) in planned.messages.iter().enumerate() {
            if message.role == Role::Tool {
                let previous = &planned.messages[index - 1];
                assert!(
                    previous.role == Role::Assistant || previous.role == Role::Tool,
                    "resultado de tool sem chamada antes"
                );
            }
        }
    }

    #[test]
    fn an_amortized_plan_leaves_a_small_history_alone() {
        let messages = vec![user("oi"), big_result("c1", 100)];

        let planned = plan_amortized(&messages, 50_000, 20_000);

        assert_eq!(planned.messages.len(), messages.len());
        assert!(!planned.trimmed());
    }

    #[test]
    fn an_amortized_plan_cuts_past_the_target_once_the_ceiling_is_crossed() {
        let mut messages = Vec::new();
        for turn in 0..6 {
            messages.push(user(&format!("turno {turn} {}", "a ".repeat(4_000))));
            messages.push(Message::assistant("ok"));
        }

        let shallow = plan(&messages, 5_000);
        let deep = plan_amortized(&messages, 5_000, 2_000);

        assert!(shallow.trimmed());
        assert_eq!(deep.budget, 2_000);
        assert!(deep.estimate <= shallow.estimate || deep.over_budget);
        assert!(deep.omitted_units >= shallow.omitted_units);
    }

    #[test]
    fn the_summary_counts_what_was_trimmed() {
        let mut messages = Vec::new();
        for turn in 0..5 {
            messages.push(user(&format!("turno {turn} {}", "a ".repeat(3_000))));
            messages.push(Message::assistant("ok"));
        }

        let planned = plan(&messages, 5_000);

        assert!(planned.summary().contains("omitido"));
    }
}
