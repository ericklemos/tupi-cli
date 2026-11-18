//! Execução da verificação pós-escrita pelo verificador do repositório.
//!
//! O comando vem de `configs/verify.ron` (`super::VerifyRule`), roda na raiz do
//! projeto com o teto de tempo da regra e tem a saída colada na recusa: o juiz é
//! o verificador do repo, nunca o parser do tupi. Programa ausente não derruba o
//! turno — vira aviso, porque a escrita já está aplicada e a recusa só informa.

use std::path::{Path, PathBuf};
use std::time::Duration;

use super::{VerifyConfig, VerifyRule};

/// Teto de caracteres da saída colada na recusa.
const OUTPUT_LIMIT: usize = 4000;

/// Desfecho de um comando de verificação.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VerifyOutcome {
    /// O verificador aprovou o estado em disco.
    Passed {
        /// Comando como foi executado.
        command: String,
    },
    /// O verificador reprovou: a escrita já foi aplicada e não é desfeita.
    Failed {
        /// Comando como foi executado.
        command: String,
        /// Saída colada na recusa.
        output: String,
        /// Se o teto de tempo venceu antes do fim.
        timed_out: bool,
    },
    /// O programa não iniciou (ausente ou não executável): aviso, não recusa.
    Unavailable {
        /// Comando que não iniciou.
        command: String,
        /// Motivo observado na abertura do processo.
        detail: String,
    },
}

impl VerifyOutcome {
    /// Comando como aparece na recusa, no aviso e na telemetria.
    pub fn command(&self) -> &str {
        match self {
            VerifyOutcome::Passed { command }
            | VerifyOutcome::Failed { command, .. }
            | VerifyOutcome::Unavailable { command, .. } => command,
        }
    }

    /// Se o verificador reprovou o disco.
    pub fn is_failure(&self) -> bool {
        matches!(self, VerifyOutcome::Failed { .. })
    }
}

/// Resultado da verificação para um lote de caminhos já gravados.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct VerifyReport {
    written: Vec<String>,
    outcomes: Vec<VerifyOutcome>,
}

impl VerifyReport {
    /// Nenhum verificador reprovou — inclui o caso de nada declarado.
    pub fn is_clean(&self) -> bool {
        !self.outcomes.iter().any(VerifyOutcome::is_failure)
    }

    /// Comandos executados, na ordem de resolução.
    pub fn commands(&self) -> Vec<&str> {
        self.outcomes.iter().map(VerifyOutcome::command).collect()
    }

    /// Recusa pronta: comando, saída e o aviso de que a escrita ficou no disco.
    pub fn refusal(&self) -> Option<String> {
        let failed: Vec<&VerifyOutcome> = self
            .outcomes
            .iter()
            .filter(|outcome| outcome.is_failure())
            .collect();
        if failed.is_empty() {
            return None;
        }
        let mut message = String::from(
            "verificacao pos-escrita reprovada: a escrita ja foi aplicada e nao foi desfeita; \
             corrija a causa e reescreva o arquivo",
        );
        if !self.written.is_empty() {
            message.push_str(&format!(
                "\narquivos gravados: {}",
                self.written.join(", ")
            ));
        }
        for outcome in failed {
            if let VerifyOutcome::Failed {
                command,
                output,
                timed_out,
            } = outcome
            {
                let reason = if *timed_out {
                    "excedeu o timeout e foi encerrado"
                } else {
                    "saiu com falha"
                };
                message.push_str(&format!("\ncomando `{command}` {reason}\nsaida:\n{output}"));
            }
        }
        Some(message)
    }

    /// Avisos de verificadores que não iniciaram; não interrompem o turno.
    pub fn warnings(&self) -> Vec<String> {
        self.outcomes
            .iter()
            .filter_map(|outcome| match outcome {
                VerifyOutcome::Unavailable { command, detail } => Some(format!(
                    "verificacao pos-escrita ignorada: `{command}` nao iniciou ({detail}); \
                     a escrita ja aplicada segue valendo"
                )),
                _ => None,
            })
            .collect()
    }

    /// Linha de estado por comando, no formato curto do repo.
    pub fn render(&self) -> String {
        self.outcomes
            .iter()
            .map(|outcome| match outcome {
                VerifyOutcome::Passed { command } => format!("verificacao `{command}`: ok"),
                VerifyOutcome::Failed { command, .. } => {
                    format!("verificacao `{command}`: reprovada")
                }
                VerifyOutcome::Unavailable { command, detail } => {
                    format!("verificacao `{command}`: indisponivel ({detail})")
                }
            })
            .collect::<Vec<String>>()
            .join("\n")
    }
}

/// Roda, uma vez por comando, o verificador declarado para cada caminho gravado.
///
/// Caminho sem glob correspondente é no-op: quem não declarou nada não muda de
/// comportamento. A dedupe por linha de comando evita rodar `tsc --noEmit`
/// duas vezes quando a mesma escrita toca dois arquivos cobertos pela regra.
pub fn run_verification(root: &Path, config: &VerifyConfig, written: &[PathBuf]) -> VerifyReport {
    let mut report = VerifyReport::default();
    if config.is_empty() {
        return report;
    }
    let mut executed: Vec<String> = Vec::new();
    for path in written {
        let relative = crate::paths::relative(root, path);
        let Some(rule) = config.resolve(Path::new(&relative)) else {
            continue;
        };
        if !report.written.contains(&relative) {
            report.written.push(relative);
        }
        let command = rule.command_line();
        if executed.contains(&command) {
            continue;
        }
        executed.push(command);
        report.outcomes.push(run_rule(root, rule));
    }
    report
}

