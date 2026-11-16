//! Sonda T1.3: split provider×tool por turno a partir do JSONL real de
//! telemetria. Não faz requisição nenhuma: só agrega eventos já gravados.
//!
//! ```text
//! cargo test -p tupi --test turn_baseline -- --nocapture
//! # arquivo diferente do padrão:
//! TUPI_TELEMETRY_PATH=/caminho/telemetry.jsonl cargo test -p tupi --test turn_baseline -- --nocapture
//! ```
//!
//! A saída vai para `target/turn-baseline.txt` e é a fonte de
//! `docs/perf/turn-latency.md`. É `#[test]` porque o harness do agente só
//! executa `cargo build/check/test/clippy`.

use std::collections::BTreeMap;
use std::path::PathBuf;

#[derive(Default, Clone)]
struct Meta {
    provider: String,
    model: String,
    max_tool_steps: Option<u64>,
    context_budget: Option<u64>,
    max_tokens: Option<u64>,
}

#[derive(Default)]
struct Turn {
    provider_requests: u64,
    provider_us: u64,
    tool_calls: u64,
    tool_us: u64,
    steps: Option<u64>,
    duration_us: Option<u64>,
}

fn repo_root() -> PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("raiz do repositório")
}

fn ms(us: u64) -> String {
    format!("{:.0} ms", us as f64 / 1000.0)
}

fn share(part: u64, total: u64) -> String {
    if total == 0 {
        return "—".to_string();
    }
    format!("{:.0}%", part as f64 * 100.0 / total as f64)
}

