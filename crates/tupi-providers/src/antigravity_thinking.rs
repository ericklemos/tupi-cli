//! Resolucao do tier de raciocinio (`thinkingConfig`) do provider Antigravity.
//!
//! A referencia (opencode-antigravity-auth) expoe `resolveThinkingConfig`: o
//! orcamento de thinking sai do tier embutido no nome do modelo. Concentrar o
//! mapeamento aqui evita que `build_body` e o resto do provider divirjam.

/// Orcamento de thinking, em tokens de raciocinio, por tier.
///
/// Degraus estaveis e sem sobreposicao: low < medium < high < xhigh.
const BUDGET_LOW: u64 = 1_024;
const BUDGET_MEDIUM: u64 = 8_192;
const BUDGET_HIGH: u64 = 24_576;
const BUDGET_XHIGH: u64 = 32_768;

/// Tier de raciocinio embutido no nome do modelo (`-low`, `-medium`, `-high`,
/// `-xhigh`), ordenado do menor para o maior orcamento.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum ThinkingTier {
    Low,
    Medium,
    High,
    XHigh,
}

impl ThinkingTier {
    /// Orcamento de thinking enviado em `generationConfig.thinkingConfig`.
    pub(crate) fn budget_tokens(self) -> u64 {
        match self {
            Self::Low => BUDGET_LOW,
            Self::Medium => BUDGET_MEDIUM,
            Self::High => BUDGET_HIGH,
            Self::XHigh => BUDGET_XHIGH,
        }
    }
}

/// Resolve o tier a partir do nome pedido (com ou sem prefixo `antigravity-`),
/// nunca do id ja normalizado: `resolve_api_model` colapsa `gemini-3-flash-high`
/// em `gemini-3-flash` e o tier se perderia. O marcador `thinking` da
/// referencia (modelos `claude-*-thinking`) herda o orcamento maximo.
pub(crate) fn resolve_thinking_tier(model: &str) -> Option<ThinkingTier> {
    let lower = model.trim().to_ascii_lowercase();
    let without_quota = lower.strip_prefix("antigravity-").unwrap_or(&lower);
    let base = without_quota.split(':').next().unwrap_or(without_quota);

    if base.ends_with("-xhigh") {
        Some(ThinkingTier::XHigh)
    } else if base.ends_with("-high") {
        Some(ThinkingTier::High)
    } else if base.ends_with("-medium") {
        Some(ThinkingTier::Medium)
    } else if base.ends_with("-low") {
        Some(ThinkingTier::Low)
    } else if base == "thinking" || base.ends_with("-thinking") {
        Some(ThinkingTier::XHigh)
    } else {
        None
    }
}

/// Piso de `maxOutputTokens` para modelos Claude.
///
/// O orcamento de thinking sai do mesmo teto de saida (referencia:
/// `CLAUDE_THINKING_MAX_OUTPUT_TOKENS`), entao um `max_tokens` pequeno estrangula
/// a resposta e corta o raciocinio no meio. O excedente nao e consumido porque a
/// geracao para no stop token.
pub(crate) const CLAUDE_THINKING_MAX_OUTPUT_TOKENS: u64 = 64_000;

/// Header que habilita raciocinio intercalado com tool use nos modelos Claude
/// (referencia: `anthropic-beta: interleaved-thinking-2025-05-14`).
pub(crate) const INTERLEAVED_THINKING_BETA: &str = "interleaved-thinking-2025-05-14";

/// `true` quando o id pedido e um modelo Claude (com ou sem prefixo
/// `antigravity-` e sufixo de quota).
///
/// O header de thinking intercalado acompanha a familia do modelo, nao o tier:
/// o Hub nao distingue `claude-*-thinking` de `claude-*-high` no encaminhamento.
pub(crate) fn is_claude_model(model: &str) -> bool {
    let lower = model.trim().to_ascii_lowercase();
    let without_quota = lower.strip_prefix("antigravity-").unwrap_or(&lower);
    let base = without_quota.split(':').next().unwrap_or(without_quota);
    base.starts_with("claude-")
}

/// `maxOutputTokens` efetivo: em Claude o piso
/// [`CLAUDE_THINKING_MAX_OUTPUT_TOKENS`] vale mesmo quando o pedido nao traz
/// `max_tokens` ou traz um valor menor. Fora de Claude o pedido passa intacto.
pub(crate) fn claude_output_tokens(model: &str, requested: Option<u64>) -> Option<u64> {
    if is_claude_model(model) {
        Some(requested.map_or(CLAUDE_THINKING_MAX_OUTPUT_TOKENS, |tokens| {
            tokens.max(CLAUDE_THINKING_MAX_OUTPUT_TOKENS)
        }))
    } else {
        requested
    }
}