/// Executa um comando de verificação na raiz do projeto.
///
/// Falha de saída vira `Failed` (recusa); programa que não inicia vira
/// `Unavailable` (aviso) — a diferença é o que derruba o turno ou não.
pub fn run_rule(root: &Path, rule: &VerifyRule) -> VerifyOutcome {
    let command = rule.command_line();
    let timeout = Duration::from_secs(rule.timeout_secs());
    match crate::proc::program(&rule.program, &rule.args, root, timeout) {
        Ok(output) if output.success => VerifyOutcome::Passed { command },
        Ok(output) => {
            let timed_out = output.timed_out;
            tracing::warn!(%command, "verificacao pos-escrita reprovada");
            VerifyOutcome::Failed {
                command,
                output: captured(&output),
                timed_out,
            }
        }
        Err(error) => {
            tracing::warn!(%command, %error, "verificador ausente; turno segue");
            VerifyOutcome::Unavailable {
                command,
                detail: error.to_string(),
            }
        }
    }
}

/// Saída do verificador como entra na recusa: stderr, stdout, truncada.
fn captured(output: &crate::proc::Output) -> String {
    let mut text = String::new();
    let stderr = output.stderr.trim_end();
    let stdout = output.stdout.trim_end();
    if !stderr.is_empty() {
        text.push_str(stderr);
    }
    if !stdout.is_empty() {
        if !text.is_empty() {
            text.push('\n');
        }
        text.push_str(stdout);
    }
    if text.is_empty() {
        text.push_str(match output.timed_out {
            true => "sem saida antes do timeout",
            false => "sem saida",
        });
    }
    match text.chars().count() > OUTPUT_LIMIT {
        true => {
            let kept: String = text.chars().take(OUTPUT_LIMIT).collect();
            format!("{kept}\n... (saida truncada em {OUTPUT_LIMIT} caracteres)")
        }
        false => text,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn rule(glob: &str, program: &str, args: &[&str]) -> VerifyRule {
        VerifyRule {
            glob: glob.to_string(),
            program: program.to_string(),
            args: args.iter().map(|arg| arg.to_string()).collect(),
            timeout_secs: Some(20),
        }
    }

    fn config(rules: Vec<VerifyRule>) -> VerifyConfig {
        VerifyConfig { rules }
    }

    #[test]
    #[cfg(unix)]
    fn a_failing_command_is_a_refusal_citing_command_and_output() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("README.md");
        fs::write(&file, "titulo\n").unwrap();
        let config = config(vec![rule(
            "**/*.md",
            "sh",
            &["-c", "echo 'linha 3: erro'; exit 3"],
        )]);

        let report = run_verification(dir.path(), &config, std::slice::from_ref(&file));

        assert!(!report.is_clean());
        let refusal = report.refusal().expect("falha do verificador e recusa");
        assert!(
            refusal.contains("sh -c echo 'linha 3: erro'; exit 3"),
            "{refusal}"
        );
        assert!(refusal.contains("linha 3: erro"), "{refusal}");
        assert!(refusal.contains("nao foi desfeita"), "{refusal}");
        assert!(refusal.contains("README.md"), "{refusal}");
    }

    #[test]
    fn a_missing_command_only_warns() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("app.ts");
        fs::write(&file, "let a = 1;\n").unwrap();
        let config = config(vec![rule(
            "*.ts",
            "tupi-verificador-que-nao-existe",
            &["--noEmit"],
        )]);

        let report = run_verification(dir.path(), &config, std::slice::from_ref(&file));

        assert!(report.is_clean(), "comando ausente nao derruba o turno");
        assert!(report.refusal().is_none());
        let warnings = report.warnings();
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert!(
            warnings[0].contains("tupi-verificador-que-nao-existe --noEmit"),
            "{warnings:?}"
        );
        assert!(warnings[0].contains("segue valendo"), "{warnings:?}");
    }

    #[test]
    #[cfg(unix)]
    fn a_passing_command_runs_once_per_command() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("docs")).unwrap();
        let first = dir.path().join("README.md");
        let second = dir.path().join("docs/guia.md");
        fs::write(&first, "a\n").unwrap();
        fs::write(&second, "b\n").unwrap();
        let config = config(vec![rule("**/*.md", "sh", &["-c", "echo run >> runs.txt"])]);

        let report = run_verification(dir.path(), &config, &[first, second]);

        assert!(report.is_clean());
        assert!(report.refusal().is_none());
        assert_eq!(report.commands().len(), 1, "{:?}", report.commands());
        let runs = fs::read_to_string(dir.path().join("runs.txt")).unwrap();
        assert_eq!(runs.lines().count(), 1, "{runs}");
        assert!(report.render().contains(": ok"), "{}", report.render());
    }

    #[test]
    fn a_path_without_a_matching_rule_runs_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("notas.txt");
        fs::write(&file, "x\n").unwrap();
        let config = config(vec![rule("*.py", "ruff", &["check"])]);

        let report = run_verification(dir.path(), &config, std::slice::from_ref(&file));

        assert!(report.is_clean());
        assert!(report.commands().is_empty());
        assert!(report.render().is_empty());
    }

    #[test]
    fn nothing_declared_is_an_explicit_no_op() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a.rs");
        fs::write(&file, "fn main() {}\n").unwrap();

        let report = run_verification(dir.path(), &VerifyConfig::default(), &[file]);

        assert!(report.is_clean());
        assert!(report.refusal().is_none());
        assert!(report.warnings().is_empty());
    }
}
