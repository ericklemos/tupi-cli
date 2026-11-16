//! O resultado do `shell` é inspeção crua: a memória do projeto não entra nele
//! (telemetria do run 1794840347760-112433, seq 12, 20, 22 e 24).

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use async_trait::async_trait;
use tupi_app::memory::Memory;
use tupi_kernel::{ChatRequest, Provider, ProviderError, ProviderEvent, ProviderStream};

struct CountingProvider {
    reply: String,
    calls: AtomicUsize,
}

#[async_trait]
impl Provider for CountingProvider {
    fn id(&self) -> &str {
        "counting"
    }

    async fn list_models(&self) -> Result<Vec<String>, ProviderError> {
        Ok(Vec::new())
    }

    async fn stream_chat(&self, _request: ChatRequest) -> Result<ProviderStream, ProviderError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(Box::pin(futures_util::stream::iter(vec![Ok(
            ProviderEvent::Text(self.reply.clone()),
        )])))
    }
}

#[tokio::test]
async fn the_shell_result_never_carries_project_memory() {
    let root = tempfile::tempdir().unwrap();
    let fact = tupi_domain::memory::record(
        root.path(),
        "o login usa PKCE",
        vec!["auth".to_string()],
        "test",
    )
    .unwrap()
    .expect("fato gravado");

    let provider = CountingProvider {
        reply: format!("{{\"ids\":[\"{}\"]}}", fact.id),
        calls: AtomicUsize::new(0),
    };
    let memory = Memory::new(root.path().to_path_buf());
    let cancel = AtomicBool::new(false);

    // O `shell` não recebe anexo e nem consulta o provedor de memória.
    assert!(
        memory
            .recall_for_tool(&provider, "m", "shell", "{\"command\":\"ls\"}", &cancel)
            .await
            .is_none(),
        "o resultado do shell não pode carregar a memória do projeto"
    );
    assert_eq!(
        provider.calls.load(Ordering::SeqCst),
        0,
        "o shell não pode gastar uma chamada de provedor com recall"
    );

    // As demais tools seguem recebendo o anexo, sem mudança.
    let block = memory
        .recall_for_tool(
            &provider,
            "m",
            "read",
            "{\"path\":\"crates/auth/login.rs\"}",
            &cancel,
        )
        .await
        .expect("o read continua recebendo o bloco de memória");
    assert!(block.text.contains("PKCE"));
    assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
}

/// T4.1: o turno que já recebeu o bloco no prefixo não paga um segundo
/// julgamento por tool call — uma requisição a menos no turno.
#[tokio::test]
async fn a_turn_that_already_attached_memory_does_not_judge_again_per_tool() {
    let root = tempfile::tempdir().unwrap();
    let fact = tupi_domain::memory::record(
        root.path(),
        "o login usa PKCE",
        vec!["auth".to_string()],
        "test",
    )
    .unwrap()
    .expect("fato gravado");
    let provider = CountingProvider {
        reply: format!("{{\"ids\":[\"{}\"]}}", fact.id),
        calls: AtomicUsize::new(0),
    };
    let memory = Memory::new(root.path().to_path_buf());
    let cancel = AtomicBool::new(false);

    let block = memory
        .begin_turn(&provider, "m", "como funciona o login?", &cancel)
        .await
        .expect("bloco anexado ao prefixo do turno");
    assert!(block.text.contains("PKCE"));
    assert_eq!(provider.calls.load(Ordering::SeqCst), 1);

    // A tool call seguinte reencontra a memória no prefixo: nada de nova
    // requisição ao provedor, e nada reanexado ao bloco de resultado.
    assert!(
        memory
            .recall_for_tool(
                &provider,
                "m",
                "read",
                "{\"path\":\"crates/auth/login.rs\"}",
                &cancel,
            )
            .await
            .is_none(),
        "o bloco já está no prefixo do turno"
    );
    assert_eq!(
        provider.calls.load(Ordering::SeqCst),
        1,
        "o turno não pode pagar dois julgamentos de memória"
    );
}

/// Provider que demora a responder: se o turno esperasse o julgamento, o teste
/// ficaria preso nele.
struct HangingProvider {
    calls: AtomicUsize,
}

#[async_trait]
impl Provider for HangingProvider {
    fn id(&self) -> &str {
        "hanging"
    }

    async fn list_models(&self) -> Result<Vec<String>, ProviderError> {
        Ok(Vec::new())
    }

    async fn stream_chat(&self, _request: ChatRequest) -> Result<ProviderStream, ProviderError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
        Ok(Box::pin(futures_util::stream::iter(vec![Ok(
            ProviderEvent::Text("{\"ids\":[]}".to_string()),
        )])))
    }
}

/// T4.2: o turno já julgou (mesmo sem achar nada). A tool call seguinte não
/// paga outra requisição nem espera por ela.
#[tokio::test]
async fn a_turn_that_already_judged_never_waits_for_the_memory_judge() {
    let root = tempfile::tempdir().unwrap();
    tupi_domain::memory::record(
        root.path(),
        "o login usa PKCE",
        vec!["auth".to_string()],
        "test",
    )
    .unwrap()
    .expect("fato gravado");
    let provider = HangingProvider {
        calls: AtomicUsize::new(0),
    };
    let memory = Memory::new(root.path().to_path_buf());
    let cancel = AtomicBool::new(false);

    let _ = memory
        .begin_turn(&provider, "m", "assunto sem relação zzz", &cancel)
        .await;

    let calls_before = provider.calls.load(Ordering::SeqCst);
    let started = std::time::Instant::now();
    let recalled = memory
        .recall_for_tool(&provider, "m", "read", "{\"path\":\"a.rs\"}", &cancel)
        .await;
    let elapsed = started.elapsed();

    assert!(recalled.is_none(), "o turno já julgou: nada a anexar");
    assert!(
        elapsed < std::time::Duration::from_secs(1),
        "o turno esperou o julgamento de memória ({elapsed:?})"
    );
    assert_eq!(
        provider.calls.load(Ordering::SeqCst),
        calls_before,
        "a tool call não pode pagar uma requisição de julgamento"
    );
}
