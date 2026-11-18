//! `tupi telemetry`: relatório legível do split provider×tool por turno.
//!
//! O split já é somado no caminho quente (`Stats::absorb`,
//! `crates/tupi-app/src/telemetry.rs:285-335`), mas como total do processo. Aqui
//! a conta é refeita por turno lendo os eventos que já foram gravados: nenhum
//! evento novo é emitido e o turno não paga passo nenhum por isso.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

pub const USAGE: &str = "uso: tupi telemetry [--path ARQUIVO] [--limit N]\n\
     lê o JSONL de telemetria e imprime, por turno, passos, Σprovider_us, \
     Σtool_us e prompt/cached/completion (--limit 0 mostra todos)";

/// Turnos exibidos quando ninguém passa `--limit`.
pub const DEFAULT_LIMIT: usize = 20;

/// Uma linha do relatório: um turno, com o tempo do modelo separado do tempo
/// de tool.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TurnRow {
    pub run: String,
    pub request: u64,
    /// `None` quando o turno não chegou a gravar `turn_finished`.
    pub steps: Option<usize>,
    pub provider_us: u64,
    pub provider_requests: u64,
    pub tool_us: u64,
    pub tool_calls: u64,
    /// `read`/`read_many`/`peek` do turno: a re-leitura que o manifesto do
    /// turno existe para cortar (T6).
    pub rereads: u64,
    /// Parede do turno (`turn_finished.duration_us`).
    pub duration_us: Option<u64>,
    pub prompt_tokens: u64,
    pub cached_tokens: u64,
    pub completion_tokens: u64,
    /// O passo a passo do turno, na ordem em que os passos foram gravados.
    pub step_rows: Vec<StepRow>,
}

/// Um passo do turno: a escala de correção **em vigor no envio**
/// (`provider_response.scale_ppm`) e a fatia cacheada do prompt cobrado.
///
/// O total do turno esconde as duas coisas que decidem custo: uma escala que
/// oscila dentro do turno troca o request a cada passo e derruba
/// `cached_tokens`; uma escala monotônica preserva o prefixo. Por isso os dois
/// números aparecem por passo, não somados.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct StepRow {
    pub step: u64,
    /// `None` quando a linha não trazia `scale_ppm` (arquivo de schema antigo).
    pub scale_ppm: Option<u64>,
    pub prompt_tokens: u64,
    pub cached_tokens: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Report {
    pub turns: Vec<TurnRow>,
    /// Linhas que não eram JSON de evento; contadas, nunca escondidas.
    pub malformed: usize,
    /// Agregado por `ToolFinished.name` do arquivo inteiro: o histograma que
    /// P5 exige antes de cortar qualquer coisa (sem número, não fica).
    pub tools: std::collections::BTreeMap<String, ToolRow>,
}

