//! Credenciais do `openai-plan` no redb: login grava, refresh regrava e o
//! provedor volta a ser registrado depois de reiniciar (sem rede).

use std::sync::Arc;

use tupi_app::app::App;
use tupi_kernel::ProviderRegistry;
use tupi_providers::openai_plan::{OpenAiPlanCredentials, PROVIDER_ID};

fn creds(access_token: &str, expires_at: u64) -> OpenAiPlanCredentials {
    OpenAiPlanCredentials {
        access_token: access_token.to_string(),
        refresh_token: "rt.saved".to_string(),
        expires_at,
        account_id: Some("acc-123".to_string()),
        email: Some("dev@example.com".to_string()),
    }
}

#[test]
fn login_grava_refresh_regrava_e_sync_reconecta() {
    let path = std::env::temp_dir().join(format!("tupi-openai-plan-{}.redb", std::process::id()));
    let _ = std::fs::remove_file(&path);

    let store_one = Arc::new(tupi_store::Store::open(&path).unwrap());
    let store_two = Arc::new(tupi_store::Store::open(&path).unwrap());

    // Login: a credencial entra em provider_credentials("openai-plan").
    let login = creds("at.login", 1_000);
    App::save_openai_plan_credentials(&store_one, &login).unwrap();
    assert_eq!(
        store_two.connected_providers().unwrap(),
        vec![PROVIDER_ID.to_string()]
    );
    assert_eq!(
        App::load_openai_plan_credentials(&store_two).expect("credenciais no redb"),
        login
    );

    // Refresh: o token novo substitui o vencido no mesmo registro.
    let renewed = creds("at.renewed", 9_999);
    App::save_openai_plan_credentials(&store_one, &renewed).unwrap();
    let loaded = App::load_openai_plan_credentials(&store_two).expect("credencial renovada");
    assert_eq!(loaded, renewed);
    assert_eq!(loaded.account_id.as_deref(), Some("acc-123"));

    let registry_two = ProviderRegistry::new();
    App::sync_connected_providers(&registry_two, Some(&store_two));
    assert_eq!(registry_two.ids(), vec![PROVIDER_ID.to_string()]);
    assert!(registry_two.get(PROVIDER_ID).is_some());

    drop(store_one);
    drop(store_two);

    // Reinício: só o redb reaberto basta para reconectar.
    let reopened = Arc::new(tupi_store::Store::open(&path).unwrap());
    let registry_reopened = ProviderRegistry::new();
    App::sync_connected_providers(&registry_reopened, Some(&reopened));
    assert_eq!(registry_reopened.ids(), vec![PROVIDER_ID.to_string()]);

    // Registro explícito (caminho do /connect) é idempotente no id.
    App::register_openai_plan_credentials(
        &registry_reopened,
        creds("at.again", u64::MAX),
        Some(Arc::clone(&reopened)),
    );
    assert_eq!(registry_reopened.ids(), vec![PROVIDER_ID.to_string()]);

    let _ = std::fs::remove_file(&path);
}
