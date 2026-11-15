use futures_util::StreamExt;
use serde_json::Value;
use tupi_domain::{Event, RequestId, ToolCall};
use tupi_kernel::pipeline::{PIPELINE_DEFAULT_BUDGET_TOKENS, PIPELINE_TOOL, PipelinePlan};
use tupi_kernel::{EventBus, ToolRegistry};

use crate::app::{describe_call, run_tool, summarize};

pub(crate) const MAX_PARALLEL: usize = 8;

/// Valida o plano inteiro antes de qualquer call rodar.
pub(crate) fn validate(plan: &PipelinePlan, tools: &ToolRegistry) -> Result<(), String> {
    if plan.steps.is_empty() {
        return Err("pipeline sem steps".to_string());
    }
    if plan.budget_tokens == Some(0) {
        return Err("budget_tokens precisa ser positivo".to_string());
    }

    for (index, step) in plan.steps.iter().enumerate() {
        if step.calls.is_empty() {
            return Err(format!("step {index} sem calls"));
        }
        for call in &step.calls {
            if call.tool == PIPELINE_TOOL {
                return Err(format!("step {index}: pipeline não pode chamar pipeline"));
            }
            if tools.get(&call.tool).is_none() {
                return Err(format!("step {index}: tool desconhecida: {}", call.tool));
            }
            if !call.arguments.is_object() {
                return Err(format!(
                    "step {index}: arguments de {} precisa ser objeto",
                    call.tool
                ));
            }
            scan_placeholders(&call.arguments, index)?;
        }
    }

    Ok(())
}

/// Substitui placeholders nos valores string, recursivamente.
pub(crate) fn resolve(arguments: &Value, step: usize, outputs: &[String]) -> Result<Value, String> {
    match arguments {
        Value::String(text) => Ok(Value::String(substitute(text, step, outputs)?)),
        Value::Array(items) => items
            .iter()
            .map(|item| resolve(item, step, outputs))
            .collect::<Result<Vec<_>, _>>()
            .map(Value::Array),
        Value::Object(map) => {
            let mut out = serde_json::Map::new();
            for (key, value) in map {
                out.insert(key.clone(), resolve(value, step, outputs)?);
            }
            Ok(Value::Object(out))
        }
        other => Ok(other.clone()),
    }
}

/// Uma seção da concatenação de uma step.
pub(crate) fn section(tool: &str, summary: &str, body: &str) -> String {
    format!("### {tool} {summary}\n{body}")
}

pub(crate) fn concat(sections: &[String]) -> String {
    sections.join("\n\n")
}

/// Corta a saída final no orçamento, em fronteira de char, com marcador.
pub(crate) fn truncate(text: &str, budget_tokens: usize) -> String {
    if tupi_domain::tokens::estimate_tokens(text) <= budget_tokens {
        return text.to_string();
    }

    let mut lo = 0usize;
    let mut hi = text.len();
    while lo < hi {
        let mid = lo + (hi - lo).div_ceil(2);
        let mut cut = mid;
        while cut > lo && !text.is_char_boundary(cut) {
            cut -= 1;
        }
        if cut == lo {
            break;
        }
        if tupi_domain::tokens::estimate_tokens(&text[..cut]) <= budget_tokens {
            lo = cut;
        } else {
            hi = cut - 1;
        }
    }

    let body = text[..lo].trim_end();
    format!("{body}\n\n… (truncado: orçamento de {budget_tokens} tokens)")
}

