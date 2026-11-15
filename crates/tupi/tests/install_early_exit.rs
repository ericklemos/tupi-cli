//! `tupi install` responde antes de qualquer inicialização pesada: não exige
//! chave de API e não cria nem abre o `~/.tupi/tupi.redb`.
//!
//! Os testes rodam o binário de verdade num `HOME` temporário e com o ambiente
//! limpo, então nada vaza do perfil de quem executa a suíte.
#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

/// Executa o binário com `HOME`, `USERPROFILE` e `SHELL` injetados e o resto do
/// ambiente descartado: nem a chave de API nem o perfil real participam.
struct Sandbox {
    dir: tempfile::TempDir,
}

impl Sandbox {
    fn new() -> Self {
        Self {
            dir: tempfile::tempdir().expect("diretório temporário"),
        }
    }

    fn home(&self) -> &Path {
        self.dir.path()
    }

    /// O arquivo que `Store::open` cria no boot normal da TUI.
    fn store(&self) -> PathBuf {
        self.home().join(".tupi").join("tupi.redb")
    }

    fn run(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_tupi"))
            .args(args)
            .current_dir(self.home())
            .env_clear()
            .env("HOME", self.home())
            .env("USERPROFILE", self.home())
            .env("SHELL", "/bin/zsh")
            .stdin(Stdio::null())
            .output()
            .expect("binário tupi")
    }
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

#[test]
fn install_runs_without_an_api_key() {
    let sandbox = Sandbox::new();

    let output = sandbox.run(&["install"]);

    assert!(
        output.status.success(),
        "`tupi install` falhou sem chave de API: {}",
        stderr(&output)
    );
    assert!(
        !output.stdout.is_empty(),
        "`tupi install` deveria relatar o diretório e o arquivo tocado"
    );
}

#[test]
fn install_never_creates_the_local_store() {
    let sandbox = Sandbox::new();

    let output = sandbox.run(&["install"]);

    assert!(output.status.success(), "{}", stderr(&output));
    assert!(
        !sandbox.store().exists(),
        "`tupi install` criou {}; o redb não pode ser aberto nesse caminho",
        sandbox.store().display()
    );
    assert!(
        !sandbox.home().join(".tupi").exists(),
        "`tupi install` tocou no diretório de estado {}/.tupi",
        sandbox.home().display()
    );
}

/// Guarda dos testes acima: prova que este ambiente realmente não tem chave e
/// que o caminho da TUI continua exigindo uma.
#[test]
fn the_same_environment_without_the_subcommand_asks_for_the_key() {
    let sandbox = Sandbox::new();

    let output = sandbox.run(&[]);

    assert!(!output.status.success(), "o boot normal deveria falhar");
    assert!(
        stderr(&output).contains("DEEPSEEK_API_KEY"),
        "esperava o erro de chave ausente, veio: {}",
        stderr(&output)
    );
}

#[test]
fn the_print_flag_writes_nothing_and_shows_the_block_and_the_path() {
    let sandbox = Sandbox::new();
    let profile = sandbox.home().join(".zshrc");

    let output = sandbox.run(&["install", "--print"]);

    assert!(
        output.status.success(),
        "`tupi install --print` falhou: {}",
        stderr(&output)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("# >>> tupi install >>>"),
        "saída sem o bloco marcado: {stdout}"
    );
    assert!(
        stdout.contains(&profile.display().to_string()),
        "saída sem o caminho que receberia o bloco: {stdout}"
    );
    assert!(!profile.exists(), "o --print criou {}", profile.display());
    assert_eq!(
        std::fs::read_dir(sandbox.home())
            .expect("diretório temporário")
            .count(),
        0,
        "o --print escreveu em {}",
        sandbox.home().display()
    );
}
