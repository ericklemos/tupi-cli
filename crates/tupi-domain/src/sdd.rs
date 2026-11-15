//! Resumo de um plano SDD, compartilhado entre kernel, app e TUI.
//!
//! O store de planos vive em `tupi-tools`; o kernel só conhece `tupi-domain`,
//! então o contrato de listagem/retomada trafega este resumo em vez do plano
//! completo.

use std::fmt;

/// Status de um plano SDD no que interessa à retomada.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SddSpecStatus {
    /// Plano em andamento: aceita ser retomado.
    Draft,
    /// Plano encerrado com sucesso.
    Completed,
    /// Plano encerrado sem sucesso.
    Failed,
}

impl SddSpecStatus {
    /// Rótulo estável para briefs, autocomplete e logs.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Draft => "draft",
            Self::Completed => "completed",
            Self::Failed => "failed",
        }
    }

    /// `true` para planos que ainda aceitam turnos. `Failed` conta: a
    /// execução pode ser retomada; só `Completed` encerra o plano.
    pub fn is_resumable(self) -> bool {
        matches!(self, Self::Draft | Self::Failed)
    }
}

impl fmt::Display for SddSpecStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Resumo de um plano SDD persistido em `.tupi/sdd/<slug>/`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SddSpecSummary {
    /// Identificador do plano (nome do diretório em `.tupi/sdd/`).
    pub slug: String,
    /// Título do plano, como escrito no `plan.md`.
    pub title: String,
    /// Status atual do plano.
    pub status: SddSpecStatus,
    /// Quantidade de tasks que ainda não terminaram.
    pub open_tasks: usize,
    /// Quantidade total de tasks do plano.
    pub total_tasks: usize,
}

impl SddSpecSummary {
    /// Monta o resumo a partir dos campos crus do plano.
    pub fn new(
        slug: impl Into<String>,
        title: impl Into<String>,
        status: SddSpecStatus,
        open_tasks: usize,
        total_tasks: usize,
    ) -> Self {
        Self {
            slug: slug.into(),
            title: title.into(),
            status,
            open_tasks,
            total_tasks,
        }
    }

    /// Um plano é retomável enquanto não terminou (Completed/Failed ficam de fora).
    pub fn is_resumable(&self) -> bool {
        self.status.is_resumable()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn summary(status: SddSpecStatus) -> SddSpecSummary {
        SddSpecSummary::new("auth", "Retomar plano de auth", status, 2, 5)
    }

    #[test]
    fn completed_is_not_resumable_but_failed_is() {
        assert!(!summary(SddSpecStatus::Completed).is_resumable());
        assert!(summary(SddSpecStatus::Failed).is_resumable());
        assert!(summary(SddSpecStatus::Draft).is_resumable());
    }

    #[test]
    fn the_status_carries_a_stable_label() {
        assert_eq!(SddSpecStatus::Draft.as_str(), "draft");
        assert_eq!(SddSpecStatus::Completed.to_string(), "completed");
        assert_eq!(SddSpecStatus::Failed.to_string(), "failed");
        assert!(SddSpecStatus::Draft.is_resumable());
    }

    #[test]
    fn the_summary_keeps_the_task_counts() {
        let spec = summary(SddSpecStatus::Draft);
        assert_eq!(spec.slug, "auth");
        assert_eq!(spec.title, "Retomar plano de auth");
        assert_eq!(spec.open_tasks, 2);
        assert_eq!(spec.total_tasks, 5);
        assert_eq!(spec, summary(SddSpecStatus::Draft));
    }
}