/// Executa a pipeline e devolve só a concatenação da última step.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn execute(
    tools: &ToolRegistry,
    events: &EventBus,
    request_id: RequestId,
    call: &ToolCall,
    telemetry: &crate::telemetry::Telemetry,
    cancel_flag: &std::sync::Arc<std::sync::atomic::AtomicBool>,
    in_flight: &std::sync::Arc<tokio::sync::Notify>,
    in_flight_count: &std::sync::Arc<std::sync::atomic::AtomicUsize>,
) -> Result<String, String> {
    fn record_finished(
        telemetry: &crate::telemetry::Telemetry,
        request_id: RequestId,
        call: &ToolCall,
        result: &Result<String, String>,
        duration: std::time::Duration,
    ) {
        if !telemetry.is_enabled() {
            return;
        }

        let content = match result {
            Ok(content) => content,
            Err(error) => error,
        };
        let clipped = crate::telemetry::clip(content);
        let (text, error) = if result.is_ok() {
            (Some(clipped.text), None)
        } else {
            (None, Some(clipped.text))
        };
        let summary = crate::app::summarize(content);

        telemetry.record(crate::telemetry::TelemetryEvent::ToolFinished(
            crate::telemetry::ToolFinished {
                request: request_id,
                call: call.id.clone(),
                name: call.name.clone(),
                duration_us: crate::telemetry::micros(duration),
                ok: result.is_ok(),
                result: text,
                result_chars: clipped.chars,
                result_bytes: clipped.bytes,
                truncated: crate::telemetry::truncated(content, clipped.truncated),
                summary,
                error,
            },
        ));
    }

    let plan: PipelinePlan = serde_json::from_str(&call.arguments)
        .map_err(|error| format!("pipeline inválida: {error}"))?;
    validate(&plan, tools)?;

    let mut outputs: Vec<String> = Vec::with_capacity(plan.steps.len());

    for (step_index, step) in plan.steps.iter().enumerate() {
        let inner: Vec<ToolCall> = step
            .calls
            .iter()
            .enumerate()
            .map(|(index, pipeline_call)| {
                let arguments = resolve(&pipeline_call.arguments, step_index, &outputs)?;
                Ok(ToolCall {
                    id: format!("{}#{step_index}.{index}", call.id),
                    name: pipeline_call.tool.clone(),
                    arguments: arguments.to_string(),
                })
            })
            .collect::<Result<Vec<_>, String>>()?;

        for inner_call in &inner {
            let summary = describe_call(tools, inner_call);
            events.publish(Event::ToolStarted {
                request_id,
                call_id: inner_call.id.clone(),
                name: inner_call.name.clone(),
                arguments: inner_call.arguments.clone(),
                summary: summary.clone(),
            });

            if telemetry.is_enabled() {
                let arguments = crate::telemetry::clip(&inner_call.arguments);
                telemetry.record(crate::telemetry::TelemetryEvent::ToolStarted(
                    crate::telemetry::ToolStarted {
                        request: request_id,
                        call: inner_call.id.clone(),
                        name: inner_call.name.clone(),
                        arguments: arguments.text,
                        arguments_chars: arguments.chars,
                        arguments_bytes: arguments.bytes,
                        truncated: arguments.truncated,
                        summary,
                    },
                ));
            }
        }

        let serial = step.calls.iter().any(|pipeline_call| {
            tools
                .get(&pipeline_call.tool)
                .map(|tool| tool.mutates())
                .unwrap_or(false)
        });

        let mut results: Vec<Option<Result<String, String>>> = vec![None; inner.len()];

        if serial {
            for (index, inner_call) in inner.iter().enumerate() {
                let started = std::time::Instant::now();
                let result =
                    run_tool(tools, inner_call, cancel_flag, in_flight, in_flight_count).await;
                let duration = started.elapsed();
                record_finished(telemetry, request_id, inner_call, &result, duration);
                publish_finished(events, request_id, inner_call, &result, duration);
                results[index] = Some(result);
            }
        } else {
            let mut pending = futures_util::stream::iter(inner.iter().cloned().enumerate())
                .map(|(index, inner_call)| {
                    let cancel_flag = std::sync::Arc::clone(cancel_flag);
                    let in_flight = std::sync::Arc::clone(in_flight);
                    let in_flight_count = std::sync::Arc::clone(in_flight_count);
                    async move {
                        let started = std::time::Instant::now();
                        let result = run_tool(
                            tools,
                            &inner_call,
                            &cancel_flag,
                            &in_flight,
                            &in_flight_count,
                        )
                        .await;
                        (index, result, started.elapsed())
                    }
                })
                .buffer_unordered(MAX_PARALLEL);

            while let Some((index, result, duration)) = pending.next().await {
                record_finished(telemetry, request_id, &inner[index], &result, duration);
                publish_finished(events, request_id, &inner[index], &result, duration);
                results[index] = Some(result);
            }
        }

        let mut sections = Vec::with_capacity(inner.len());
        for (inner_call, result) in inner.iter().zip(results) {
            let result = result.expect("toda call da step produz resultado");
            sections.push(match result {
                Ok(content) => section(
                    &inner_call.name,
                    &describe_call(tools, inner_call),
                    &content,
                ),
                Err(error) => section(&inner_call.name, "— falhou", &error),
            });
        }
        outputs.push(concat(&sections));
    }

    let budget = plan.budget_tokens.unwrap_or(PIPELINE_DEFAULT_BUDGET_TOKENS);
    Ok(truncate(
        outputs.last().map(String::as_str).unwrap_or(""),
        budget,
    ))
}

