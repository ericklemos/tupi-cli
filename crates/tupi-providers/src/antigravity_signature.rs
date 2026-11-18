//! Store das assinaturas de pensamento (`thoughtSignature`) que o Google
//! devolve nas chamadas de ferramenta.
//!
//! Referencia: o plugin opencode-antigravity-auth guarda a assinatura por
//! sessao/modelo/projeto (signature-store) e manda
//! `skip_thought_signature_validator` apenas como fallback de cache miss. Aqui
//! `build_body` continua montando o envelope com a sentinela; este modulo troca
//! a sentinela pela assinatura real quando ela esta em cache, e o provider
//! alimenta o cache com o que veio no stream.

use std::collections::{HashMap, VecDeque};

use serde_json::Value;
use tupi_kernel::ChatRequest;

use crate::antigravity::build_body;

/// Valor de fallback que o provider manda quando nao ha assinatura real.
const SKIP_SIGNATURE: &str = "skip_thought_signature_validator";

/// Quantas assinaturas um mesmo escopo (sessao/modelo) guarda.
const SIGNATURES_PER_SCOPE: usize = 64;

/// Quantos escopos o store mantem antes de descartar o mais antigo.
const SIGNATURE_SCOPES: usize = 32;

/// Separador da chave de escopo (sessao e modelo nao usam '#').
const SCOPE_SEP: char = '#';

/// Assinaturas de pensamento por escopo (sessao/modelo) e id de chamada.
#[derive(Debug, Default, Clone)]
pub struct SignatureStore {
    scopes: HashMap<String, Vec<(String, String)>>,
    order: VecDeque<String>,
}

impl SignatureStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// Guarda a assinatura de uma chamada; a ultima vista para o id vence.
    pub fn insert(&mut self, scope: &str, call_id: &str, signature: &str) {
        if !self.scopes.contains_key(scope) {
            self.order.push_back(scope.to_string());
            while self.order.len() > SIGNATURE_SCOPES {
                let Some(oldest) = self.order.pop_front() else {
                    break;
                };
                self.scopes.remove(&oldest);
            }
        }
        let entries = self.scopes.entry(scope.to_string()).or_default();
        if let Some(slot) = entries.iter_mut().find(|(id, _)| id == call_id) {
            slot.1 = signature.to_string();
            return;
        }
        entries.push((call_id.to_string(), signature.to_string()));
        if entries.len() > SIGNATURES_PER_SCOPE {
            entries.remove(0);
        }
    }

    /// Assinatura real da chamada, quando o Google ja devolveu uma.
    pub fn get(&self, scope: &str, call_id: &str) -> Option<&str> {
        self.scopes
            .get(scope)
            .and_then(|entries| entries.iter().rev().find(|(id, _)| id == call_id))
            .map(|(_, signature)| signature.as_str())
    }

    /// Escopos guardados (limitado por `SIGNATURE_SCOPES`).
    pub fn scopes(&self) -> usize {
        self.scopes.len()
    }
}

/// Chave do store para um envelope de pedido: `conversationId` + `model` (o
/// `conversationId` ja carrega o projeto).
pub fn scope_of_envelope(envelope: &Value) -> Option<String> {
    let session = envelope.get("conversationId")?.as_str()?;
    let model = envelope.get("model")?.as_str()?;
    Some(format!("{session}{SCOPE_SEP}{model}"))
}

/// Troca a sentinela de fallback pela assinatura real em cada parte
/// `functionCall` cujo id esta em cache. Devolve quantas partes mudaram.
pub fn apply_cached_signatures(envelope: &mut Value, signatures: &SignatureStore) -> usize {
    let Some(scope) = scope_of_envelope(envelope) else {
        return 0;
    };
    let Some(contents) = envelope
        .get_mut("request")
        .and_then(|request| request.get_mut("contents"))
        .and_then(Value::as_array_mut)
    else {
        return 0;
    };

    let mut applied = 0;
    for message in contents {
        let Some(parts) = message.get_mut("parts").and_then(Value::as_array_mut) else {
            continue;
        };
        for part in parts {
            let Some(call_id) = part
                .get("functionCall")
                .and_then(|call| call.get("id"))
                .and_then(Value::as_str)
                .map(str::to_string)
            else {
                continue;
            };
            let Some(signature) = signatures.get(&scope, &call_id) else {
                continue;
            };
            part["thoughtSignature"] = Value::String(signature.to_string());
            applied += 1;
        }
    }
    applied
}

/// Colhe as assinaturas de um evento cru do stream (`data: {...}` do SSE) e
/// guarda no store sob o escopo do envelope. Payload nao-JSON (keepalive) ou
/// sem assinatura nao muda nada.
pub fn collect_signatures(payload: &str, scope: &str, store: &mut SignatureStore) {
    let Ok(root) = serde_json::from_str::<Value>(payload) else {
        return;
    };
    let response = root.get("response").unwrap_or(&root);
    let Some(candidates) = response.get("candidates").and_then(Value::as_array) else {
        return;
    };

    for candidate in candidates {
        let Some(parts) = candidate
            .get("content")
            .and_then(|content| content.get("parts"))
            .and_then(Value::as_array)
        else {
            continue;
        };
        for part in parts {
            let Some(signature) = part.get("thoughtSignature").and_then(Value::as_str) else {
                continue;
            };
            if signature.is_empty() || signature == SKIP_SIGNATURE {
                continue;
            }
            let Some(call_id) = part
                .get("functionCall")
                .and_then(|call| call.get("id"))
                .and_then(Value::as_str)
            else {
                continue;
            };
            store.insert(scope, call_id, signature);
        }
    }
}

