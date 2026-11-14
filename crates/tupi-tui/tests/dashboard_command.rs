//! Testes do comando `/dashboard` e do launcher injetado na TUI.

use std::sync::Arc;

use tupi_tui::run::{DashboardLauncher, dashboard_notice};
use tupi_tui::state::{ChatState, COMMANDS};

struct FakeLauncher;

#[async_trait::async_trait]
impl DashboardLauncher for FakeLauncher {
    async fn open(&self) -> Result<String, String> {
        Ok("http://127.0.0.1:9999".to_string())
    }
}

#[test]
fn the_dashboard_command_is_offered_in_autocomplete() {
    assert!(COMMANDS.contains(&"/dashboard"));

    let mut state = ChatState::default();
    state.input = "/dash".into();
    state.refresh_suggestions();

    assert_eq!(state.suggestion_selected(), Some("/dashboard"));
}

#[tokio::test]
async fn dashboard_notice_reports_the_url_or_the_missing_launcher() {
    let launcher: Arc<dyn DashboardLauncher> = Arc::new(FakeLauncher);
    let notice = dashboard_notice(Some(&launcher)).await;
    assert!(notice.contains("http://127.0.0.1:9999"), "notice: {notice}");

    let empty = dashboard_notice(None).await;
    assert!(empty.contains("--web"), "notice: {empty}");
}