/// Agrega as linhas do JSONL por `(run, request)`, na ordem em que o turno
/// apareceu no arquivo.
pub fn aggregate<'a, I>(lines: I) -> Report
where
    I: IntoIterator<Item = &'a str>,
{
    const EVENTS: [&str; 3] = ["provider_response", "tool_finished", "turn_finished"];

    let mut report = Report::default();
    let mut seen: HashMap<(String, u64), usize> = HashMap::new();

    for line in lines {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
            report.malformed += 1;
            continue;
        };
        let Some(event) = value["event"].as_str() else {
            report.malformed += 1;
            continue;
        };
        if !EVENTS.contains(&event) {
            continue;
        }
        let Some(request) = value["request"].as_u64() else {
            report.malformed += 1;
            continue;
        };
        // `name` é a tool executada (`ToolFinished.name`,
        // `crates/tupi-app/src/telemetry.rs:537`).
        let name = value["name"].as_str().unwrap_or_default();
        let run = value["run"].as_str().unwrap_or_default().to_string();
        let position = *seen.entry((run.clone(), request)).or_insert_with(|| {
            report.turns.push(TurnRow {
                run,
                request,
                ..TurnRow::default()
            });
            report.turns.len() - 1
        });
        let row = &mut report.turns[position];

        match event {
            "provider_response" => {
                row.provider_requests += 1;
                row.provider_us += value["duration_us"].as_u64().unwrap_or_default();
                row.prompt_tokens += value["prompt_tokens"].as_u64().unwrap_or_default();
                row.cached_tokens += value["cached_tokens"].as_u64().unwrap_or_default();
                row.completion_tokens += value["completion_tokens"].as_u64().unwrap_or_default();
                // Passo a passo: reemissão por truncagem repete o `step`, então o
                // mesmo passo acumula tokens — e a escala em vigor é a última
                // gravada, a que o app usou no envio seguinte.
                let step = value["step"].as_u64().unwrap_or_default();
                let prompt = value["prompt_tokens"].as_u64().unwrap_or_default();
                let cached = value["cached_tokens"].as_u64().unwrap_or_default();
                let scale = value["scale_ppm"].as_u64();
                match row
                    .step_rows
                    .iter_mut()
                    .find(|step_row| step_row.step == step)
                {
                    Some(step_row) => {
                        step_row.prompt_tokens += prompt;
                        step_row.cached_tokens += cached;
                        if scale.is_some() {
                            step_row.scale_ppm = scale;
                        }
                    }
                    None => row.step_rows.push(StepRow {
                        step,
                        scale_ppm: scale,
                        prompt_tokens: prompt,
                        cached_tokens: cached,
                    }),
                }
            }
            "tool_finished" => {
                row.tool_calls += 1;
                row.tool_us += value["duration_us"].as_u64().unwrap_or_default();
                // Só as tools de leitura contam como re-leitura.
                if matches!(name, "read" | "lookup" | "peek") {
                    row.rereads += 1;
                }
            }
            _ => {
                row.steps = value["steps"].as_u64().map(|steps| steps as usize);
                row.duration_us = value["duration_us"].as_u64();
                // Arquivo rotacionado pode ter perdido as respostas do turno; os
                // totais do `turn_finished` são a única fonte que sobrou.
                if row.provider_requests == 0 {
                    row.prompt_tokens = value["total_prompt_tokens"].as_u64().unwrap_or_default();
                    row.cached_tokens = value["total_cached_tokens"].as_u64().unwrap_or_default();
                    row.completion_tokens =
                        value["total_completion_tokens"].as_u64().unwrap_or_default();
                }
            }
        }
        // O histograma por tool é do arquivo, não do turno: acumula mesmo em
        // linha cujo turno ainda não abriu. Linha sem `name` não inventa tool;
        // o `tool_calls` do turno já a contou.
        if event == "tool_finished" && !name.is_empty() {
            let tool = report.tools.entry(name.to_string()).or_default();
            tool.calls += 1;
            if value["ok"].as_bool().unwrap_or(false) {
                tool.ok += 1;
            } else {
                tool.failed += 1;
            }
            tool.result_bytes += value["result_bytes"].as_u64().unwrap_or_default();
            if value["truncated"].as_bool().unwrap_or(false) {
                tool.truncated += 1;
            }
        }
    }

    report
}

/// Uma tool somada ao longo do arquivo, um `tool_finished` por vez.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ToolRow {
    pub calls: u64,
    pub ok: u64,
    pub failed: u64,
    pub result_bytes: u64,
    pub truncated: u64,
}

fn short(run: &str) -> String {
    run.chars().take(8).collect()
}

/// Escala em partes por milhão no formato do relatório: `1.0000` = estimativa
/// exata, abaixo disso o app planeja com menos orçamento do que estimou.
fn scale_label(ppm: u64) -> String {
    format!("{:.4}", ppm as f64 / 1_000_000.0)
}

fn share(part: u64, total: u64) -> String {
    if total == 0 {
        "0%".to_string()
    } else {
        format!("{:.0}%", 100.0 * part as f64 / total as f64)
    }
}

