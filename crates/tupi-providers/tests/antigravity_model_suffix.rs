//! Experimento T7.3 — sufixo `:antigravity` no id do modelo.
//!
//! A referencia (opencode-antigravity-auth, src/constants.ts) descreve o
//! sufixo `:antigravity` no endpoint de producao ("used for models without
//! :antigravity suffix"), o que levantou a hipotese de que o Hub espera o id
//! sufixado no envelope de `v1internal:streamGenerateContent`.
//!
//! Resultado registrado: NAO confirmado em teste real. Nao havia credencial
//! viva para observar se o Hub aceita o id sufixado nem se quota/resposta
//! mudam, entao o sufixo nao foi ligado: `build_body` continua mandando o id
//! puro de `resolve_api_model`. Quando alguem repetir o experimento contra o
//! Hub e ele aceitar o id sufixado, a producao passa a anexar o sufixo e estes
//! testes mudam junto, com data e resultado do experimento no comentario.

use tupi_kernel::ChatRequest;
use tupi_providers::antigravity::{build_body, resolve_api_model};

/// Ids que o provider aceita hoje (amostra dos tiers gemini/claude/gpt-oss,
/// incluindo os que `resolve_api_model` normaliza).
const MODELS: &[&str] = &[
    "gemini-3.8-flash",
    "gemini-3.8-flash-high",
    "gemini-4-hydrogen",
    "gemini-4-argon-low",
    "gemini-3.1-pro",
    "gemini-3.1-pro-low",
    "gemini-3-flash",
    "gemini-3-flash-high",
    "gemini-2.5-pro",
    "claude-sonnet-5-5-high",
    "gpt-oss-120b-medium",
];

fn envelope_model(model: &str) -> String {
    let body = build_body(&ChatRequest::new(model, Vec::new()), "rising-fact-p41fc");
    body["model"]
        .as_str()
        .expect("envelope sempre carrega o id do modelo")
        .to_string()
}

#[test]
fn resolucao_de_nome_fica_estavel_sem_sufixo() {
    // Ids sem sufixo resolvem para si mesmos; a assinatura publica nao muda.
    for model in MODELS {
        let resolved = resolve_api_model(model);
        assert!(!resolved.contains(':'), "{model} ganhou sufixo: {resolved}");
        assert_eq!(resolved, envelope_model(model));
    }
    // O prefixo do tupi sai antes de entrar no envelope.
    assert_eq!(
        resolve_api_model("antigravity-gemini-3.8-flash-medium"),
        "gemini-3.8-flash-medium"
    );
    // Idempotente: id que ja venha sufixado passa igual (nao ha duplicacao).
    assert_eq!(
        resolve_api_model("gemini-3.8-flash-high:antigravity"),
        "gemini-3.8-flash-high:antigravity"
    );
}

#[test]
fn envelope_nao_anexa_sufixo_antes_do_experimento_confirmar() {
    // Se este teste falhar, alguem ligou o sufixo: atualize o cabecalho do
    // arquivo com data e resultado do experimento real antes de mergear.
    for model in MODELS {
        let served = envelope_model(model);
        assert_eq!(served, resolve_api_model(model));
        assert!(
            !served.ends_with(":antigravity"),
            "envelope de {model} saiu com ':antigravity' sem experimento real: {served}"
        );
    }
}
