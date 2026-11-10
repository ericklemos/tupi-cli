use std::sync::Arc;

use async_trait::async_trait;
use tupi_domain::RequestId;

use crate::CommandError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    SendMessage { text: String },
    CancelStream { request_id: RequestId },
    SetModel { model: String },
    RefreshModels,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommandOutcome {
    Started { request_id: RequestId },
    Cancelled,
    ModelChanged { model: String },
    Noop,
}

#[async_trait]
pub trait CommandHandler: Send + Sync {
    async fn handle(&self, command: Command) -> Result<CommandOutcome, CommandError>;
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
        self.handler.handle(command).await
    }
}
