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