/// Como `build_body`, mas reaproveitando as assinaturas ja vistas nesta
/// sessao/modelo. Sem cache o envelope sai igual ao de `build_body`, com a
/// sentinela na primeira chamada de ferramenta do turno.
pub fn build_body_with_signatures(
    request: &ChatRequest,
    project_id: &str,
    signatures: &SignatureStore,
) -> Value {
    let mut envelope = build_body(request, project_id);
    apply_cached_signatures(&mut envelope, signatures);
    envelope
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tupi_domain::{Message, ToolCall};

    const SKIP: &str = "skip_thought_signature_validator";

    fn request_with_call(call_id: &str) -> ChatRequest {
        let call = ToolCall {
            id: call_id.to_string(),
            name: "read_file".into(),
            arguments: r#"{"path":"a.txt"}"#.into(),
        };
        ChatRequest::new(
            "gemini-3.8-flash-high",
            vec![
                Message::user("leia o arquivo"),
                Message::assistant_with_calls("", vec![call]),
                Message::tool(call_id, "conteudo A"),
            ],
        )
    }

    fn first_call_signature(envelope: &Value) -> &Value {
        &envelope["request"]["contents"][1]["parts"][0]["thoughtSignature"]
    }

    #[test]
    fn without_cache_the_skip_sentinel_is_emitted() {
        let request = request_with_call("call_1");
        let envelope = build_body(&request, "proj-1");
        assert_eq!(first_call_signature(&envelope), SKIP);

        // Store vazio se comporta como o build_body: fallback, nao silencio.
        let envelope = build_body_with_signatures(&request, "proj-1", &SignatureStore::new());
        assert_eq!(first_call_signature(&envelope), SKIP);
    }

    #[test]
    fn cached_signature_replaces_the_skip_sentinel() {
        let request = request_with_call("call_1");
        let scope = scope_of_envelope(&build_body(&request, "proj-1")).unwrap();
        let mut store = SignatureStore::new();
        store.insert(&scope, "call_1", "sig-real-do-google");

        let envelope = build_body_with_signatures(&request, "proj-1", &store);
        assert_eq!(first_call_signature(&envelope), "sig-real-do-google");
        assert_ne!(first_call_signature(&envelope), SKIP);
    }

    #[test]
    fn signature_of_another_model_or_project_is_not_reused() {
        let request = request_with_call("call_1");
        let scope = scope_of_envelope(&build_body(&request, "proj-1")).unwrap();
        let mut store = SignatureStore::new();
        store.insert(&scope, "call_1", "sig-real-do-google");

        let other_project = build_body_with_signatures(&request, "proj-2", &store);
        assert_eq!(first_call_signature(&other_project), SKIP);

        let other_model = build_body_with_signatures(
            &ChatRequest::new("not-a-known-model", request_with_call("call_1").messages),
            "proj-1",
            &store,
        );
        assert_eq!(first_call_signature(&other_model), SKIP);
    }

    #[test]
    fn streamed_thought_signature_comes_back_on_the_next_turn() {
        let payload = json!({
            "response": {
                "candidates": [{
                    "content": {
                        "parts": [{
                            "functionCall": { "id": "call_1", "name": "read_file", "args": {} },
                            "thoughtSignature": "sig-do-stream"
                        }]
                    }
                }]
            }
        })
        .to_string();

        let request = request_with_call("call_1");
        let scope = scope_of_envelope(&build_body(&request, "proj-1")).unwrap();
        let mut store = SignatureStore::new();
        // Keepalive nao-JSON nao pode poluir nem derrubar o store.
        collect_signatures("keepalive", &scope, &mut store);
        collect_signatures(&payload, &scope, &mut store);

        let envelope = build_body_with_signatures(&request, "proj-1", &store);
        assert_eq!(first_call_signature(&envelope), "sig-do-stream");
    }

    #[test]
    fn skip_sentinel_in_the_stream_is_not_cached() {
        let payload = json!({
            "response": {
                "candidates": [{
                    "content": {
                        "parts": [{
                            "functionCall": { "id": "call_1", "name": "read_file", "args": {} },
                            "thoughtSignature": SKIP
                        }]
                    }
                }]
            }
        })
        .to_string();

        let request = request_with_call("call_1");
        let scope = scope_of_envelope(&build_body(&request, "proj-1")).unwrap();
        let mut store = SignatureStore::new();
        collect_signatures(&payload, &scope, &mut store);

        let envelope = build_body_with_signatures(&request, "proj-1", &store);
        assert_eq!(first_call_signature(&envelope), SKIP);
    }

    #[test]
    fn store_keeps_scope_count_bounded() {
        let mut store = SignatureStore::new();
        for index in 0..(SIGNATURE_SCOPES + 5) {
            store.insert(&format!("session-{index}#model"), "call_1", "sig");
        }
        assert_eq!(store.scopes(), SIGNATURE_SCOPES);
        assert!(store.get("session-0#model", "call_1").is_none());
        let newest = format!("session-{}#model", SIGNATURE_SCOPES + 4);
        assert_eq!(store.get(&newest, "call_1"), Some("sig"));
    }
}
