mod command;
mod error;
mod event;
mod provider;
mod query;
mod registry;

pub use command::{Command, CommandBus, CommandHandler, CommandOutcome};
pub use error::{CommandError, ProviderError, QueryError};
pub use event::EventBus;
pub use provider::{ChatRequest, Provider, ProviderEvent, ProviderStream};
pub use query::{Query, QueryBus, QueryHandler, QueryResult};
pub use registry::ProviderRegistry;

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

    #[tokio::test]
    async fn lagged_subscriber_reports_lag() {
        let bus = EventBus::new(1);
        let mut rx = bus.subscribe();
        bus.publish(Event::StreamFinished { request_id: 1 });
        bus.publish(Event::StreamFinished { request_id: 2 });
        assert!(matches!(
            rx.recv().await,
            Err(tokio::sync::broadcast::error::RecvError::Lagged(_))
        ));
    }
}
