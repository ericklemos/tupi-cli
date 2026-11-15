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
    /// Teto de tokens da resposta. `None` deixa o padrão do provedor.
    pub max_tokens: Option<usize>,
}

impl ChatRequest {
    pub fn new(model: impl Into<String>, messages: Vec<Message>) -> Self {
        Self {
            model: model.into(),
            messages,
            tools: Vec::new(),
            max_tokens: None,
        }
    }

    pub fn with_tools(mut self, tools: Vec<ToolSpec>) -> Self {
        self.tools = tools;
        self
    }

    /// `0` significa "sem teto": a chave não é enviada ao provedor.
    pub fn with_max_tokens(mut self, max_tokens: usize) -> Self {
        self.max_tokens = (max_tokens > 0).then_some(max_tokens);
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProviderEvent {
    /// Primeiro chunk chegou do provedor. Não carrega conteúdo; serve para
    /// medir TTFT antes de o parser produzir texto, tool call ou usage.
    Started,
    Text(String),
    ToolCalls(Vec<ToolCall>),
    /// O provedor encerrou a resposta e disse por quê (`stop`, `length`, …).
    /// Vem depois de texto/tool calls e antes do fim do stream. `length`
    /// significa que a resposta foi cortada no teto de tokens.
    Finished(String),
    /// O que o provedor reportou de consumo. Cada campo é opcional porque
    /// nem todo provedor manda tudo, e nem sempre no mesmo chunk.
    Usage {
        prompt_tokens: Option<u64>,
        cached_tokens: Option<u64>,
        completion_tokens: Option<u64>,
    },
}

pub type ProviderStream = Pin<Box<dyn Stream<Item = Result<ProviderEvent, ProviderError>> + Send>>;

#[async_trait]
pub trait Provider: Send + Sync {
    fn id(&self) -> &str;

    async fn list_models(&self) -> Result<Vec<String>, ProviderError>;

    async fn stream_chat(&self, request: ChatRequest) -> Result<ProviderStream, ProviderError>;

    /// Corpo exato da última requisição enviada ao provedor, para depuração.
    fn last_payload(&self) -> Option<serde_json::Value> {
        None
    }
}
