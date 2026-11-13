use tupi_app::App;
use tupi_kernel::{Command, CommandHandler, EventBus, ProviderRegistry, ToolRegistry};

#[tokio::test]
async fn connect_unknown_provider_returns_error() {
    let bus = EventBus::new(8);
    let app = App::with_tools(
        ProviderRegistry::new(),
        "fake",
        "m".into(),
        bus,
        None,
        ToolRegistry::new(),
        String::new(),
    );

    let err = app
        .handle(Command::Connect {
            provider: "desconhecido".into(),
        })
        .await
        .unwrap_err();

    assert!(err.to_string().contains("desconhecido"));
}

#[test]
fn antigravity_credentials_survive_store_reopen_and_register() {
    let path = std::env::temp_dir().join(format!("tupi-connect-ag-{}.redb", std::process::id()));
    let _ = std::fs::remove_file(&path);

    let creds = tupi_providers::antigravity::AntigravityCredentials {
        access_token: "ya29.saved".into(),
        refresh_token: "1//refresh".into(),
        expires_at: u64::MAX,
        project_id: "proj-123".into(),
        email: Some("dev@example.com".into()),
    };

    let store_one = std::sync::Arc::new(tupi_store::Store::open(&path).unwrap());
    let store_two = std::sync::Arc::new(tupi_store::Store::open(&path).unwrap());

    App::save_antigravity_credentials(&store_one, &creds).unwrap();
    assert_eq!(
        store_two.connected_providers().unwrap(),
        vec!["antigravity".to_string()]
    );

    let loaded =
        App::load_antigravity_credentials(&store_two).expect("credenciais persistidas no redb");
    assert_eq!(loaded, creds);

    let registry_two = ProviderRegistry::new();
    App::sync_connected_providers(&registry_two, Some(&store_two));
    assert_eq!(registry_two.ids(), vec!["antigravity".to_string()]);

    drop(store_one);
    drop(store_two);

    let reopened = std::sync::Arc::new(tupi_store::Store::open(&path).unwrap());
    let registry_reopened = ProviderRegistry::new();
    App::sync_connected_providers(&registry_reopened, Some(&reopened));
    assert_eq!(registry_reopened.ids(), vec!["antigravity".to_string()]);

    let _ = std::fs::remove_file(&path);
}

#[tokio::test]
async fn new_app_session_loads_connected_antigravity_and_lists_models() {
    let path =
        std::env::temp_dir().join(format!("tupi-connect-models-{}.redb", std::process::id()));
    let _ = std::fs::remove_file(&path);

    let creds = tupi_providers::antigravity::AntigravityCredentials {
        access_token: "ya29.saved".into(),
        refresh_token: "1//refresh".into(),
        expires_at: u64::MAX,
        project_id: "proj-123".into(),
        email: Some("dev@example.com".into()),
    };

    let store = std::sync::Arc::new(tupi_store::Store::open(&path).unwrap());
    App::save_antigravity_credentials(&store, &creds).unwrap();
    // Even if a broken empty cache existed, RefreshModels must ignore it.
    store.save_models("antigravity", &[], u64::MAX / 2).unwrap();

    let bus = EventBus::new(8);
    let mut rx = bus.subscribe();
    let app = App::with_tools(
        ProviderRegistry::new(),
        "antigravity",
        "gemini-3.8-flash-high".into(),
        bus,
        Some(std::sync::Arc::clone(&store)),
        ToolRegistry::new(),
        String::new(),
    );

    app.handle(Command::RefreshModels).await.unwrap();

    let event = tokio::time::timeout(std::time::Duration::from_secs(5), rx.recv())
        .await
        .unwrap()
        .unwrap();
    match event {
        tupi_domain::Event::ModelsLoaded { models } => {
            assert!(!models.is_empty());
            assert!(models.contains(&"gemini-4-hydrogen-high".to_string()));
            assert!(models.contains(&"gemini-4-argon-high".to_string()));
        }
        other => panic!("esperava ModelsLoaded, recebeu {other:?}"),
    }

    let _ = std::fs::remove_file(&path);
}
