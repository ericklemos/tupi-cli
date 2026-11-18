//! Sonda T3.3: quanto o dedup por `(path, blake3)` rende num turno real, lido do
//! JSONL de telemetria **já gravado**.
//!
//! Nada de instrumentação nova: os corpos já estão em `tool_finished.result`
//! (`ToolFinished.result`, cortado em 512 KiB por `clip`), na ordem em que foram
//! produzidos. Para cada turno (`run`, `request`) a sonda reconstrói o histórico
//! de resultados que o `plan_with` veria em cada passo e roda o dedup de
//! verdade, `tupi_domain::context::dedup_identical_results`.
//!
//! Duas colunas importam:
//!
//! - **teto**: tokens que o dedup pouparia no passo, sem perguntar se a poda
//!   roda. É o ganho disponível no histórico daquele passo.
//! - **real**: idem, mas zerado nos passos em que `plan_with` nem chega ao dedup
//!   — o caminho rápido devolve o histórico intacto quando a estimativa local
//!   não passa do orçamento (`crates/tupi-domain/src/context.rs:114-116`). A
//!   estimativa vem de `provider_response.estimate_tokens`, a mesma que o app
//!   faz antes de enviar.
//!
//! Rodar:
//!
//! ```text
//! cargo test -p tupi-app --test dedup_gain -- --nocapture
//! TUPI_TELEMETRY_PATH=/caminho/telemetry.jsonl \
//!   cargo test -p tupi-app --test dedup_gain -- --nocapture
//! ```
//!
//! O relatório também é gravado em `target/dedup-gain.txt`.

use std::collections::BTreeMap;
use std::path::PathBuf;

use tupi_domain::Message;
use tupi_domain::context::dedup_identical_results;
use tupi_domain::tokens::estimate_tokens;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|path| path.parent())
        .map(|path| path.to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."))
}

/// Um resultado de tool como a telemetria o gravou.
#[derive(Clone)]
struct Recorded {
    /// Passo cuja resposta do provedor pediu a call.
    step: u64,
    name: String,
    content: String,
}

impl Recorded {
    fn message(&self) -> Message {
        Message::tool("call", self.content.clone())
    }
}

#[derive(Default, Clone)]
struct Gain {
    /// Σ dos passos: tokens que o dedup pouparia no snapshot daquele passo.
    ceiling_tokens: u64,
    /// Σ dos passos em que a poda de fato roda.
    real_tokens: u64,
    /// Σ dos passos: resultados trocados por ponteiro.
    duplicates: u64,
    /// Duplicatas por nome de tool, somadas nos passos.
    by_tool: BTreeMap<String, u64>,
}

/// Dedup do snapshot do passo: quantos resultados mudariam, quantos tokens saem
/// e de que tool veio cada um.
fn measure(snapshot: &[&Recorded]) -> (usize, u64, BTreeMap<String, u64>) {
    let history: Vec<Message> = snapshot.iter().map(|record| record.message()).collect();
    let (deduped, _) = dedup_identical_results(&history);

    let mut saved = 0u64;
    let mut by_tool: BTreeMap<String, u64> = BTreeMap::new();
    for (index, message) in deduped.iter().enumerate() {
        if message.content == history[index].content {
            continue;
        }
        let before = estimate_tokens(&history[index].content) as u64;
        let after = estimate_tokens(&message.content) as u64;
        saved += before.saturating_sub(after);
        *by_tool.entry(snapshot[index].name.clone()).or_default() += 1;
    }

    (by_tool.values().sum::<u64>() as usize, saved, by_tool)
}

