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
pub const DEFAULT_RECENT_RESULTS_KEPT: usize = RECENT_RESULTS_KEPT;

pub fn plan(messages: &[Message], budget: usize) -> Plan {
    plan_with(messages, budget, DEFAULT_RECENT_RESULTS_KEPT)
}

pub fn plan_with(messages: &[Message], budget: usize, recent_results: usize) -> Plan {
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

    let (mut kept, shrunk_results) = shrink_old_results(messages, recent_results);

    let mut omitted_units = 0;
    let mut summary_lines: Vec<String> = Vec::new();
    let mut estimate = total(&kept);
    while estimate > budget && units(&kept) > 1 {
        let cut = first_unit_len(&kept);
        summarize_unit(&kept[..cut], &mut summary_lines);
        kept.drain(..cut);
        omitted_units += 1;
        estimate = total(&kept);
    }

    if !summary_lines.is_empty() {
        attach_summary(&mut kept, &summary_lines, budget);
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
    plan_amortized_with(messages, ceiling, target, DEFAULT_RECENT_RESULTS_KEPT)
}

pub fn plan_amortized_with(
    messages: &[Message],
    ceiling: usize,
    target: usize,
    recent_results: usize,
) -> Plan {
    if ceiling == 0 || total(messages) <= ceiling || target >= ceiling {
        return plan_with(messages, ceiling, recent_results);
    }
    plan_with(messages, target, recent_results)
}

fn total(messages: &[Message]) -> usize {
    messages.iter().map(estimate_message).sum()
}

fn is_pointer(content: &str) -> bool {
    content.starts_with(POINTER_PREFIX)
}

/// Substitui o conteúdo de resultados antigos por um ponteiro barato.
/// Prefixo do bloco determinístico de resumo de turnos podados.
pub const SUMMARY_PREFIX: &str = "[Resumo de turnos podados:";

const MIN_RESULT_SHRINK_TOKENS: usize = 24;
const MIN_ARG_SHRINK_TOKENS: usize = 48;
const MAX_SUMMARY_TOKENS: usize = 160;
const KEPT_ARG_KEYS: &[&str] = &[
    "path", "symbol", "from", "to", "kind", "package", "target", "filter", "action", "url", "query",
];

fn is_shrunk_arg(arguments: &str) -> bool {
    arguments.contains("\"_omitted_tokens\"")
}

fn shrink_json_arguments(arguments: &str, tokens: usize) -> String {
    let mut out = serde_json::Map::new();
    if let Ok(serde_json::Value::Object(map)) = serde_json::from_str::<serde_json::Value>(arguments)
    {
        for &key in KEPT_ARG_KEYS {
            if let Some(val) = map.get(key) {
                let keep = match val {
                    serde_json::Value::String(s) => s.chars().count() <= 120,
                    serde_json::Value::Number(_) | serde_json::Value::Bool(_) => true,
                    _ => false,
                };
                if keep {
                    out.insert(key.to_string(), val.clone());
                }
            }
        }
    }
    out.insert(
        "_omitted_tokens".to_string(),
        serde_json::Value::Number(serde_json::Number::from(tokens)),
    );
    serde_json::Value::Object(out).to_string()
}

fn split_existing_summary(content: &str) -> (Vec<String>, &str) {
    if let Some(rest) = content.strip_prefix(SUMMARY_PREFIX)
        && let Some(end_idx) = rest.find("\n]\n\n")
    {
        let block = &rest[..end_idx];
        let body = &rest[end_idx + 4..];
        let lines = block
            .lines()
            .map(str::trim)
            .filter(|line| line.starts_with("- "))
            .map(ToOwned::to_owned)
            .collect();
        return (lines, body);
    }
    (Vec::new(), content)
}

fn format_call_target(name: &str, arguments: &str) -> String {
    if let Ok(serde_json::Value::Object(map)) = serde_json::from_str::<serde_json::Value>(arguments)
    {
        let path = map.get("path").and_then(|v| v.as_str());
        let symbol = map.get("symbol").and_then(|v| v.as_str());
        match (path, symbol) {
            (Some(p), Some(s)) => return format!("{name}({p}::{s})"),
            (Some(p), None) => return format!("{name}({p})"),
            (None, Some(s)) => return format!("{name}({s})"),
            (None, None) => {}
        }
        for key in ["target_symbol_or_file", "symbol_name", "query", "kind"] {
            if let Some(val) = map.get(key).and_then(|v| v.as_str()) {
                return format!("{name}({val})");
            }
        }
    }
    name.to_string()
}


/// Cabeçalho do manifesto do turno. Entra no prefixo do turno (como a memória),
/// nunca no bloco de resultado da tool: metadado de lista não é saída de
/// comando (razão registrada na supressão do `shell` em `memory.rs`).
pub const MANIFEST_PREFIX: &str = "## Já entregue neste turno (sem corpos: não releia; `peek` para o trecho omitido, `read` com repeat=\"allow\" para o corpo)";

/// Teto de entradas: o manifesto é lembrete compacto, não um segundo histórico.
const MANIFEST_MAX_ENTRIES: usize = 24;

/// Só o começo do resultado é inspecionado: hash e handle ficam no cabeçalho,
/// então o corpo nunca é varrido.
const MANIFEST_HEAD_CHARS: usize = 400;

/// Valor entre crases logo depois do marcador (`blake3 \`…\``, `handle: \`…\``).
fn marked_value(head: &str, marker: &str) -> Option<String> {
    let start = head.find(marker)? + marker.len();
    let value = head[start..].split('`').next()?.trim();
    (!value.is_empty()).then(|| value.to_string())
}

fn header_meta(head: &str) -> (Option<String>, Option<String>) {
    (
        marked_value(head, "blake3 `"),
        marked_value(head, "handle: `"),
    )
}

/// Manifesto do turno: uma linha por resultado já entregue, com o alvo do call
/// (path/símbolo), o hash do arquivo e o handle de `peek` quando existem.
/// Nenhum corpo: a lista é curta, o prefixo fica estável entre os passos e o
/// modelo deixa de gastar um passo relendo só para descobrir que já tinha o
/// resultado.
pub fn turn_manifest(messages: &[Message]) -> String {
    let mut targets: std::collections::HashMap<&str, String> = std::collections::HashMap::new();
    for message in messages {
        for call in &message.tool_calls {
            targets.insert(
                call.id.as_str(),
                format_call_target(&call.name, &call.arguments),
            );
        }
    }

    let mut lines: Vec<String> = Vec::new();
    for message in messages.iter().rev() {
        if message.role != Role::Tool {
            continue;
        }
        let Some(id) = message.tool_call_id.as_deref() else {
            continue;
        };
        let Some(target) = targets.get(id) else {
            continue;
        };
        let head: String = message.content.chars().take(MANIFEST_HEAD_CHARS).collect();
        let (hash, handle) = header_meta(&head);
        let mut line = format!("- {target}");
        if let Some(hash) = hash {
            line.push_str(&format!(" · blake3 {hash}"));
        }
        if let Some(handle) = handle {
            line.push_str(&format!(" · peek {handle}"));
        }
        if lines.contains(&line) {
            continue;
        }
        lines.push(line);
        if lines.len() >= MANIFEST_MAX_ENTRIES {
            break;
        }
    }

    if lines.is_empty() {
        return String::new();
    }
    lines.reverse();
    let mut out = String::from(MANIFEST_PREFIX);
    for line in &lines {
        out.push('\n');
        out.push_str(line);
    }
    out
}

fn summarize_unit(unit: &[Message], summary_lines: &mut Vec<String>) {
    let Some(first) = unit.first() else {
        return;
    };
    let (prior_lines, raw_user) = split_existing_summary(&first.content);
    summary_lines.extend(prior_lines);

    let collapsed: String = raw_user.split_whitespace().collect::<Vec<_>>().join(" ");
    let snippet: String = collapsed.chars().take(80).collect();

    let mut tools = Vec::new();
    for msg in unit {
        if msg.role == Role::Assistant {
            for call in &msg.tool_calls {
                let formatted = format_call_target(&call.name, &call.arguments);
                if !tools.contains(&formatted) && tools.len() < 6 {
                    tools.push(formatted);
                }
            }
        }
    }

    let line = if tools.is_empty() {
        format!("- {snippet}")
    } else {
        format!("- {snippet} [tools: {}]", tools.join(", "))
    };
    summary_lines.push(line);
}

fn attach_summary(kept: &mut [Message], summary_lines: &[String], budget: usize) {
    if kept.is_empty() || kept[0].role != Role::User {
        return;
    }
    let (existing_in_first, clean_body) = split_existing_summary(&kept[0].content);
    let clean_body_owned = clean_body.to_string();
    let mut all_lines = Vec::new();
    all_lines.extend_from_slice(summary_lines);
    all_lines.extend(existing_in_first);

    while all_lines.len() > 1 {
        let candidate = format!("{SUMMARY_PREFIX}\n{}\n]\n\n", all_lines.join("\n"));
        if estimate_tokens(&candidate) <= MAX_SUMMARY_TOKENS {
            break;
        }
        all_lines.remove(0);
    }

    let header = format!("{SUMMARY_PREFIX}\n{}\n]\n\n", all_lines.join("\n"));
    let header_tokens = estimate_tokens(&header);
    if header_tokens > MAX_SUMMARY_TOKENS {
        return;
    }

    kept[0].content = format!("{header}{clean_body_owned}");
    if total(kept) > budget {
        kept[0].content = clean_body_owned;
    }
}

fn shrink_old_results(messages: &[Message], recent_results: usize) -> (Vec<Message>, usize) {
    let tool_indices: Vec<usize> = messages
        .iter()
        .enumerate()
        .filter(|(_, message)| message.role == Role::Tool)
        .map(|(index, _)| index)
        .collect();

    let shrinkable = tool_indices.len().saturating_sub(recent_results);
    let recent_call_ids: std::collections::HashSet<String> = tool_indices
        .iter()
        .skip(shrinkable)
        .filter_map(|&idx| messages[idx].tool_call_id.clone())
        .collect();

    let mut kept = messages.to_vec();
    let mut shrunk_count = 0usize;

    for &index in tool_indices.iter().take(shrinkable) {
        let message = &mut kept[index];
        if is_pointer(&message.content) {
            continue;
        }
        let tokens = estimate_tokens(&message.content);
        if tokens <= MIN_RESULT_SHRINK_TOKENS {
            continue;
        }
        message.content = format!(
            "{POINTER_PREFIX}: ~{tokens} tokens. Chame a tool de novo com repeat=\"allow\" se precisar do corpo inteiro.]"
        );
        shrunk_count += 1;
    }

    for message in &mut kept {
        if message.role != Role::Assistant || message.tool_calls.is_empty() {
            continue;
        }
        for call in &mut message.tool_calls {
            if recent_call_ids.contains(&call.id) {
                continue;
            }
            if is_shrunk_arg(&call.arguments) {
                continue;
            }
            let tokens = estimate_tokens(&call.arguments);
            if tokens <= MIN_ARG_SHRINK_TOKENS {
                continue;
            }
            call.arguments = shrink_json_arguments(&call.arguments, tokens);
            shrunk_count += 1;
        }
    }

    (kept, shrunk_count)
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
        let pointers: Vec<_> = planned
            .messages
            .iter()
            .filter(|message| is_pointer(&message.content))
            .collect();
        assert!(!pointers.is_empty());
        assert!(pointers.len() <= planned.shrunk_results);
        assert!(
            pointers[0].content.contains("repeat=\"allow\""),
            "expected pointer to instruct repeat=\"allow\" so DeliveryLedger does not block re-reads, got: {}",
            pointers[0].content
        );
        assert!(planned.estimate < total(&messages));
    }

    #[test]
    fn recent_results_kept_is_configurable() {
        let messages = vec![
            user("leia"),
            Message::assistant_with_calls("", vec![call("c1")]),
            big_result("c1", 2_000),
            Message::assistant_with_calls("", vec![call("c2")]),
            big_result("c2", 2_000),
        ];

        let planned = plan_with(&messages, 1_000, 0);
        let tools: Vec<_> = planned
            .messages
            .iter()
            .filter(|message| message.role == Role::Tool)
            .collect();

        assert_eq!(tools.len(), 2);
        assert!(tools.iter().all(|message| is_pointer(&message.content)));
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
            !planned.messages[0]
                .content
                .contains(&format!("turno 0 {}", "a ".repeat(100))),
            "o corpo pesado do turno mais velho deveria ter saído"
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

    #[test]
    fn tiny_tool_results_are_not_replaced_by_longer_pointers() {
        let messages = vec![
            user("cheque"),
            Message::assistant_with_calls("", vec![call("c1")]),
            Message::tool("c1", "SIM"),
            Message::assistant_with_calls("", vec![call("c2")]),
            big_result("c2", 2_000),
            Message::assistant_with_calls("", vec![call("c3")]),
            big_result("c3", 2_000),
            Message::assistant_with_calls("", vec![call("c4")]),
            big_result("c4", 2_000),
        ];

        let planned = plan_with(&messages, 4_500, 1);

        assert_eq!(planned.messages[2].content, "SIM");
        assert!(is_pointer(&planned.messages[4].content));
    }

    #[test]
    fn old_heavy_tool_call_arguments_are_shrunk_to_valid_json_metadata() {
        let heavy_args = serde_json::json!({
            "path": "crates/tupi-app/src/app.rs",
            "symbol": "send_message",
            "new_body": "fn send_message() { ".to_string() + &"let x = 1; ".repeat(400) + "}"
        })
        .to_string();
        let messages = vec![
            user("edite"),
            Message::assistant_with_calls(
                "",
                vec![ToolCall {
                    id: "c1".into(),
                    name: "edit".into(),
                    arguments: heavy_args,
                }],
            ),
            Message::tool("c1", "ok"),
            Message::assistant_with_calls("", vec![call("c2")]),
            big_result("c2", 1_500),
        ];

        let planned = plan_with(&messages, 2_000, 1);
        let shrunk_args = &planned.messages[1].tool_calls[0].arguments;
        let parsed: serde_json::Value = serde_json::from_str(shrunk_args).unwrap();

        assert_eq!(parsed["path"], "crates/tupi-app/src/app.rs");
        assert_eq!(parsed["symbol"], "send_message");
        assert!(parsed.get("new_body").is_none());
        assert!(parsed["_omitted_tokens"].as_u64().unwrap() > 400);
        assert!(planned.shrunk_results >= 1);
    }

    #[test]
    fn malformed_heavy_tool_call_arguments_shrink_to_valid_json_fallback() {
        let bad_args = "{not valid json ".to_string() + &"x ".repeat(300);
        let messages = vec![
            user("edite"),
            Message::assistant_with_calls(
                "",
                vec![ToolCall {
                    id: "c1".into(),
                    name: "write".into(),
                    arguments: bad_args,
                }],
            ),
            Message::tool("c1", "ok"),
            Message::assistant_with_calls("", vec![call("c2")]),
            big_result("c2", 1_500),
        ];

        let planned = plan_with(&messages, 1_500, 1);
        let shrunk_args = &planned.messages[1].tool_calls[0].arguments;
        let parsed: serde_json::Value = serde_json::from_str(shrunk_args).unwrap();

        assert!(parsed["_omitted_tokens"].as_u64().unwrap() > 100);
    }

    #[test]
    fn replanning_already_shrunk_history_does_not_report_shrunk_results_again() {
        let heavy_args = serde_json::json!({
            "path": "crates/tupi-app/src/app.rs",
            "symbol": "send_message",
            "new_body": "fn send_message() { ".to_string() + &"let x = 1; ".repeat(400) + "}"
        })
        .to_string();
        let messages = vec![
            user("edite"),
            Message::assistant_with_calls(
                "",
                vec![ToolCall {
                    id: "c1".into(),
                    name: "edit".into(),
                    arguments: heavy_args,
                }],
            ),
            big_result("c1", 2_000),
            Message::assistant_with_calls("", vec![call("c2")]),
            big_result("c2", 2_000),
        ];

        let first = plan_with(&messages, 1_000, 1);
        assert!(first.trimmed());
        assert_eq!(first.shrunk_results, 2);

        let second = plan_with(&first.messages, 1_000, 1);
        assert_eq!(second.shrunk_results, 0);
        assert!(!second.trimmed());
    }

    #[test]
    fn dropped_units_leave_a_bounded_deterministic_summary_on_first_surviving_user_message() {
        let mut messages = Vec::new();
        messages.push(user(&format!(
            "corrija o bug no parser {}",
            "a ".repeat(1_500)
        )));
        messages.push(Message::assistant_with_calls(
            "",
            vec![ToolCall {
                id: "c1".into(),
                name: "edit".into(),
                arguments: r#"{"path":"src/parser.rs","symbol":"parse"}"#.into(),
            }],
        ));
        messages.push(Message::tool("c1", "ok"));
        messages.push(Message::assistant("pronto"));

        messages.push(user(&format!(
            "agora rode os testes {}",
            "b ".repeat(1_500)
        )));
        messages.push(Message::assistant("tudo verde"));

        let planned = plan(&messages, 2_200);

        assert_eq!(planned.omitted_units, 1);
        assert_eq!(planned.messages[0].role, Role::User);
        assert!(planned.messages[0].content.starts_with(SUMMARY_PREFIX));
        assert!(
            planned.messages[0]
                .content
                .contains("corrija o bug no parser")
        );
        assert!(
            planned.messages[0]
                .content
                .contains("edit(src/parser.rs::parse)")
        );
        assert!(planned.messages[0].content.contains("agora rode os testes"));

        let mut more = planned.messages.clone();
        more.push(user(&format!("terceiro turno {}", "c ".repeat(1_500))));
        more.push(Message::assistant("ok"));

        let planned_again = plan(&more, 2_200);
        let summary_headers = planned_again.messages[0]
            .content
            .matches(SUMMARY_PREFIX)
            .count();
        assert_eq!(summary_headers, 1);
        assert!(
            planned_again.messages[0]
                .content
                .contains("agora rode os testes")
        );
    }
}
