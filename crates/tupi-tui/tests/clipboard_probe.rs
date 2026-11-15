//! Integração real com o clipboard da sessão gráfica.
//!
//! Depende de um display (X11/XWayland ou Wayland), por isso fica ignorado
//! por padrão: rode com `cargo test -p tupi-tui --test clipboard_probe -- --ignored`
//! numa sessão com área de transferência.

#[test]
#[ignore = "usa o clipboard da sessão gráfica; rode com -- --ignored"]
fn copy_reaches_the_system_clipboard_without_external_helpers() {
    let marker = format!("tupi-probe-{}", std::process::id());

    assert!(
        tupi_tui::clipboard::copy_to_system(&marker),
        "nenhum backend copiou: tupi ficaria dependente de OSC 52"
    );
    assert_eq!(
        tupi_tui::clipboard::paste_from_system().as_deref(),
        Some(marker.as_str()),
        "o texto copiado não volta do clipboard do sistema"
    );
}
