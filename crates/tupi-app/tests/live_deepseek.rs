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
        history.iter().any(
            |message| message.role == Role::Tool && message.content.contains("!raw.is_empty()")
        ),
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

#[tokio::test(flavor = "multi_thread")]
#[ignore = "chama a API real da DeepSeek"]
async fn live_deepseek_edits_a_symbol_body() {
    let key = std::env::var("TUPI_CLI_DEEPSEEK_PLATAFORM_APIKEY")
        .expect("TUPI_CLI_DEEPSEEK_PLATAFORM_APIKEY não definida");
    let model = std::env::var("TUPI_MODEL").unwrap_or_else(|_| "deepseek-flash".to_string());

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("auth.rs");
    std::fs::write(
        &path,
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
        text: "Use a tool read para ler auth.rs, símbolo verify_token. Pegue o blake3 do \
               cabeçalho e use a tool edit para trocar o corpo por exatamente:\n\
               {\n    raw.len() > 3\n}\n\
               Não altere a assinatura. Responda apenas \"ok\"."
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

    let after = std::fs::read_to_string(&path).unwrap();
    assert!(
        after.contains("raw.len() > 3"),
        "o corpo editado deve estar no disco: {after}"
    );
    assert!(!after.contains("!raw.is_empty()"), "{after}");
    assert!(
        history
            .iter()
            .any(|message| message.role == Role::Tool && message.content.contains("editado")),
        "a tool edit precisa ter confirmado a edição"
    );
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "chama a API real da DeepSeek"]
async fn live_deepseek_uses_search_to_find_the_symbol() {
    let key = std::env::var("TUPI_CLI_DEEPSEEK_PLATAFORM_APIKEY")
        .expect("TUPI_CLI_DEEPSEEK_PLATAFORM_APIKEY não definida");
    let model = std::env::var("TUPI_MODEL").unwrap_or_else(|_| "deepseek-flash".to_string());

    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("src")).unwrap();
    std::fs::write(
        dir.path().join("src/auth.rs"),
        "pub fn verify_token(raw: &str) -> bool {\n    !raw.is_empty()\n}\n\npub fn logout() {}\n",
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
        text: "Use a tool search com a query \"onde o token é validado\" e responda \
               apenas com o nome da função encontrada, sem markdown."
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

    let tool_message = history
        .iter()
        .find(|message| message.role == Role::Tool)
        .expect("a tool search precisa ter sido chamada");
    assert!(
        tool_message.content.contains("verify_token"),
        "a busca deve citar o símbolo procurado: {:?}",
        tool_message.content
    );

    let last = history.last().unwrap();
    assert_eq!(last.role, Role::Assistant);
    assert!(
        last.content.contains("verify_token"),
        "a resposta final deve citar o nome encontrado: {:?}",
        last.content
    );
}
