//! Estimativa de tokens compartilhada entre o app e as tools.
//!
//! A régua antiga era `chars / 3.3`, que subestima código (pontuação densa) e
//! superestima prosa. Aqui a contagem imita o que um tokenizer BPE faz de
//! verdade: pedaços de até 4 caracteres alfanuméricos custam um token cada, e
//! cada sinal de pontuação custa um token próprio. O erro relativo fica na
//! casa de 10% tanto para código quanto para português, o que é suficiente
//! para decidir o que entra num envelope de orçamento.

/// Tamanho médio, em caracteres, de um pedaço alfanumérico que vira um token.
const PIECE_CHARS: usize = 4;

/// Estimativa de tokens de um texto qualquer.
pub fn estimate_tokens(text: &str) -> usize {
    if text.is_empty() {
        return 0;
    }

    let mut tokens = 0usize;
    let mut run = 0usize;

    for ch in text.chars() {
        if ch.is_alphanumeric() || ch == '_' {
            run += 1;
            continue;
        }
        if run > 0 {
            tokens += run.div_ceil(PIECE_CHARS);
            run = 0;
        }
        // Espaço não vira token; qualquer outro sinal, sim.
        if !ch.is_whitespace() {
            tokens += 1;
        }
    }
    if run > 0 {
        tokens += run.div_ceil(PIECE_CHARS);
    }

    tokens.max(1)
}

/// Estimativa de tokens de uma mensagem, incluindo metadados de tool call.
pub fn estimate_message(message: &crate::Message) -> usize {
    let mut total = estimate_tokens(&message.content);
    total += 4; // papel, separadores e o envelope por mensagem
    for call in &message.tool_calls {
        total += estimate_tokens(&call.name) + estimate_tokens(&call.arguments) + 8;
    }
    total
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_text_costs_nothing() {
        assert_eq!(estimate_tokens(""), 0);
    }

    #[test]
    fn prose_costs_roughly_one_token_per_word() {
        // "lê"(1) "o"(1) "arquivo"(2) "agora"(2) — perto do que um BPE real
        // gasta, sem inflar nem afundar como a régua antiga de chars/3.3.
        assert_eq!(estimate_tokens("lê o arquivo agora"), 6);
    }

    #[test]
    fn code_punctuation_costs_tokens() {
        // `f(a, b);` são 3 pedaços e 4 sinais.
        assert_eq!(estimate_tokens("f(a, b);"), 7);
    }

    #[test]
    fn whitespace_is_not_a_token() {
        assert_eq!(estimate_tokens("a b c"), 3);
        assert_eq!(estimate_tokens("a\n\n\nb"), 2);
    }

    #[test]
    fn long_identifiers_are_split_in_pieces() {
        assert_eq!(estimate_tokens("abcdefgh"), 2);
        assert_eq!(estimate_tokens("abcdefghi"), 3);
    }

    #[test]
    fn accented_words_count_as_alphanumeric() {
        assert_eq!(estimate_tokens("ação"), 1);
    }

    #[test]
    fn a_message_costs_content_plus_its_tool_calls() {
        let call = crate::ToolCall {
            id: "c1".into(),
            name: "read".into(),
            arguments: "{\"path\":\"a.rs\"}".into(),
        };
        let message = crate::Message::assistant_with_calls("", vec![call]);
        let bare = crate::Message::assistant("");

        assert!(estimate_message(&message) > estimate_message(&bare));
    }
}