/// Formata o relatório. `limit == 0` mostra todos os turnos; os totais somam o
/// arquivo inteiro e o cabeçalho diz quantos ficaram de fora.
pub fn render(report: &Report, path: &Path, limit: usize) -> String {
    // Quantas tools o histograma imprime: o corte é declarado em "top N de M",
    // nunca silencioso.
    const TOP_TOOLS: usize = 8;

    let turns = &report.turns;
    let shown = if limit == 0 {
        turns.as_slice()
    } else {
        &turns[turns.len().saturating_sub(limit)..]
    };

    let mut out = format!("telemetria: {}\n", path.display());
    out.push_str(&format!(
        "turnos: {} · exibindo {} · linhas ilegíveis: {}\n",
        turns.len(),
        shown.len(),
        report.malformed
    ));
    if turns.is_empty() {
        out.push_str("nenhum turno no arquivo (sem provider_response/turn_finished)\n");
        return out;
    }
    out.push('\n');
    out.push_str(&format!(
        "{:<8} {:>6} {:>6} {:>7} {:>12} {:>10} {:>10} {:>10} {:>10} {:>10}\n",
        "run", "turno", "passos", "re-read", "provider_us", "tool_us", "parede_us", "prompt",
        "cached", "completion"
    ));
    for row in shown {
        out.push_str(&format!(
            "{:<8} {:>6} {:>6} {:>7} {:>12} {:>10} {:>10} {:>10} {:>10} {:>10}\n",
            short(&row.run),
            row.request,
            row.steps
                .map(|steps| steps.to_string())
                .unwrap_or_else(|| "-".to_string()),
            row.rereads,
            row.provider_us,
            row.tool_us,
            row.duration_us
                .map(|us| us.to_string())
                .unwrap_or_else(|| "-".to_string()),
            row.prompt_tokens,
            row.cached_tokens,
            row.completion_tokens
        ));
    }

    let steps: u64 = turns
        .iter()
        .map(|row| row.steps.unwrap_or_default() as u64)
        .sum();
    let provider: u64 = turns.iter().map(|row| row.provider_us).sum();
    let tool: u64 = turns.iter().map(|row| row.tool_us).sum();
    let rereads: u64 = turns.iter().map(|row| row.rereads).sum();
    let wall: u64 = turns
        .iter()
        .map(|row| row.duration_us.unwrap_or_default().max(row.provider_us + row.tool_us))
        .sum();
    let prompt: u64 = turns.iter().map(|row| row.prompt_tokens).sum();
    let cached: u64 = turns.iter().map(|row| row.cached_tokens).sum();
    let completion: u64 = turns.iter().map(|row| row.completion_tokens).sum();

    out.push('\n');
    out.push_str(&format!(
        "{:<8} {:>6} {:>6} {:>7} {:>12} {:>10} {:>10} {:>10} {:>10} {:>10}\n",
        "total", "", steps, rereads, provider, tool, wall, prompt, cached, completion
    ));
    out.push_str(&format!(
        "split do tempo somado: provider {} · tool {} · resto {}\n",
        share(provider, wall),
        share(tool, wall),
        share(wall.saturating_sub(provider + tool), wall)
    ));
    out.push_str(&format!(
        "re-leituras: {} read/lookup/peek no total · {:.2} por turno\n",
        rereads,
        rereads as f64 / turns.len() as f64
    ));

    // Escala e `cached_tokens` por passo: o total do turno não distingue um
    // prefixo cacheado preservado (escala parada) de um request trocado a cada
    // passo (escala oscilando). Antes/depois de qualquer mudança de política se
    // compara aqui, passo a passo, no mesmo turno.
    out.push('\n');
    let per_step: Vec<&TurnRow> = shown
        .iter()
        .filter(|row| !row.step_rows.is_empty())
        .collect();
    if per_step.is_empty() {
        out.push_str("passos: nenhum `provider_response` com `step` nos turnos exibidos\n");
    } else {
        out.push_str(
            "passos dos turnos exibidos (escala em vigor no envio · cache = cached/prompt):\n",
        );
        out.push_str(&format!(
            "{:<8} {:>6} {:>6} {:>8} {:>10} {:>10} {:>7}\n",
            "run", "turno", "passo", "escala", "prompt", "cached", "cache"
        ));
        for row in &per_step {
            for step in &row.step_rows {
                out.push_str(&format!(
                    "{:<8} {:>6} {:>6} {:>8} {:>10} {:>10} {:>7}\n",
                    short(&row.run),
                    row.request,
                    step.step,
                    step.scale_ppm
                        .map(scale_label)
                        .unwrap_or_else(|| "-".to_string()),
                    step.prompt_tokens,
                    step.cached_tokens,
                    share(step.cached_tokens, step.prompt_tokens)
                ));
            }
        }
    }

    // Histograma por tool: chamadas desc e, no empate, o nome asc que o
    // `BTreeMap` já entrega (o sort é estável). `ok`/`falhas` dizem onde a tool
    // recusa; `truncados`, quantas respostas caíram no cap.
    out.push('\n');
    let mut tools: Vec<(&String, &ToolRow)> = report.tools.iter().collect();
    if tools.is_empty() {
        out.push_str("tools: nenhuma chamada com `name` no arquivo\n");
    } else {
        tools.sort_by(|a, b| b.1.calls.cmp(&a.1.calls));
        let calls: u64 = tools.iter().map(|(_, row)| row.calls).sum();
        let truncated: u64 = tools.iter().map(|(_, row)| row.truncated).sum();
        let cut = tools.len().min(TOP_TOOLS);
        out.push_str(&format!(
            "tools por chamadas (top {} de {} · {} chamadas · {} truncadas):\n",
            cut,
            tools.len(),
            calls,
            truncated
        ));
        out.push_str(&format!(
            "{:<20} {:>8} {:>6} {:>7} {:>10} {:>10}\n",
            "tool", "chamadas", "ok", "falhas", "bytes", "truncados"
        ));
        for (name, row) in &tools[..cut] {
            out.push_str(&format!(
                "{:<20} {:>8} {:>6} {:>7} {:>10} {:>10}\n",
                name, row.calls, row.ok, row.failed, row.result_bytes, row.truncated
            ));
        }
    }
    out
}

