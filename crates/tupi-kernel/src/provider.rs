use std::pin::Pin;

use async_trait::async_trait;
use futures_core::Stream;
use tupi_domain::Message;

use crate::ProviderError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChatRequest {
    pub model: String,
    pub messages: Vec<Message>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProviderEvent {
    Text(String),
}

pub type ProviderStream = Pin<Box<dyn Stream<Item = Result<ProviderEvent, ProviderError>> + Send>>;

#[async_trait]
pub trait Provider: Send + Sync {
    fn id(&self) -> &str;

    async fn list_models(&self) -> Result<Vec<String>, ProviderError>;

    async fn stream_chat(&self, request: ChatRequest) -> Result<ProviderStream, ProviderError>;
}
