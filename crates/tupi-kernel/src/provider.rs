use std::pin::Pin;

use async_trait::async_trait;
use futures_core::Stream;
use tupi_domain::{Message, ToolCall};

use crate::{ProviderError, ToolSpec};

#[derive(Debug, Clone, PartialEq)]
pub struct ChatRequest {
    pub model: String,
    pub messages: Vec<Message>,
    pub tools: Vec<ToolSpec>,
}

impl ChatRequest {
    pub fn new(model: impl Into<String>, messages: Vec<Message>) -> Self {
        Self {
            model: model.into(),
            messages,
            tools: Vec::new(),
        }
    }

    pub fn with_tools(mut self, tools: Vec<ToolSpec>) -> Self {
        self.tools = tools;
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProviderEvent {
    Text(String),
    ToolCalls(Vec<ToolCall>),
    Usage { completion_tokens: u64 },
}

pub type ProviderStream = Pin<Box<dyn Stream<Item = Result<ProviderEvent, ProviderError>> + Send>>;

#[async_trait]
pub trait Provider: Send + Sync {
    fn id(&self) -> &str;

    async fn list_models(&self) -> Result<Vec<String>, ProviderError>;

    async fn stream_chat(&self, request: ChatRequest) -> Result<ProviderStream, ProviderError>;
}
