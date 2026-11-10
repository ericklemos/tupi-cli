//! Verificação manual contra a DeepSeek real.
//!
//! Roda só quando pedido explicitamente: `cargo test -p tupi-app --test
//! live_deepseek -- --ignored --nocapture`. Sem a chave, o teste falha com uma
//! mensagem clara em vez de pular em silêncio.

use tupi_app::App;
use tupi_domain::{Event, Role};
use tupi_kernel::{Command, CommandHandler, EventBus, ProviderRegistry, ToolRegistry};
use tupi_providers::deepseek;

#[tokio::test(flavor = "multi_thread")]
#[ignore = "chama a API real da DeepSeek"]
async fn live_deepseek_uses_read_and_answers_from_the_body() {
    let key = std::env::var("TUPI_CLI_DEEPSEEK_PLATAFORM_APIKEY")
        .expect("TUPI_CLI_DEEPSEEK_PLATAFORM_APIKEY não definida");
    let model = std::env::var("TUPI_MODEL").unwrap_or_else(|_| "deepseek-flash".to_string());

    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("auth.rs"),
        "pub fn verify_token(raw: &str) -> bool {\n    !raw.is_empty()\n}\n",
    )
    .unwrap();

    let events = EventBus::new(256);
    let mut rx = events.subscribe();
    let mut providers = ProviderRegistry::new();
    deepseek::register_all(&mut providers, &key);
    let mut tools = ToolRegistry::new();
    tupi_tools::register(dir.path().to_path_buf(), &mut tools);

    let app = App::with_tools(providers, "deepseek", model, events, None, tools);

    app.handle(Command::SendMessage {
        text: "Use a tool read para ler o arquivo auth.rs, símbolo verify_token. \
               Depois responda apenas com o corpo da função, sem markdown."
            .to_string(),
    })
    .await
    .unwrap();

    loop {
        match rx.recv().await {
            Ok(Event::StreamFinished { .. }) => break,
            Ok(Event::StreamFailed { message, .. }) => panic!("stream falhou: {message}"),
            Ok(_) => {}
            Err(error) => panic!("eventos fecharam: {error}"),
        }
    }

    let history = app.session().messages;
    for message in &history {
        eprintln!("{:?} {:?}", message.role, message.content);
        for call in &message.tool_calls {
            eprintln!("  tool_call {} {} {}", call.id, call.name, call.arguments);
        }
    }

    assert!(
        history
            .iter()
            .any(|message| message.role == Role::Tool && message.content.contains("!raw.is_empty()")),
        "a tool read precisa ter devolvido o corpo do símbolo"
    );

    let last = history.last().unwrap();
    assert_eq!(last.role, Role::Assistant);
    assert!(
        last.content.contains("!raw.is_empty()"),
        "a resposta final deve citar o corpo lido: {:?}",
        last.content
    );
}
