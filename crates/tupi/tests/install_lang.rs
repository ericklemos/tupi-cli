//! Idioma da saída de `tupi install`.
//!
//! O `lang` explícito de `ai.project.json` decide a língua do relatório. Sem o
//! campo — ou com o arquivo ausente ou inválido — a saída é em inglês, mesmo
//! com o locale do sistema em português.

use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

/// Locale do sistema em português: nunca pode escolher o idioma da saída.
const PT_BR_LOCALE: [(&str, &str); 2] = [("LANG", "pt_BR.UTF-8"), ("LC_ALL", "pt_BR.UTF-8")];

struct Sandbox {
    dir: tempfile::TempDir,
}

impl Sandbox {
    fn new() -> Self {
        let dir = tempfile::tempdir().expect("diretório temporário");
        std::fs::create_dir_all(dir.path().join("home")).expect("home do sandbox");
        Self { dir }
    }

    /// Root do projeto: é o `current_dir` do processo filho.
    fn root(&self) -> &Path {
        self.dir.path()
    }

    /// `HOME`/`USERPROFILE` do processo filho: o perfil real nunca é tocado.
    fn home(&self) -> PathBuf {
        self.root().join("home")
    }

    /// Escreve `ai.project.json` no root do projeto e devolve o caminho.
    fn project(&self, text: &str) -> PathBuf {
        let path = self.root().join("ai.project.json");
        std::fs::write(&path, text).expect("ai.project.json");
        path
    }

    fn run(&self, args: &[&str], env: &[(&str, &str)]) -> Output {
        let mut command = Command::new(env!("CARGO_BIN_EXE_tupi"));
        command
            .args(args)
            .current_dir(self.root())
            .env_clear()
            .env("HOME", self.home())
            .env("USERPROFILE", self.home())
            .env("SHELL", "/bin/zsh")
            .stdin(Stdio::null());
        for (key, value) in env {
            command.env(key, value);
        }
        command.output().expect("binário tupi")
    }
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn ok(output: &Output) -> String {
    assert!(
        output.status.success(),
        "`tupi install` falhou: {}",
        stderr(output)
    );
    stdout(output)
}

#[test]
fn an_explicit_lang_in_the_project_file_drives_the_dry_run() {
    let sandbox = Sandbox::new();
    sandbox.project(r#"{"name":"projeto","lang":"pt-br"}"#);

    let out = ok(&sandbox.run(&["install", "--print"], &[]));

    assert!(
        out.contains("nenhum arquivo foi criado ou alterado"),
        "saída em inglês apesar do lang explícito: {out}"
    );
    assert!(
        out.contains("diretório: "),
        "saída sem o rótulo traduzido: {out}"
    );
    assert!(
        out.contains("arquivo: "),
        "saída sem o rótulo traduzido: {out}"
    );
    assert!(
        !out.contains("no file was created or changed"),
        "saída bilíngue: {out}"
    );
    assert!(
        out.contains("# >>> tupi install >>>"),
        "saída sem o bloco marcado: {out}"
    );
}

#[test]
fn an_explicit_english_lang_wins_over_the_portuguese_locale() {
    let sandbox = Sandbox::new();
    sandbox.project(r#"{"name":"projeto","lang":"en"}"#);

    let out = ok(&sandbox.run(&["install", "--print"], &PT_BR_LOCALE));

    assert!(
        out.contains("no file was created or changed"),
        "saída sem o inglês explícito: {out}"
    );
    assert!(
        out.contains("directory: "),
        "saída sem o rótulo em inglês: {out}"
    );
    assert!(
        !out.contains("diretório"),
        "o locale do sistema escolheu a saída: {out}"
    );
}

#[test]
fn an_absent_lang_is_english_and_ignores_the_system_locale() {
    let sandbox = Sandbox::new();
    sandbox.project(r#"{"name":"projeto"}"#);

    let out = ok(&sandbox.run(&["install", "--print"], &PT_BR_LOCALE));

    assert!(
        out.contains("no file was created or changed"),
        "sem `lang` a saída deveria ser inglês: {out}"
    );
    assert!(
        !out.contains("diretório"),
        "o locale do sistema escolheu a saída: {out}"
    );
}

#[test]
fn an_invalid_lang_and_a_missing_project_file_are_english() {
    let sandbox = Sandbox::new();
    let invalid = sandbox.project(r#"{"name":"projeto","lang":"tlh"}"#);

    let out = ok(&sandbox.run(&["install", "--print"], &PT_BR_LOCALE));
    assert!(
        out.contains("no file was created or changed"),
        "`lang` inválido deveria cair no inglês: {out}"
    );
    assert_eq!(
        std::fs::read_to_string(&invalid).expect("ai.project.json"),
        r#"{"name":"projeto","lang":"tlh"}"#,
        "o install reescreveu ai.project.json"
    );

    std::fs::remove_file(&invalid).expect("remover ai.project.json");
    let out = ok(&sandbox.run(&["install", "--print"], &PT_BR_LOCALE));
    assert!(
        out.contains("no file was created or changed"),
        "projeto ausente deveria cair no inglês: {out}"
    );
    assert!(
        !sandbox.root().join("ai.project.json").exists(),
        "o install criou ai.project.json"
    );
}

#[test]
fn the_language_is_only_read_from_the_project_file() {
    let sandbox = Sandbox::new();
    let path = sandbox.project(r#"{"name":"projeto"}"#);
    let original = std::fs::read(&path).expect("ai.project.json");

    ok(&sandbox.run(&["install", "--print"], &PT_BR_LOCALE));

    assert_eq!(
        std::fs::read(&path).expect("ai.project.json"),
        original,
        "o --print reescreveu ai.project.json com o lang do sistema"
    );
}

#[test]
fn the_real_run_reports_in_the_project_language_and_is_idempotent() {
    let sandbox = Sandbox::new();
    sandbox.project(r#"{"name":"projeto","lang":"pt-br"}"#);

    let first = ok(&sandbox.run(&["install"], &[]));
    assert!(
        first.contains("diretório declarado no PATH"),
        "relatório não traduzido: {first}"
    );
    assert!(
        first.contains("ação: criado"),
        "ação não traduzida: {first}"
    );
    assert!(
        first.contains("reabra o shell"),
        "sem o aviso de reabrir o shell: {first}"
    );

    let profile = sandbox.home().join(".zshrc");
    assert!(
        profile.exists(),
        "nada foi escrito em {}",
        profile.display()
    );
    let saved = std::fs::read_to_string(&profile).expect(".zshrc");

    let second = ok(&sandbox.run(&["install"], &[]));
    assert!(
        second.contains("ação: já-presente"),
        "segunda execução não foi no-op: {second}"
    );
    assert_eq!(
        std::fs::read_to_string(&profile).expect(".zshrc"),
        saved,
        "o bloco foi duplicado no perfil"
    );
}
