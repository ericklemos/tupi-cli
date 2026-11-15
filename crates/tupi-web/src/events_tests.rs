//! Testes do coletor de eventos e do stream SSE.

use std::time::Duration;

use futures_util::StreamExt;
use tupi_domain::Event;
use tupi_kernel::EventBus;

use crate::events::{EventHub, WebEvent, sse_stream};

async fn wait_for(hub: &EventHub, expected: usize) {
    for _ in 0..200 {
        if hub.len() >= expected {
            return;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    panic!(
        "ring buffer não recebeu {expected} eventos (tem {})",
        hub.len()
    );
}

fn is_notice(event: &WebEvent, text: &str) -> bool {
    matches!(event, WebEvent::Notice { message, .. } if message == text)
}

#[tokio::test]
async fn event_hub_replays_ring_buffer_and_emits_refresh_hint_on_mutating_tool() {
    let bus = EventBus::new(64);
    let hub = EventHub::new(&bus, 200);

    bus.publish(Event::Notice {
        message: "primeiro".to_string(),
    });
    bus.publish(Event::ToolFinished {
        request_id: 7,
        call_id: "call-1".to_string(),
        name: "write".to_string(),
        ok: true,
        summary: "gravou arquivo".to_string(),
        // O dashboard web não exibe duração; o campo existe para a TUI.
        duration_us: 1_500,
    });
    wait_for(&hub, 3).await;

    let (snapshot, _rx) = hub.subscribe();
    assert_eq!(snapshot.len(), 3);
    assert!(is_notice(&snapshot[0], "primeiro"));
    assert!(matches!(
        &snapshot[1],
        WebEvent::ToolFinished { name, ok: true, .. } if name == "write"
    ));
    assert!(matches!(
        &snapshot[2],
        WebEvent::RefreshHint { scope, .. } if scope == "graph"
    ));
}

#[tokio::test]
async fn sse_stream_replays_ring_buffer_and_survives_lagged_receiver() {
    let bus = EventBus::new(256);
    let hub = EventHub::with_caps(&bus, 64, 2);

    let (_, rx) = hub.subscribe();
    for index in 0..12 {
        bus.publish(Event::Notice {
            message: format!("n{index}"),
        });
    }
    wait_for(&hub, 12).await;

    let mut stream = Box::pin(sse_stream(Vec::new(), rx));
    let first = stream.next().await.expect("primeiro item");
    assert!(matches!(first, WebEvent::Lagged { skipped, .. } if skipped > 0));

    let second = stream.next().await.expect("segundo item");
    let third = stream.next().await.expect("terceiro item");
    let mut seen = 0;
    for event in [second, third] {
        if matches!(event, WebEvent::Notice { .. }) {
            seen += 1;
        }
    }
    assert_eq!(
        seen, 2,
        "o stream deve continuar entregando eventos após o lag"
    );
}

#[tokio::test]
async fn sse_stream_replays_snapshot_before_live_events() {
    let bus = EventBus::new(64);
    let hub = EventHub::new(&bus, 16);

    bus.publish(Event::Notice {
        message: "snapshot".to_string(),
    });
    wait_for(&hub, 1).await;

    let (snapshot, mut rx) = hub.subscribe();
    bus.publish(Event::Notice {
        message: "ao vivo".to_string(),
    });
    tokio::time::sleep(Duration::from_millis(20)).await;
    let _ = rx.recv().await;

    let mut stream = Box::pin(sse_stream(snapshot, hub.subscribe().1));
    let first = stream.next().await.unwrap();
    assert!(is_notice(&first, "snapshot"));
}

#[tokio::test]
async fn context_budget_event_carries_only_the_new_ceiling() {
    let bus = EventBus::new(8);
    let hub = EventHub::new(&bus, 8);

    bus.publish(Event::ContextBudget { budget: 12_000 });
    wait_for(&hub, 1).await;

    let (snapshot, _rx) = hub.subscribe();
    assert!(matches!(
        &snapshot[0],
        WebEvent::ContextBudget {
            seq: 0,
            budget: 12_000
        }
    ));
}