fn publish_finished(
    events: &EventBus,
    request_id: RequestId,
    call: &ToolCall,
    result: &Result<String, String>,
    duration: std::time::Duration,
) {
    let (ok, summary) = match result {
        Ok(content) => (true, summarize(content)),
        Err(error) => (false, summarize(error)),
    };
    events.publish(Event::ToolFinished {
        request_id,
        call_id: call.id.clone(),
        name: call.name.clone(),
        ok,
        summary,
        // Medido ao redor da chamada que roda a tool, não no consumidor: o
        // evento atravessa o bus e pode ser lido bem depois. O piso de 1µs
        // mantém `duration_us > 0` numa chamada instantânea, para que zero
        // nunca signifique "não medido".
        duration_us: crate::telemetry::micros(duration).max(1),
    });
}

fn scan_placeholders(value: &Value, step: usize) -> Result<(), String> {
    match value {
        Value::String(text) => substitute(text, step, &[]).map(|_| ()),
        Value::Array(items) => {
            for item in items {
                scan_placeholders(item, step)?;
            }
            Ok(())
        }
        Value::Object(map) => {
            for item in map.values() {
                scan_placeholders(item, step)?;
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

fn substitute(text: &str, step: usize, outputs: &[String]) -> Result<String, String> {
    let mut out = String::new();
    let mut rest = text;

    while let Some(start) = rest.find("{{") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        let Some(end) = after.find("}}") else {
            return Err(format!("placeholder sem fechamento: {}", &rest[start..]));
        };
        let key = &after[..end];
        let value = match key {
            "prev" => {
                if step == 0 {
                    return Err("{{prev}} não existe na primeira step".to_string());
                }
                outputs.get(step - 1).map(String::as_str).unwrap_or("")
            }
            _ => {
                let Some(index) = key
                    .strip_prefix("step")
                    .and_then(|number| number.parse::<usize>().ok())
                else {
                    return Err(format!("placeholder desconhecido: {{{{{key}}}}}"));
                };
                if index >= step {
                    return Err(format!(
                        "{{{{step{index}}}}} ainda não existe na step {step}"
                    ));
                }
                outputs.get(index).map(String::as_str).unwrap_or("")
            }
        };
        out.push_str(value);
        rest = &after[end + 2..];
    }

    out.push_str(rest);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plan(json: &str) -> PipelinePlan {
        serde_json::from_str(json).unwrap()
    }

    #[test]
    fn placeholders_use_the_previous_or_an_earlier_step() {
        let args = serde_json::json!({"content": "antes {{prev}} depois {{step0}}"});
        let out = resolve(&args, 2, &["A".into(), "B".into()]).unwrap();
        assert_eq!(out["content"], "antes B depois A");
    }

    #[test]
    fn a_typo_or_a_future_step_is_refused() {
        assert!(resolve(&serde_json::json!({"x": "{{step9}}"}), 1, &["A".into()]).is_err());
        assert!(resolve(&serde_json::json!({"x": "{{prevv}}"}), 1, &["A".into()]).is_err());
        assert!(resolve(&serde_json::json!({"x": "{{prev}}"}), 0, &[]).is_err());
    }

    #[test]
    fn only_strings_are_substituted() {
        let out = resolve(&serde_json::json!({"n": 3, "ok": true}), 0, &[]).unwrap();
        assert_eq!(out["n"], 3);
        assert_eq!(out["ok"], true);
    }

    #[test]
    fn validation_refuses_empty_nested_and_unknown_tools() {
        let tools = ToolRegistry::new();
        assert!(validate(&plan(r#"{"steps":[]}"#), &tools).is_err());
        assert!(validate(&plan(r#"{"steps":[{"calls":[]}]}"#), &tools).is_err());
        assert!(
            validate(
                &plan(r#"{"steps":[{"calls":[{"tool":"pipeline","arguments":{}}]}]}"#),
                &tools
            )
            .is_err()
        );
        assert!(
            validate(
                &plan(r#"{"steps":[{"calls":[{"tool":"nao_existe","arguments":{}}]}]}"#),
                &tools
            )
            .is_err()
        );
    }

    #[test]
    fn sections_keep_the_declared_order() {
        let text = concat(&[
            section("read", "a.rs", "um"),
            section("read", "b.rs", "dois"),
        ]);
        assert!(text.find("a.rs").unwrap() < text.find("b.rs").unwrap());
        assert!(text.contains("### read a.rs\num"));
    }

    #[test]
    fn truncation_marker_is_explicit_and_cuts_on_a_char_boundary() {
        let text = "á".repeat(5000);
        let out = truncate(&text, 10);
        assert!(out.contains("truncado"));
        assert!(out.len() < text.len());

        let body = out.split("\n\n…").next().unwrap();
        assert!(body.chars().all(|ch| ch == 'á'));
        assert!(tupi_domain::tokens::estimate_tokens(body) <= 10);
    }
}
