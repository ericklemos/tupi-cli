mod command;
pub use command::TranscriptEntry;
mod error;
mod event;
mod provider;
mod query;
mod registry;
mod tool;
pub use tool::ModeMask;
mod sdd_config;
pub use sdd_config::{SddConfig, SddExecution, SddPlanning, SddRunner, SddToolConfig, sdd_config};

mod workflow;

pub use command::{Command, CommandBus, CommandHandler, CommandOutcome, MemoryAction};
pub use error::{CommandError, ProviderError, QueryError};
pub use event::EventBus;
pub use provider::{ChatRequest, Provider, ProviderEvent, ProviderStream};
pub use query::{Query, QueryBus, QueryHandler, QueryResult};
pub use registry::ProviderRegistry;
pub use tool::{
    Tool, ToolError, ToolRegistry, ToolSpec, current_tool_cancel, is_tool_cancelled,
    tools_payload, with_tool_cancel,
};
pub use workflow::WorkflowIndex;

pub use tupi_domain::AgentMode;

#[cfg(test)]
mod tests {
    use super::*;
    use tupi_domain::Event;

    #[test]
    fn unknown_provider_returns_none() {
        let registry = ProviderRegistry::new();
        assert!(registry.get("deepseek").is_none());
        assert!(registry.ids().is_empty());
    }

    #[test]
    fn command_connect_and_event_notice_variants() {
        let cmd = Command::Connect {
            provider: "antigravity".into(),
        };
        match cmd {
            Command::Connect { provider } => assert_eq!(provider, "antigravity"),
            _ => panic!("esperava Command::Connect"),
        }

        let evt = Event::Notice {
            message: "logando".into(),
        };
        match evt {
            Event::Notice { message } => assert_eq!(message, "logando"),
            _ => panic!("esperava Event::Notice"),
        }
    }

    #[tokio::test]
    async fn lagged_subscriber_reports_lag() {
        let bus = EventBus::new(1);
        let mut rx = bus.subscribe();
        bus.publish(Event::StreamFinished {
            request_id: 1,
            prompt_tokens: None,
            cached_tokens: None,
            completion_tokens: None,
        });
        bus.publish(Event::StreamFinished {
            request_id: 2,
            prompt_tokens: None,
            cached_tokens: None,
            completion_tokens: None,
        });
        assert!(matches!(
            rx.recv().await,
            Err(tokio::sync::broadcast::error::RecvError::Lagged(_))
        ));
    }
}
pub mod decide;
pub mod embed;
pub mod generate;
pub mod pipeline;
