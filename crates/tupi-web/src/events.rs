//! Coletor de eventos da sessão com ring buffer e stream SSE.

use std::collections::VecDeque;
use std::convert::Infallible;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use axum::extract::State;
use axum::response::sse::{Event as SseEvent, KeepAlive, Sse};
use futures_util::{Stream, StreamExt};
use serde::Serialize;
use tokio::sync::broadcast;
use tupi_domain::Event;
use tupi_kernel::EventBus;

use crate::routes::AppState;

const DEFAULT_BROADCAST: usize = 128;

/// Evento já normalizado para o navegador: `type` identifica a variante e
/// `seq` ordena a linha do tempo.
#[derive(Clone, Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum WebEvent {
    Token {
        seq: u64,
        request_id: u64,
        text: String,
    },
    StreamFinished {
        seq: u64,
        request_id: u64,
        prompt_tokens: Option<u64>,
        cached_tokens: Option<u64>,
        completion_tokens: Option<u64>,
    },
    ContextUpdated {
        seq: u64,
        request_id: u64,
        tokens: u64,
        budget: u64,
    },
    /// Novo teto de contexto depois de trocar de modelo: só o budget muda,
    /// sem inventar ocupação (o cliente preserva os tokens conhecidos).
    ContextBudget {
        seq: u64,
        budget: u64,
    },
    StreamFailed {
        seq: u64,
        request_id: u64,
        message: String,
    },
    ContextTrimmed {
        seq: u64,
        request_id: u64,
        estimate: usize,
        omitted_units: usize,
        shrunk_results: usize,
    },
    PromptMessages {
        seq: u64,
        request_id: u64,
        body: String,
    },
    ModelsLoaded {
        seq: u64,
        models: Vec<String>,
    },
    ModelsFailed {
        seq: u64,
        message: String,
    },
    Notice {
        seq: u64,
        message: String,
    },
    ToolStarted {
        seq: u64,
        request_id: u64,
        call_id: String,
        name: String,
        arguments: String,
        summary: String,
    },
    ToolFinished {
        seq: u64,
        request_id: u64,
        call_id: String,
        name: String,
        ok: bool,
        summary: String,
    },
    Memory {
        seq: u64,
        text: String,
        after: bool,
    },
    /// Pede ao cliente que recarregue a aba afetada.
    RefreshHint {
        seq: u64,
        scope: String,
    },
    /// O receptor ficou para trás e perdeu eventos.
    Lagged {
        seq: u64,
        skipped: u64,
    },
    ModeChanged {
        seq: u64,
        mode: u8,
    },
}

fn scope_for_tool(name: &str) -> &'static str {
    match name {
        "facts" | "state" | "brain_facts" | "brain_state" => "memory",
        "write" | "write_many" | "edit" | "patch" | "move" | "brain_write" | "brain_edit"
        | "brain_patch" | "brain_move" => "graph",
        _ => "workspace",
    }
}

fn map_event(event: Event, seq: &AtomicU64) -> Vec<WebEvent> {
    let next = || seq.fetch_add(1, Ordering::Relaxed);
    match event {
        Event::Token { request_id, text } => vec![WebEvent::Token {
            seq: next(),
            request_id,
            text,
        }],
        Event::StreamFinished {
            request_id,
            prompt_tokens,
            cached_tokens,
            completion_tokens,
        } => vec![WebEvent::StreamFinished {
            seq: next(),
            request_id,
            prompt_tokens,
            cached_tokens,
            completion_tokens,
        }],
        Event::ContextUpdated {
            request_id,
            tokens,
            budget,
        } => vec![WebEvent::ContextUpdated {
            seq: next(),
            request_id,
            tokens,
            budget,
        }],
        Event::ContextBudget { budget } => vec![WebEvent::ContextBudget {
            seq: next(),
            budget,
        }],
        Event::StreamFailed {
            request_id,
            message,
        } => vec![WebEvent::StreamFailed {
            seq: next(),
            request_id,
            message,
        }],
        Event::ContextTrimmed {
            request_id,
            estimate,
            omitted_units,
            shrunk_results,
        } => vec![WebEvent::ContextTrimmed {
            seq: next(),
            request_id,
            estimate,
            omitted_units,
            shrunk_results,
        }],
        Event::PromptMessages { request_id, body } => vec![WebEvent::PromptMessages {
            seq: next(),
            request_id,
            body,
        }],
        Event::ModelsLoaded { models } => vec![WebEvent::ModelsLoaded {
            seq: next(),
            models,
        }],
        Event::ModelsFailed { message } => vec![WebEvent::ModelsFailed {
            seq: next(),
            message,
        }],
        Event::Notice { message } => vec![WebEvent::Notice {
            seq: next(),
            message,
        }],
        Event::ToolStarted {
            request_id,
            call_id,
            name,
            arguments,
            summary,
        } => vec![WebEvent::ToolStarted {
            seq: next(),
            request_id,
            call_id,
            name,
            arguments,
            summary,
        }],
        Event::ToolFinished {
            request_id,
            call_id,
            name,
            ok,
            summary,
            duration_us: _,
        } => {
            let mut events = vec![WebEvent::ToolFinished {
                seq: next(),
                request_id,
                call_id,
                name: name.clone(),
                ok,
                summary,
            }];
            if ok {
                events.push(WebEvent::RefreshHint {
                    seq: next(),
                    scope: scope_for_tool(&name).to_string(),
                });
            }
            events
        }
        Event::Memory { text, after } => vec![WebEvent::Memory {
            seq: next(),
            text,
            after,
        }],
        Event::ModeChanged { mode } => vec![WebEvent::ModeChanged {
            seq: next(),
            mode: mode.as_u8(),
        }],
    }
}