#[test]
fn turn_baseline() {
    let root = repo_root();
    let path = std::env::var_os("TUPI_TELEMETRY_PATH")
        .map(PathBuf::from)
        .unwrap_or_else(|| root.join(".agents/tupi/debug/telemetry.jsonl"));
    let Ok(body) = std::fs::read_to_string(&path) else {
        eprintln!("sem {path:?}: sonda ignorada");
        return;
    };

    let mut metas: BTreeMap<String, Meta> = BTreeMap::new();
    let mut turns: BTreeMap<(String, u64), Turn> = BTreeMap::new();
    let mut malformed = 0usize;
    // Por tool: chamadas, ok, falhas, bytes, truncados — as mesmas colunas de
    // `ToolRow` (`crates/tupi/src/telemetry_report.rs`), agregadas por run.
    let mut tools: BTreeMap<(String, String), [u64; 5]> = BTreeMap::new();

    for line in body.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
            malformed += 1;
            continue;
        };
        let Some(event) = value["event"].as_str() else {
            malformed += 1;
            continue;
        };
        let run = value["run"].as_str().unwrap_or_default().to_string();
        match event {
            "run_started" => {
                metas.insert(
                    run,
                    Meta {
                        provider: value["provider"].as_str().unwrap_or("?").to_string(),
                        model: value["model"].as_str().unwrap_or("?").to_string(),
                        max_tool_steps: value["max_tool_steps"].as_u64(),
                        context_budget: value["context_budget"].as_u64(),
                        max_tokens: value["max_tokens"].as_u64(),
                    },
                );
            }
            "provider_response" | "tool_finished" | "turn_finished" => {
                let Some(request) = value["request"].as_u64() else {
                    malformed += 1;
                    continue;
                };
                let turn = turns.entry((run, request)).or_default();
                match event {
                    "provider_response" => {
                        turn.provider_requests += 1;
                        turn.provider_us += value["duration_us"].as_u64().unwrap_or_default();
                    }
                    "tool_finished" => {
                        turn.tool_calls += 1;
                        turn.tool_us += value["duration_us"].as_u64().unwrap_or_default();
                        // O nome viaja em cada `tool_finished`; sem ele o split
                        // não diz qual tool pagou o tempo.
                        if let Some(name) = value["name"].as_str() {
                            let of_run = value["run"].as_str().unwrap_or_default();
                            let row = tools
                                .entry((of_run.to_string(), name.to_string()))
                                .or_default();
                            row[0] += 1;
                            if value["ok"].as_bool().unwrap_or(false) {
                                row[1] += 1;
                            } else {
                                row[2] += 1;
                            }
                            row[3] += value["result_bytes"].as_u64().unwrap_or_default();
                            if value["truncated"].as_bool().unwrap_or(false) {
                                row[4] += 1;
                            }
                        }
                    }
                    _ => {
                        turn.steps = value["steps"].as_u64();
                        turn.duration_us = value["duration_us"].as_u64();
                    }
                }
            }
            _ => {}
        }
    }

    // O `run` começa pelo epoch em ms, então a última chave é o run mais recente.
    let latest = turns
        .iter()
        .filter(|(_, turn)| turn.duration_us.is_some() && turn.provider_requests > 0)
        .map(|((run, _), _)| run.clone())
        .next_back();

    let mut report = String::new();
    report.push_str("# split provider×tool por turno (telemetria real)\n");
    report.push_str(&format!("arquivo: {}\n", path.display()));
    report.push_str(&format!("linhas ilegíveis: {malformed}\n"));
    let Some(run) = latest else {
        report.push_str("nenhum turno com provider_response + turn_finished\n");
        eprintln!("{report}");
        let _ = std::fs::write(root.join("target/turn-baseline.txt"), &report);
        return;
    };
    let meta = metas.get(&run).cloned().unwrap_or_default();
    report.push_str(&format!(
        "run: {run} · provider {} · model {} · max_tool_steps {:?} · context_budget {:?} · max_tokens {:?}\n",
        meta.provider, meta.model, meta.max_tool_steps, meta.context_budget, meta.max_tokens
    ));

    let rows: Vec<(u64, &Turn)> = turns
        .iter()
        .filter(|((r, _), _)| r == &run)
        .map(|((_, request), turn)| (*request, turn))
        .collect();

    let (mut p_us, mut t_us, mut d_us, mut steps, mut calls, mut reqs) = (0, 0, 0, 0, 0, 0);
    report.push_str("\n## turnos\n\n");
    report
        .push_str("| turno | passos | requisições | provedor | por passo | tools | calls | total | resto |\n");
    report.push_str("| --- | --- | --- | --- | --- | --- | --- | --- | --- |\n");
    for (request, turn) in &rows {
        let total = turn.duration_us.unwrap_or_default();
        let rest = total.saturating_sub(turn.provider_us + turn.tool_us);
        let per_step = turn
            .steps
            .filter(|steps| *steps > 0)
            .map(|steps| turn.provider_us / steps)
            .unwrap_or_default();
        report.push_str(&format!(
            "| {request} | {} | {} | {} ({}) | {} | {} ({}) | {} | {} | {} ({}) |\n",
            turn.steps
                .map(|s| s.to_string())
                .unwrap_or_else(|| "?".into()),
            turn.provider_requests,
            ms(turn.provider_us),
            share(turn.provider_us, total),
            ms(per_step),
            ms(turn.tool_us),
            share(turn.tool_us, total),
            turn.tool_calls,
            ms(total),
            ms(rest),
            share(rest, total),
        ));
        p_us += turn.provider_us;
        t_us += turn.tool_us;
        d_us += turn.duration_us.unwrap_or_default();
        steps += turn.steps.unwrap_or_default();
        calls += turn.tool_calls;
        reqs += turn.provider_requests;
    }
    report.push_str(&format!(
        "\n## soma dos {} turnos do run\n\n- provedor: {} em {} requisições ({})\n- tools: {} em {} calls ({})\n- resto (montagem de prompt e espera fora das duas medidas): {} ({})\n- passos: {} · por passo: {} · por requisição: {}\n- por turno: {}\n",
        rows.len(),
        ms(p_us),
        reqs,
        share(p_us, d_us),
        ms(t_us),
        calls,
        share(t_us, d_us),
        ms(d_us.saturating_sub(p_us + t_us)),
        share(d_us.saturating_sub(p_us + t_us), d_us),
        steps,
        ms(if steps == 0 { 0 } else { p_us / steps }),
        ms(if reqs == 0 { 0 } else { p_us / reqs }),
        ms(if rows.is_empty() { 0 } else { d_us / rows.len() as u64 }),
    ));

    // Histograma por tool do run mais recente: cap por item e linha de
    // completude só se sustentam com bytes e truncados declarados por tool.
    let mut per_tool: Vec<(&String, &[u64; 5])> = tools
        .iter()
        .filter(|((r, _), _)| r == &run)
        .map(|((_, name), row)| (name, row))
        .collect();
    per_tool.sort_by(|a, b| b.1[0].cmp(&a.1[0]).then_with(|| a.0.cmp(b.0)));
    report.push_str("\n## tools do run\n\n");
    if per_tool.is_empty() {
        report.push_str("nenhum `tool_finished` com `name` no run\n");
    } else {
        let calls: u64 = per_tool.iter().map(|(_, row)| row[0]).sum();
        let truncated: u64 = per_tool.iter().map(|(_, row)| row[4]).sum();
        report.push_str(&format!(
            "{} tools chamadas · {} chamadas · {} truncadas\n\n",
            per_tool.len(),
            calls,
            truncated
        ));
        report.push_str("| tool | chamadas | ok | falhas | bytes | truncados |\n");
        report.push_str("| --- | --- | --- | --- | --- | --- |\n");
        for (name, row) in &per_tool {
            report.push_str(&format!(
                "| {name} | {} | {} | {} | {} | {} |\n",
                row[0], row[1], row[2], row[3], row[4]
            ));
        }
    }

    print!("{report}");
    let target = root.join("target/turn-baseline.txt");
    let _ = std::fs::write(&target, &report);
    eprintln!("relatório em {}", target.display());
}
