//! `/workflows/<nome>` e o autocomplete do namespace.
//!
//! O App só recebe o nome e os argumentos; quem monta a mensagem com os passos
//! é o `App`. Aqui ficam a chamada do comando e a lista que a TUI sugere.

use tupi_domain::WORKFLOWS_PREFIX;
use tupi_kernel::{Command, CommandBus, CommandError, CommandOutcome, Query, QueryBus, QueryResult};

use crate::state::ChatState;
use tupi_domain::RequestId;

pub(crate) use tupi_domain::parse_workflow_invocation;
/// Executa `/workflows/<nome> [pedido]`.
///
/// A conversa recebe os passos completos, mas o transcript mostra a linha que o
/// usuário digitou: repetir o markdown na tela gastaria espaço sem informação.
pub(crate) async fn run_workflow(
    bus: &CommandBus,
    state: &mut ChatState,
    name: &str,
    extra: &str,
) -> Option<RequestId> {
    match bus
        .dispatch(Command::RunWorkflow {
            name: name.to_string(),
            extra: extra.to_string(),
        })
        .await
    {
        Ok(CommandOutcome::Started { request_id }) => {
            let mut label = format!("{WORKFLOWS_PREFIX}{}", name.trim());
            let extra = extra.trim();
            if !extra.is_empty() {
                label.push(' ');
                label.push_str(extra);
            }
            state.push_user(&label);
            state.begin_assistant();
            state.scroll = 0;
            state.input.clear();
            state.refresh_suggestions();
            Some(request_id)
        }
        Ok(_) => None,
        Err(CommandError::Busy) => {
            state.notice = Some("aguarde o fim do stream".to_string());
            None
        }
        Err(error) => {
            state.fail(&error.to_string());
            None
        }
    }
}

/// Nomes oferecidos depois de `/workflows/`. Falha do catálogo não impede a
/// sessão: sem nomes, a barra final só não sugere nada.
pub(crate) async fn workflow_names(queries: &QueryBus) -> Vec<String> {
    match queries.dispatch(Query::ListWorkflows).await {
        Ok(QueryResult::Workflows(workflows)) => workflows
            .into_iter()
            .map(|workflow| workflow.name)
            .collect(),
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_prefix_comes_from_the_domain() {
        assert_eq!(WORKFLOWS_PREFIX, "/workflows/");
    }
}