#[test]
fn dedup_gain() {
    let root = repo_root();
    let path = std::env::var_os("TUPI_TELEMETRY_PATH")
        .map(PathBuf::from)
        .unwrap_or_else(|| root.join(".agents/tupi/debug/telemetry.jsonl"));
    let Ok(body) = std::fs::read_to_string(&path) else {
        eprintln!("sem {path:?}: sonda ignorada");
        return;
    };

    let mut runs: BTreeMap<String, (String, String, Option<u64>, Option<u64>)> = BTreeMap::new();
    // Estimativa local do prompt enviado, por (run, request, step).
    let mut estimates: BTreeMap<(String, u64), BTreeMap<u64, u64>> = BTreeMap::new();
    let mut results: BTreeMap<(String, u64), Vec<Recorded>> = BTreeMap::new();
    let mut current_step: BTreeMap<(String, u64), u64> = BTreeMap::new();
    let mut malformed = 0usize;

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
                runs.insert(
                    run,
                    (
                        value["provider"].as_str().unwrap_or("?").to_string(),
                        value["model"].as_str().unwrap_or("?").to_string(),
                        value["context_budget"].as_u64(),
                        value["max_tool_steps"].as_u64(),
                    ),
                );
            }
            "provider_response" => {
                let Some(request) = value["request"].as_u64() else {
                    continue;
                };
                let step = value["step"].as_u64().unwrap_or(0);
                let estimate = value["estimate_tokens"].as_u64().unwrap_or_default();
                estimates
                    .entry((run.clone(), request))
                    .or_default()
                    .insert(step, estimate);
                current_step.insert((run, request), step);
            }
            "tool_finished" => {
                let Some(request) = value["request"].as_u64() else {
                    continue;
                };
                if !value["ok"].as_bool().unwrap_or(false) {
                    continue;
                }
                let Some(content) = value["result"].as_str() else {
                    continue;
                };
                let key = (run, request);
                results.entry(key.clone()).or_default().push(Recorded {
                    step: current_step.get(&key).copied().unwrap_or(0),
                    name: value["name"].as_str().unwrap_or("?").to_string(),
                    content: content.to_string(),
                });
            }
            _ => {}
        }
    }

    let mut report = String::new();
    report.push_str("# ganho do dedup por (path, blake3) — telemetria real\n");
    report.push_str(&format!("arquivo: {}\n", path.display()));
    report.push_str(&format!("linhas ilegíveis: {malformed}\n"));

    let mut gains: BTreeMap<(String, u64), Gain> = BTreeMap::new();
    let mut turns_with_duplicates = 0usize;

    for (key, entries) in &results {
        let (run, _request) = key;
        let budget = runs
            .get(run)
            .and_then(|(_, _, budget, _)| *budget)
            .unwrap_or_default();
        let steps = entries.iter().map(|entry| entry.step).max().unwrap_or(0);
        let mut gain = Gain::default();

        // O prompt do passo `step` carrega os resultados produzidos antes dele.
        for step in 1..=steps.saturating_add(1) {
            let snapshot: Vec<&Recorded> =
                entries.iter().filter(|entry| entry.step < step).collect();
            if snapshot.is_empty() {
                continue;
            }
            let (duplicates, tokens, by_tool) = measure(&snapshot);
            let estimate = estimates
                .get(key)
                .and_then(|steps| steps.get(&step))
                .copied()
                .unwrap_or_default();
            gain.ceiling_tokens += tokens;
            gain.duplicates += duplicates as u64;
            for (name, count) in by_tool {
                *gain.by_tool.entry(name).or_default() += count;
            }
            // `plan_with` devolve o histórico intacto enquanto a estimativa couber
            // no orçamento; dedup só roda quando ele age.
            if budget > 0 && estimate > budget {
                gain.real_tokens += tokens;
            }
        }

        if gain.duplicates > 0 {
            turns_with_duplicates += 1;
        }
        gains.insert(key.clone(), gain);
    }

    report.push_str(&format!(
        "\n{} turno(s) com resultado de tool · {} com duplicata\n",
        gains.len(),
        turns_with_duplicates
    ));
    report.push_str("\n| run | turno | orçamento | duplicatas | tokens/passo (teto) | tokens/passo (real) | tools |\n");
    report.push_str("| --- | --- | --- | --- | --- | --- | --- |\n");

    let (mut all_duplicates, mut all_ceiling, mut all_real) = (0u64, 0u64, 0u64);
    let mut all_tools: BTreeMap<String, u64> = BTreeMap::new();
    for (key, gain) in &gains {
        let (run, request) = key;
        let budget = runs
            .get(run)
            .and_then(|(_, _, budget, _)| *budget)
            .map(|budget| budget.to_string())
            .unwrap_or_else(|| "?".into());
        let tools = gain
            .by_tool
            .iter()
            .map(|(name, count)| format!("{name} {count}"))
            .collect::<Vec<_>>();
        report.push_str(&format!(
            "| {run} | {request} | {budget} | {} | {} | {} | {} |\n",
            gain.duplicates,
            gain.ceiling_tokens,
            gain.real_tokens,
            if tools.is_empty() {
                "—".to_string()
            } else {
                tools.join(", ")
            }
        ));
        all_duplicates += gain.duplicates;
        all_ceiling += gain.ceiling_tokens;
        all_real += gain.real_tokens;
        for (name, count) in &gain.by_tool {
            *all_tools.entry(name.clone()).or_default() += count;
        }
    }

    report.push_str(&format!(
        "\n## soma do arquivo\n\n- duplicatas: {all_duplicates}\n- tokens/passo no teto: {all_ceiling}\n- tokens/passo reais: {all_real}\n"
    ));
    if all_tools.is_empty() {
        report.push_str("- duplicatas por tool: nenhuma\n");
    } else {
        report.push_str("- duplicatas por tool: ");
        let rows: Vec<String> = all_tools
            .iter()
            .map(|(name, count)| format!("{name} {count}"))
            .collect();
        report.push_str(&format!("{}\n", rows.join(" · ")));
    }

    report.push_str(&format!(
        "\nrun mais recente com resultado: {}\n",
        gains
            .keys()
            .map(|(run, _)| run.clone())
            .next_back()
            .unwrap_or_default()
    ));

    print!("{report}");
    let target = root.join("target/dedup-gain.txt");
    let _ = std::fs::write(&target, &report);
    eprintln!("relatório em {}", target.display());
}
