//! Abertura do dashboard no navegador padrão.

/// Abre `url` no navegador do sistema, sem nunca herdar o terminal da TUI.
pub fn open_browser(url: &str) {
    let (program, args) = browser_command(url);
    let _ = spawn_browser_command(program, &args);
}

#[cfg(target_os = "linux")]
fn browser_command(url: &str) -> (&'static str, Vec<String>) {
    ("xdg-open", vec![url.to_string()])
}

#[cfg(target_os = "macos")]
fn browser_command(url: &str) -> (&'static str, Vec<String>) {
    ("open", vec![url.to_string()])
}

#[cfg(target_os = "windows")]
fn browser_command(url: &str) -> (&'static str, Vec<String>) {
    ("cmd", vec!["/C".to_string(), "start".to_string(), String::new(), url.to_string()])
}

/// Dispara o processo em segundo plano. Devolve `false` quando o programa não
/// existe ou não pode ser iniciado (ambiente headless, por exemplo).
pub(crate) fn spawn_browser_command(program: &str, args: &[String]) -> bool {
    let mut command = std::process::Command::new(program);
    command
        .args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());

    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }

    match command.spawn() {
        Ok(mut child) => {
            std::thread::spawn(move || {
                let _ = child.wait();
            });
            true
        }
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn open_browser_ignores_missing_launcher_without_panicking() {
        assert!(!super::spawn_browser_command(
            "__tupi_browser_que_nao_existe__",
            &[],
        ));
    }
}
