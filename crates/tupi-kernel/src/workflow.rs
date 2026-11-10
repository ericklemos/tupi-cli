use tupi_domain::Workflow;

/// Catálogo de workflows já resolvido entre as camadas nativo, global e local.
///
/// O kernel só enxerga o contrato: quem lê o disco é a implementação em
/// `tupi-tools`, e por isso a TUI pode pedir a lista sem conhecer diretório.
pub trait WorkflowIndex: Send + Sync {
    /// Workflows disponíveis, em ordem de nome, com o mais próximo vencendo.
    fn list(&self) -> Vec<Workflow>;

    /// Procura por nome exato, sem diferenciar maiúsculas de minúsculas.
    fn find(&self, name: &str) -> Option<Workflow> {
        let name = name.trim();
        self.list()
            .into_iter()
            .find(|workflow| workflow.name.eq_ignore_ascii_case(name))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tupi_domain::WorkflowSource;

    struct Fixed(Vec<Workflow>);

    impl WorkflowIndex for Fixed {
        fn list(&self) -> Vec<Workflow> {
            self.0.clone()
        }
    }

    fn workflow(name: &str) -> Workflow {
        Workflow {
            name: name.to_string(),
            description: String::new(),
            source: WorkflowSource::Native,
            path: None,
            body: String::new(),
            shadowed: Vec::new(),
        }
    }

    #[test]
    fn find_ignores_case_and_surrounding_space() {
        let index = Fixed(vec![workflow("fix-bug"), workflow("feature")]);
        assert_eq!(
            index.find(" Fix-Bug ").map(|w| w.name),
            Some("fix-bug".into())
        );
        assert!(index.find("review").is_none());
    }
}