/// `tupi telemetry …` responde antes de env/redb/provider, como `install`.
pub fn subcommand(argv: &[String]) -> Option<&[String]> {
    match argv.get(1).map(String::as_str) {
        Some("telemetry") => Some(&argv[2..]),
        _ => None,
    }
}

pub fn run(args: &[String], default_path: &Path) -> anyhow::Result<i32> {
    let mut path = std::env::var_os("TUPI_TELEMETRY_PATH")
        .map(PathBuf::from)
        .unwrap_or_else(|| default_path.to_path_buf());
    let mut limit = DEFAULT_LIMIT;
    let mut index = 0;

    while index < args.len() {
        match args[index].as_str() {
            "--help" | "-h" => {
                println!("{USAGE}");
                return Ok(0);
            }
            "--path" => {
                index += 1;
                let value = args
                    .get(index)
                    .ok_or_else(|| anyhow::anyhow!("--path exige um arquivo"))?;
                path = PathBuf::from(value);
            }
            "--limit" => {
                index += 1;
                let value = args
                    .get(index)
                    .ok_or_else(|| anyhow::anyhow!("--limit exige um número"))?;
                limit = value.parse::<usize>().map_err(|error| {
                    anyhow::anyhow!("limite inválido para --limit: {value} ({error})")
                })?;
            }
            other => anyhow::bail!("argumento desconhecido para `tupi telemetry`: {other}\n{USAGE}"),
        }
        index += 1;
    }

    let body = std::fs::read_to_string(&path)
        .map_err(|error| anyhow::anyhow!("falha ao ler {}: {error}", path.display()))?;
    print!("{}", render(&aggregate(body.lines()), &path, limit));
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(event: &str, fields: &str) -> String {
        format!(
            r#"{{"schema":1,"ts_ms":1,"seq":1,"run":"r1","event":"{event}",{fields}}}"#
        )
    }

    fn report(lines: &[String]) -> Report {
        aggregate(lines.iter().map(String::as_str))
    }

    #[test]
    fn the_split_separates_provider_and_tool_time_per_turn() {
        let report = report(&[
            line("turn_started", r#""request":7"#),
            line("provider_response", r#""request":7,"duration_us":900"#),
            line("provider_response", r#""request":7,"duration_us":1100"#),
            line("tool_finished", r#""request":7,"duration_us":300"#),
            line("turn_finished", r#""request":7,"steps":2,"duration_us":2500"#),
        ]);

        assert_eq!(report.turns.len(), 1);
        let row = &report.turns[0];
        assert_eq!(row.request, 7);
        assert_eq!(row.steps, Some(2));
        assert_eq!(row.provider_us, 2000);
        assert_eq!(row.provider_requests, 2);
        assert_eq!(row.tool_us, 300);
        assert_eq!(row.tool_calls, 1);
        assert_eq!(row.duration_us, Some(2500));
    }

    #[test]
    fn the_tokens_sum_every_request_of_the_turn() {
        let report = report(&[
            line(
                "provider_response",
                r#""request":1,"prompt_tokens":400,"cached_tokens":300,"completion_tokens":10"#,
            ),
            line(
                "provider_response",
                r#""request":1,"prompt_tokens":500,"cached_tokens":350,"completion_tokens":20"#,
            ),
            line("turn_finished", r#""request":1,"steps":2"#),
        ]);

        let row = &report.turns[0];
        assert_eq!(row.prompt_tokens, 900);
        assert_eq!(row.cached_tokens, 650);
        assert_eq!(row.completion_tokens, 30);
    }

    #[test]
    fn a_turn_without_provider_responses_falls_back_to_the_finished_totals() {
        let report = report(&[line(
            "turn_finished",
            r#""request":2,"steps":3,"total_prompt_tokens":100,"total_cached_tokens":90,"total_completion_tokens":5"#,
        )]);

        let row = &report.turns[0];
        assert_eq!(row.provider_requests, 0);
        assert_eq!(row.prompt_tokens, 100);
        assert_eq!(row.cached_tokens, 90);
        assert_eq!(row.completion_tokens, 5);
    }

    #[test]
    fn a_turn_without_turn_finished_keeps_its_steps_unknown() {
        let report = report(&[
            line("provider_response", r#""request":3,"duration_us":10"#),
            line("tool_finished", r#""request":3,"duration_us":5"#),
        ]);

        let row = &report.turns[0];
        assert_eq!(row.steps, None);
        assert_eq!(row.duration_us, None);
        assert_eq!(row.provider_us, 10);
        assert_eq!(row.tool_us, 5);
    }

    #[test]
    fn malformed_lines_are_counted_and_never_hidden() {
        let report = report(&[
            "não é json".to_string(),
            r#"{"schema":1,"run":"r1"}"#.to_string(),
            line("turn_finished", r#""request":4,"steps":1"#),
        ]);

        assert_eq!(report.malformed, 2);
        assert_eq!(report.turns.len(), 1);
    }

    #[test]
    fn each_run_keeps_its_own_turn() {
        let report = report(&[
            line("turn_finished", r#""request":1,"steps":1"#),
            r#"{"schema":1,"run":"r2","event":"turn_finished","request":1,"steps":4}"#.to_string(),
        ]);

        assert_eq!(report.turns.len(), 2);
        assert_eq!(report.turns[1].steps, Some(4));
    }

    #[test]
    fn the_limit_keeps_the_last_turns_and_the_totals_sum_the_file() {
        let report = report(&[
            line("turn_finished", r#""request":1,"steps":1,"duration_us":100"#),
            line("turn_finished", r#""request":2,"steps":2,"duration_us":200"#),
            line("turn_finished", r#""request":3,"steps":3,"duration_us":300"#),
        ]);

        let out = render(&report, Path::new("/repo/telemetry.jsonl"), 1);
        assert!(out.contains("turnos: 3 · exibindo 1"), "{out}");
        assert!(out.contains("/repo/telemetry.jsonl"), "{out}");
        assert!(!out.contains("      1      1"), "{out}");
        assert!(out.contains("total"), "{out}");
        // Os totais somam os três turnos, não só o exibido.
        assert!(out.contains("      6"), "{out}");
    }

    #[test]
    fn the_report_names_the_columns_of_the_split() {
        let report = report(&[
            line("provider_response", r#""request":9,"duration_us":1000"#),
            line("turn_finished", r#""request":9,"steps":1,"duration_us":1200"#),
        ]);

        let out = render(&report, Path::new("telemetry.jsonl"), 0);
        for column in ["passos", "provider_us", "tool_us", "prompt", "cached", "completion"] {
            assert!(out.contains(column), "coluna {column} ausente: {out}");
        }
        assert!(out.contains("split do tempo somado: provider 83%"), "{out}");
    }

    #[test]
    fn each_tool_accumulates_calls_failures_bytes_and_truncation_by_name() {
        let report = report(&[
            line(
                "tool_finished",
                r#""request":1,"name":"read","ok":true,"result_bytes":1200,"truncated":true"#,
            ),
            line(
                "tool_finished",
                r#""request":1,"name":"read","ok":false,"result_bytes":0,"truncated":false"#,
            ),
            line(
                "tool_finished",
                r#""request":1,"name":"search","ok":true,"result_bytes":800,"truncated":false"#,
            ),
            line(
                "tool_finished",
                r#""request":1,"name":"shell","ok":true,"result_bytes":90,"truncated":true"#,
            ),
            "não é json".to_string(),
        ]);

        assert_eq!(report.malformed, 1);
        assert_eq!(report.tools.len(), 3);

        let read = &report.tools["read"];
        assert_eq!((read.calls, read.ok, read.failed), (2, 1, 1));
        assert_eq!((read.result_bytes, read.truncated), (1_200, 1));

        let search = &report.tools["search"];
        assert_eq!((search.calls, search.failed), (1, 0));
        assert_eq!((search.result_bytes, search.truncated), (800, 0));

        let shell = &report.tools["shell"];
        assert_eq!((shell.calls, shell.ok), (1, 1));
        assert_eq!((shell.result_bytes, shell.truncated), (90, 1));
    }

    #[test]
    fn the_tool_histogram_prints_the_top_n_with_the_truncated_column() {
        let mut lines: Vec<String> = Vec::new();
        // Dez tools empatadas em uma chamada: o desempate é o nome asc.
        for index in 0..10 {
            let name = format!("t{index}");
            lines.push(line(
                "tool_finished",
                &format!(r#""request":1,"name":"{name}","ok":true,"result_bytes":10"#),
            ));
        }
        // `read` é a mais chamada e a única truncada.
        for ok in [true, false] {
            lines.push(line(
                "tool_finished",
                &format!(
                    r#""request":1,"name":"read","ok":{ok},"result_bytes":1000,"truncated":true"#
                ),
            ));
        }
        lines.push(line("turn_finished", r#""request":1,"steps":1"#));

        let out = render(&report(&lines), Path::new("telemetry.jsonl"), 0);

        for column in ["tool", "chamadas", "ok", "falhas", "bytes", "truncados"] {
            assert!(out.contains(column), "coluna {column} ausente: {out}");
        }
        // O corte não é silencioso: o topo mostrado e o total de tools aparecem.
        assert!(out.contains("tools por chamadas (top 8 de 11"), "{out}");
        assert!(out.contains("12 chamadas · 2 truncadas"), "{out}");

        // Ordem fixa: chamadas desc, nome asc no empate; o excedente fica fora.
        let position = |name: &str| {
            out.find(&format!("\n{name} "))
                .unwrap_or_else(|| panic!("{name} ausente: {out}"))
        };
        assert!(position("read") < position("t0"), "{out}");
        assert!(position("t0") < position("t6"), "{out}");
        assert!(!out.contains("\nt7 "), "a nona tool não deveria caber: {out}");

        // A coluna de truncados carrega a contagem real de cada tool.
        let read_line = out
            .lines()
            .find(|row| row.starts_with("read "))
            .expect("linha do read");
        assert!(read_line.ends_with('2'), "{read_line}");
        let t0_line = out
            .lines()
            .find(|row| row.starts_with("t0 "))
            .expect("linha da t0");
        assert!(t0_line.ends_with('0'), "{t0_line}");
    }

    #[test]
    fn each_step_prints_its_scale_and_cached_tokens() {
        // Turno de referência (T1.2) reduzido a três linhas: o que o relatório
        // passa a mostrar é a escala em vigor e o cache de cada passo, não o
        // total do turno.
        let report = report(&[
            line(
                "provider_response",
                r#""request":1,"step":1,"prompt_tokens":6328,"cached_tokens":4480,"scale_ppm":1000000"#,
            ),
            line(
                "provider_response",
                r#""request":1,"step":2,"prompt_tokens":10239,"cached_tokens":6400,"scale_ppm":870000"#,
            ),
            // Reemissão por truncagem repete o passo: os tokens somam e a escala
            // em vigor é a última gravada.
            line(
                "provider_response",
                r#""request":1,"step":2,"prompt_tokens":1000,"cached_tokens":600,"scale_ppm":830000"#,
            ),
            line("turn_finished", r#""request":1,"steps":2"#),
        ]);

        let out = render(&report, Path::new("telemetry.jsonl"), 0);
        for column in ["passo", "escala", "cached", "cache"] {
            assert!(out.contains(column), "coluna {column} ausente: {out}");
        }

        let row = |scale: &str| {
            out.lines()
                .find(|row| row.contains(scale))
                .unwrap_or_else(|| panic!("passo com escala {scale} ausente: {out}"))
                .to_string()
        };
        let first = row("1.0000");
        assert!(first.contains("6328"), "{first}");
        assert!(first.contains("4480"), "{first}");
        assert!(first.ends_with("71%"), "{first}");

        let second = row("0.8300");
        assert!(second.contains("11239"), "cache/prompt somam o passo: {second}");
        assert!(second.contains("7000"), "cache/prompt somam o passo: {second}");
        assert!(second.ends_with("62%"), "{second}");
    }

    #[test]
    fn the_subcommand_is_recognised_only_in_the_first_position() {
        let argv = ["tupi".to_string(), "telemetry".to_string(), "--limit".to_string()];

        assert_eq!(subcommand(&argv), Some(&argv[2..]));
        assert!(subcommand(&["tupi".to_string(), "--web".to_string()]).is_none());
    }
}
