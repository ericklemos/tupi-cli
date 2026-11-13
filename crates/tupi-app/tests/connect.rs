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
