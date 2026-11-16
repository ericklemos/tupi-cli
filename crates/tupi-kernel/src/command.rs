use std::sync::Arc;

use async_trait::async_trait;
use tupi_domain::RequestId;

use crate::CommandError;

#[derive(Debug, Clone, PartialEq, Eq)]
/// Ações do comando `/memory`.
pub enum MemoryAction {
    List,
    Search(String),
    Forget(String),
    Pending,
    Off,
    On,
}

/// Uma linha visível da conversa, pronta para a TUI desenhar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TranscriptEntry {
    User {
        text: String,
    },
    Assistant {
        text: String,
    },
    Tool {
        call_id: String,
        name: String,
        summary: String,
        ok: bool,
        duration_us: u64,
    },
    Notice {
        text: String,
    },
}

pub enum Command {
    SendMessage {
        text: String,
    },
    CancelStream {
        request_id: RequestId,
    },
    SetModel {
        model: String,
    },
    RefreshModels,
    ToggleShowPrompts,
    /// Alterna entre os modos Execution e Sdd.
    ToggleMode,
    /// Define explicitamente o modo do agente.
    SetMode {
        mode: tupi_domain::AgentMode,
    },
    /// `/workflows/<nome> [pedido]`: os passos entram na conversa como turno.
    RunWorkflow {
        name: String,
        extra: String,
    },
    /// `/new`: descarta o histórico e recomeça a conversa do zero.
    NewSession,
    /// `/connect <provedor>`: inicia o fluxo de conexão/login do provedor.
    Connect {
        provider: String,
    },
    /// `/memory ...`: audita e cura a memória do repositório.
    Memory {
        action: MemoryAction,
    },
    /// `/sdd <slug>`: reativa o plano persistido, entra no modo Sdd e
    /// devolve o controle ao usuário — não abre turno.
    ResumeSdd {
        slug: String,
    },
    /// `/sessions`: lista as sessões gravadas no repositório.
    ListSessions,
    /// `/sessions` + Enter: retoma a sessão escolhida.
    ResumeSession {
        id: String,
    },
    /// `/sessions` + `d` duas vezes: apaga a sessão escolhida.
    DeleteSession {
        id: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommandOutcome {
    Started {
        request_id: RequestId,
    },
    Cancelled,
    ModelChanged {
        model: String,
    },
    ShowPrompts {
        enabled: bool,
    },
    ModeChanged {
        mode: tupi_domain::AgentMode,
    },
    /// `/new` aceito: o histórico foi descartado e a próxima mensagem abre um turno novo.
    SessionReset,
    Memory {
        title: String,
        items: Vec<String>,
        ids: Vec<String>,
    },
    MemoryToggled {
        enabled: bool,
    },
    MemoryForgotten {
        id: String,
        found: bool,
    },
    /// `/sdd <slug>` aceito: o plano foi retomado e o modo já é o informado.
    SddResumed {
        slug: String,
        title: String,
        mode: tupi_domain::AgentMode,
        open_tasks: usize,
    },
    /// `/sessions` aceito: as linhas e os ids estão na ordem da lista.
    SessionsList {
        items: Vec<String>,
        ids: Vec<String>,
    },
    /// A sessão foi retomada: o app já trocou o histórico e o modo.
    SessionResumed {
        id: String,
        title: String,
        mode: tupi_domain::AgentMode,
        entries: Vec<TranscriptEntry>,
    },
    SessionDeleted {
        id: String,
        found: bool,
    },
    Noop,
}

#[async_trait]
pub trait CommandHandler: Send + Sync {
    async fn handle(&self, command: Command) -> Result<CommandOutcome, CommandError>;

    /// Roteia o comando para a sessão indicada. `None` resolve no foco atual,
    /// que é o comportamento de `dispatch`. O padrão delega para `handle` para
    /// que handlers sem noção de sessão continuem válidos.
    async fn handle_in(
        &self,
        _session: Option<tupi_domain::SessionId>,
        command: Command,
    ) -> Result<CommandOutcome, CommandError> {
        self.handle(command).await
    }
}

#[derive(Clone)]
pub struct CommandBus {
    handler: Arc<dyn CommandHandler>,
}

impl CommandBus {
    pub fn new(handler: Arc<dyn CommandHandler>) -> Self {
        Self { handler }
    }

    pub async fn dispatch(&self, command: Command) -> Result<CommandOutcome, CommandError> {
        self.handler.handle_in(None, command).await
    }

    /// Despacha o comando para uma sessão explícita sem mover o foco.
    pub async fn dispatch_in(
        &self,
        session: tupi_domain::SessionId,
        command: Command,
    ) -> Result<CommandOutcome, CommandError> {
        self.handler.handle_in(Some(session), command).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_commands_carry_what_the_tui_needs() {
        let entries = vec![
            TranscriptEntry::User { text: "oi".into() },
            TranscriptEntry::Tool {
                call_id: "c1".into(),
                name: "read".into(),
                summary: "src/lib.rs".into(),
                ok: true,
                duration_us: 42,
            },
        ];
        let outcome = CommandOutcome::SessionResumed {
            id: "100-1".into(),
            title: "Primeira".into(),
            mode: tupi_domain::AgentMode::Sdd,
            entries: entries.clone(),
        };

        match outcome {
            CommandOutcome::SessionResumed {
                id,
                mode,
                entries: got,
                ..
            } => {
                assert_eq!(id, "100-1");
                assert_eq!(mode, tupi_domain::AgentMode::Sdd);
                assert_eq!(got, entries);
            }
            _ => panic!("outcome errado"),
        }

        let _ = Command::ListSessions;
        let _ = Command::ResumeSession { id: "x".into() };
        let _ = Command::DeleteSession { id: "x".into() };
        let _ = CommandOutcome::SessionsList {
            items: vec![],
            ids: vec![],
        };
        let _ = CommandOutcome::SessionDeleted {
            id: "x".into(),
            found: false,
        };
    }
}
