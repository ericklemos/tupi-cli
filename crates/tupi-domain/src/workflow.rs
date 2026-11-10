//! Workflows: procedimentos em markdown invocados por `/workflows/<nome>`.
//!
//! O domínio não toca o disco: descreve o que já foi resolvido pelas camadas e
//! monta a mensagem injetada na conversa. Quem lê os arquivos atende a porta
//! `tupi_kernel::WorkflowIndex`.

/// Prefixo do comando que invoca um workflow.
///
/// O namespace só existe com a barra final: `/workflow` sozinho não é comando,
/// e por isso nenhum nome de workflow aparece antes dessa barra.
pub const WORKFLOWS_PREFIX: &str = "/workflows/";

/// Separa `/workflows/nome argumentos` em `(nome, argumentos)`.
///
/// Devolve `None` para qualquer entrada fora do namespace.
pub fn parse_workflow_invocation(input: &str) -> Option<(&str, &str)> {
    let rest = input.trim_start().strip_prefix(WORKFLOWS_PREFIX)?.trim();
    match rest.split_once(char::is_whitespace) {
        Some((name, extra)) => Some((name, extra.trim())),
        None => Some((rest, "")),
    }
}

/// De onde o workflow veio. A ordem é a precedência: quem vem depois sombreia
/// quem vem antes, então o projeto vence o usuário e o usuário vence o binário.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum WorkflowSource {
    Native,
    GlobalAgents,
    GlobalTupi,
    LocalAgents,
    LocalTupi,
}

impl WorkflowSource {
    pub fn as_str(self) -> &'static str {
        match self {
            WorkflowSource::Native => "nativo",
            WorkflowSource::GlobalAgents => "global (.agents)",
            WorkflowSource::GlobalTupi => "global (.tupi)",
            WorkflowSource::LocalAgents => "local (.agents)",
            WorkflowSource::LocalTupi => "local (.tupi)",
        }
    }
}

/// Procedimento em markdown, já resolvido entre as camadas.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Workflow {
    pub name: String,
    pub description: String,
    pub source: WorkflowSource,
    /// Relativo ao root do projeto; `None` quando viaja dentro do binário.
    pub path: Option<std::path::PathBuf>,
    pub body: String,
    /// Camadas que este workflow tornou inalcançáveis.
    pub shadowed: Vec<WorkflowSource>,
}

impl Workflow {
    /// Mensagem injetada na conversa por `/workflows/<nome>`.
    pub fn invocation(&self, extra: &str) -> String {
        let mut out = format!(
            "Siga o workflow `{}` ({}) passo a passo.\n\n{}\n",
            self.name,
            self.source.as_str(),
            self.body.trim()
        );
        let extra = extra.trim();
        if !extra.is_empty() {
            out.push_str(&format!("\nPedido do usuário: {extra}\n"));
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn workflow() -> Workflow {
        Workflow {
            name: "feature".into(),
            description: "leva a feature do desenho ao teste".into(),
            source: WorkflowSource::LocalTupi,
            path: None,
            body: "# Feature\n\n1. enquadre\n".into(),
            shadowed: vec![WorkflowSource::Native],
        }
    }

    #[test]
    fn the_prefix_needs_the_trailing_slash() {
        assert!(parse_workflow_invocation("/workflows/feature").is_some());
        assert!(parse_workflow_invocation("/workflow/feature").is_none());
        assert!(parse_workflow_invocation("/workflows").is_none());
        assert!(parse_workflow_invocation("/models").is_none());
    }

    #[test]
    fn the_name_comes_before_the_arguments() {
        assert_eq!(
            parse_workflow_invocation("/workflows/fix-bug"),
            Some(("fix-bug", ""))
        );
        assert_eq!(
            parse_workflow_invocation("  /workflows/fix-bug  no parser  "),
            Some(("fix-bug", "no parser"))
        );
        assert_eq!(parse_workflow_invocation("/workflows/"), Some(("", "")));
    }

    #[test]
    fn the_invocation_carries_the_body_and_the_source() {
        let text = workflow().invocation("");
        assert!(text.starts_with("Siga o workflow `feature` (local (.tupi))"));
        assert!(text.contains("1. enquadre"));
        assert!(!text.contains("Pedido do usuário"));
    }

    #[test]
    fn extra_arguments_become_the_pedido() {
        let text = workflow().invocation("adicionar workflows");
        assert!(text.ends_with("Pedido do usuário: adicionar workflows\n"));
    }

    #[test]
    fn the_order_is_the_precedence() {
        assert!(WorkflowSource::Native < WorkflowSource::GlobalAgents);
        assert!(WorkflowSource::GlobalAgents < WorkflowSource::GlobalTupi);
        assert!(WorkflowSource::GlobalTupi < WorkflowSource::LocalAgents);
        assert!(WorkflowSource::LocalAgents < WorkflowSource::LocalTupi);
    }
}
