//! Catálogo de workflows usado pelo `App`.
//!
//! A leitura do disco vive em `tupi-tools`; aqui só existe o índice vazio dos
//! testes e a resolução de `/workflows/<nome>` em mensagem de turno.

use std::sync::Arc;

use tupi_domain::Workflow;
use tupi_kernel::WorkflowIndex;

/// Índice sem nenhum workflow: o padrão de quem não ligou o catálogo.
#[derive(Debug, Default)]
pub struct EmptyIndex;

impl WorkflowIndex for EmptyIndex {
    fn list(&self) -> Vec<Workflow> {
        Vec::new()
    }
}

/// Índice fixo, para teste.
#[derive(Debug)]
pub struct FixedIndex {
    pub workflows: Vec<Workflow>,
}

impl WorkflowIndex for FixedIndex {
    fn list(&self) -> Vec<Workflow> {
        self.workflows.clone()
    }
}

pub type SharedIndex = Arc<dyn WorkflowIndex>;

#[cfg(test)]
mod tests {
    use super::*;
    use tupi_domain::WorkflowSource;

    #[test]
    fn the_empty_index_finds_nothing() {
        assert!(EmptyIndex.list().is_empty());
        assert!(EmptyIndex.find("feature").is_none());
    }

    #[test]
    fn the_fixed_index_finds_by_name() {
        let index = FixedIndex {
            workflows: vec![Workflow {
                name: "review".into(),
                description: String::new(),
                source: WorkflowSource::Native,
                path: None,
                body: "# Review\n".into(),
                shadowed: Vec::new(),
            }],
        };
        assert_eq!(
            index.find("REVIEW").map(|found| found.name),
            Some("review".into())
        );
    }
}