/// Coletor de eventos: guarda os últimos `capacity` eventos e repassa os
/// novos para quem estiver conectado no SSE.
#[derive(Clone)]
pub(crate) struct EventHub {
    ring: Arc<Mutex<VecDeque<WebEvent>>>,
    tx: broadcast::Sender<WebEvent>,
}

impl EventHub {
    pub(crate) fn new(bus: &EventBus, capacity: usize) -> Self {
        Self::with_caps(bus, capacity, DEFAULT_BROADCAST)
    }

    pub(crate) fn with_caps(bus: &EventBus, capacity: usize, broadcast_capacity: usize) -> Self {
        let capacity = capacity.max(1);
        let ring = Arc::new(Mutex::new(VecDeque::with_capacity(capacity)));
        let (tx, _) = broadcast::channel(broadcast_capacity.max(1));
        let seq = Arc::new(AtomicU64::new(0));
        let mut receiver = bus.subscribe();

        let collector = HubCollector {
            ring: Arc::clone(&ring),
            tx: tx.clone(),
            seq: Arc::clone(&seq),
            capacity,
        };
        tokio::spawn(async move {
            loop {
                match receiver.recv().await {
                    Ok(event) => {
                        for converted in map_event(event, &collector.seq) {
                            collector.push(converted);
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(skipped)) => {
                        let seq = collector.seq.fetch_add(1, Ordering::Relaxed);
                        collector.push(WebEvent::Lagged { seq, skipped });
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
        });

        Self { ring, tx }
    }

    /// Snapshot do ring buffer e receptor para os eventos seguintes.
    pub(crate) fn subscribe(&self) -> (Vec<WebEvent>, broadcast::Receiver<WebEvent>) {
        let receiver = self.tx.subscribe();
        let snapshot = self
            .ring
            .lock()
            .map(|ring| ring.iter().cloned().collect())
            .unwrap_or_default();
        (snapshot, receiver)
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.ring.lock().map(|ring| ring.len()).unwrap_or(0)
    }
}

struct HubCollector {
    ring: Arc<Mutex<VecDeque<WebEvent>>>,
    tx: broadcast::Sender<WebEvent>,
    seq: Arc<AtomicU64>,
    capacity: usize,
}

impl HubCollector {
    fn push(&self, event: WebEvent) {
        if let Ok(mut ring) = self.ring.lock() {
            ring.push_back(event.clone());
            while ring.len() > self.capacity {
                ring.pop_front();
            }
        }
        let _ = self.tx.send(event);
    }
}

/// Stream usado pelo handler SSE: snapshot primeiro, depois eventos ao vivo.
pub(crate) fn sse_stream(
    snapshot: Vec<WebEvent>,
    rx: broadcast::Receiver<WebEvent>,
) -> impl Stream<Item = WebEvent> {
    let initial = futures_util::stream::iter(snapshot);
    let live = futures_util::stream::unfold(rx, |mut rx| async move {
        match rx.recv().await {
            Ok(event) => Some((event, rx)),
            Err(broadcast::error::RecvError::Lagged(skipped)) => {
                Some((WebEvent::Lagged { seq: 0, skipped }, rx))
            }
            Err(broadcast::error::RecvError::Closed) => None,
        }
    });
    initial.chain(live)
}

pub(crate) async fn events(
    State(state): State<AppState>,
) -> Sse<impl Stream<Item = Result<SseEvent, Infallible>>> {
    let (snapshot, rx) = state.hub.subscribe();
    let stream = sse_stream(snapshot, rx).map(|event| {
        let event = SseEvent::default().event("update").json_data(event);
        Ok(event.unwrap_or_else(|_| SseEvent::default().comment("evento inválido")))
    });
    Sse::new(stream).keep_alive(KeepAlive::default())
}
