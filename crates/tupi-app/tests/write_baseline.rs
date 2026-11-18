//! Sonda da task 0 do plano de escrita agnóstica: lê o JSONL de telemetria **já
//! gravado** e quebra as chamadas de escrita por extensão de arquivo.
//!
//! Nada de instrumentação nova: junta os eventos que já existem —
//! `ToolStarted` (nome + `arguments`) e `ToolFinished` (`ok` + `error`) — pela
//! chave `(run, call)`. Para `write`/`write_many` compara o conteúdo pedido com
//! a versão anterior do mesmo path **dentro do mesmo run**, o que dá quantas
//! linhas o novo conteúdo de fato mudou.
//!
//! Rodar:
//!
//! ```text
//! cargo test -p tupi-app --test write_baseline -- --nocapture
//! TUPI_TELEMETRY_PATH=/caminho/telemetry.jsonl \
//!   cargo test -p tupi-app --test write_baseline -- --nocapture
//! ```
//!
//! O relatório também é gravado em `target/write-baseline.txt`.

use std::collections::BTreeMap;
use std::path::PathBuf;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|path| path.parent())
        .map(|path| path.to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."))
}

fn extension(path: &str) -> String {
    let name = path.rsplit(['/', '\\']).next().unwrap_or(path);
    match name.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() && !ext.is_empty() => format!(".{ext}"),
        _ => "(sem extensao)".to_string(),
    }
}

/// Linhas que aparecem num lado e não no outro, por LCS. É o "quantas linhas o
/// diff mudou"; arquivo grande demais para a matriz cai para o pior caso
/// (`max`) e é marcado como aproximado.
fn changed_lines(before: &str, after: &str) -> (usize, bool) {
    const CAP: usize = 3000;
    let a: Vec<&str> = before.lines().collect();
    let b: Vec<&str> = after.lines().collect();
    if a.len() > CAP || b.len() > CAP {
        return (a.len().max(b.len()), true);
    }
    let mut previous = vec![0usize; b.len() + 1];
    let mut current = vec![0usize; b.len() + 1];
    for left in &a {
        for (index, right) in b.iter().enumerate() {
            current[index + 1] = if left == right {
                previous[index] + 1
            } else {
                previous[index + 1].max(current[index])
            };
        }
        std::mem::swap(&mut previous, &mut current);
    }
    let common = previous[b.len()];
    ((a.len() - common) + (b.len() - common), false)
}

#[derive(Default, Clone)]
struct Started {
    name: String,
    /// path · conteúdo pedido (write/write_many) ou `new_body` (edit).
    files: Vec<(String, Option<String>)>,
    truncated: bool,
}

#[derive(Default, Clone)]
struct ExtensionRow {
    write: u64,
    write_many: u64,
    edit: u64,
    patch: u64,
    relocate: u64,
    ok: u64,
    failed: u64,
    /// `write` cirúrgico: diff < 10% do arquivo escrito.
    surgical: u64,
    /// `write` comparado com a versão anterior do mesmo path no mesmo run.
    compared: u64,
    /// Σ linhas mudadas e Σ linhas do arquivo, para a razão.
    changed: u64,
    lines: u64,
    /// Recusas de `write`: StaleHash vs. resto.
    write_stale: u64,
    /// Recusas de `edit` por classe.
    edit_stale: u64,
    edit_language: u64,
    edit_symbol: u64,
    edit_ambiguous: u64,
    edit_other: u64,
    calls: u64,
}

fn refusal_class(error: &str) -> &'static str {
    let lowered = error.to_lowercase();
    if error.contains("hash atual") || error.contains("blake3") {
        "stale"
    } else if lowered.contains("rust")
        || lowered.contains("linguagem")
        || lowered.contains("extens")
        || lowered.contains("suporta")
        || lowered.contains("parser")
    {
        "language"
    } else if lowered.contains("aparece") && lowered.contains("vezes") {
        "ambiguous"
    } else if lowered.contains("nao") || lowered.contains("não") {
        "symbol"
    } else {
        "other"
    }
}

#[test]
fn write_baseline() {
    let root = repo_root();
    let path = std::env::var_os("TUPI_TELEMETRY_PATH")
        .map(PathBuf::from)
        .unwrap_or_else(|| root.join(".agents/tupi/debug/telemetry.jsonl"));
    let Ok(body) = std::fs::read_to_string(&path) else {
        eprintln!("sem {path:?}: sonda ignorada");
        return;
    };

    let mut started: BTreeMap<(String, String), Started> = BTreeMap::new();
    let mut rows: BTreeMap<String, ExtensionRow> = BTreeMap::new();
    // Última versão conhecida de cada path por run, para o diff do `write`.
    let mut versions: BTreeMap<(String, String), String> = BTreeMap::new();
    let mut errors: BTreeMap<String, u64> = BTreeMap::new();
    let mut malformed = 0usize;
    let mut calls = 0u64;
    let mut tools: BTreeMap<String, u64> = BTreeMap::new();

    for line in body.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
            malformed += 1;
            continue;
        };
        let event = value["event"].as_str().unwrap_or_default();
        let run = value["run"].as_str().unwrap_or_default().to_string();
        let call = value["call"].as_str().unwrap_or_default().to_string();
        match event {
            "tool_started" => {
                let name = value["name"].as_str().unwrap_or_default().to_string();
                let arguments: serde_json::Value = value["arguments"]
                    .as_str()
                    .and_then(|raw| serde_json::from_str(raw).ok())
                    .unwrap_or(serde_json::Value::Null);
                let mut files = Vec::new();
                if let Some(path) = arguments["path"].as_str() {
                    let content = arguments["content"]
                        .as_str()
                        .or_else(|| arguments["new_body"].as_str())
                        .map(|text| text.to_string());
                    files.push((path.to_string(), content));
                }
                if let Some(writes) = arguments["writes"].as_array() {
                    for item in writes {
                        if let Some(path) = item["path"].as_str() {
                            files.push((
                                path.to_string(),
                                item["content"].as_str().map(|text| text.to_string()),
                            ));
                        }
                    }
                }
                started.insert(
                    (run, call),
                    Started {
                        name,
                        files,
                        truncated: value["truncated"].as_bool().unwrap_or(false),
                    },
                );
            }
            "tool_finished" => {
                let name = value["name"].as_str().unwrap_or_default().to_string();
                *tools.entry(name.clone()).or_default() += 1;
                let ok = value["ok"].as_bool().unwrap_or(false);
                let error = value["error"].as_str().unwrap_or_default().to_string();
                let key = (run.clone(), call.clone());
                let Some(start) = started.get(&key).cloned() else {
                    calls += 1;
                    continue;
                };
                calls += 1;
                let mut per_extension = Vec::new();
                for (path, content) in &start.files {
                    per_extension.push((extension(path), path.clone(), content.clone()));
                }
                if per_extension.is_empty() {
                    per_extension.push(("(sem path)".to_string(), String::new(), None));
                }
                for (ext, file_path, content) in per_extension {
                    let row = rows.entry(ext).or_default();
                    row.calls += 1;
                    match name.as_str() {
                        "write" => row.write += 1,
                        "write_many" => row.write_many += 1,
                        "edit" => row.edit += 1,
                        "patch" => row.patch += 1,
                        "relocate" => row.relocate += 1,
                        _ => {}
                    }
                    if ok {
                        row.ok += 1;
                    } else {
                        row.failed += 1;
                        *errors
                            .entry(format!(
                                "{name}: {}",
                                error.chars().take(110).collect::<String>()
                            ))
                            .or_default() += 1;
                        let class = refusal_class(&error);
                        match name.as_str() {
                            "write" if class == "stale" => row.write_stale += 1,
                            "edit" => match class {
                                "stale" => row.edit_stale += 1,
                                "language" => row.edit_language += 1,
                                "ambiguous" => row.edit_ambiguous += 1,
                                "symbol" => row.edit_symbol += 1,
                                _ => row.edit_other += 1,
                            },
                            _ => {}
                        }
                    }
                    if !ok || !matches!(name.as_str(), "write" | "write_many") {
                        continue;
                    }
                    let (Some(content), false) = (&content, start.truncated) else {
                        continue;
                    };
                    let version_key = (run.clone(), file_path.clone());
                    if let Some(before) = versions.get(&version_key) {
                        let (changed, approximated) = changed_lines(before, content);
                        let total = content.lines().count() as u64;
                        row.compared += 1;
                        row.changed += changed as u64;
                        row.lines += total;
                        if !approximated && changed as f64 * 10.0 < total as f64 {
                            row.surgical += 1;
                        }
                    }
                    versions.insert(version_key, content.clone());
                }
            }
            _ => {}
        }
    }

    let mut report = String::new();
    report.push_str("# escrita por extensão (telemetria real)\n\n");
    report.push_str(&format!("arquivo: {}\n", path.display()));
    report.push_str(&format!(
        "linhas ilegíveis: {malformed} · chamadas de tool: {calls}\n\n"
    ));
    report.push_str(&format!(
        "tools de escrita: {}\n\n",
        tools
            .iter()
            .filter(|(name, _)| matches!(
                name.as_str(),
                "write" | "write_many" | "edit" | "patch" | "relocate"
            ))
            .map(|(name, calls)| format!("{name} {calls}"))
            .collect::<Vec<_>>()
            .join(" · ")
    ));

    let mut sorted: Vec<(&String, &ExtensionRow)> = rows.iter().collect();
    sorted.sort_by(|a, b| b.1.calls.cmp(&a.1.calls).then(a.0.cmp(b.0)));
    report.push_str(
        "| extensão | chamadas | write | write_many | edit | patch | ok | falhas | write stale | edit hash | edit língua | edit símbolo | edit ambíguo | write cirúrgico |\n",
    );
    report.push_str(
        "| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |\n",
    );
    for (ext, row) in &sorted {
        report.push_str(&format!(
            "| {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} |\n",
            ext,
            row.calls,
            row.write,
            row.write_many,
            row.edit,
            row.patch,
            row.ok,
            row.failed,
            row.write_stale,
            row.edit_stale,
            row.edit_language,
            row.edit_symbol,
            row.edit_ambiguous,
            row.surgical,
        ));
    }
    report.push_str("\n## diff do write contra a versão anterior\n\n");
    report.push_str("| extensão | comparados | linhas mudadas | linhas do arquivo | razão | cirúrgicos (<10%) |\n");
    report.push_str("| --- | --- | --- | --- | --- | --- |\n");
    for (ext, row) in &sorted {
        if row.compared == 0 {
            continue;
        }
        report.push_str(&format!(
            "| {} | {} | {} | {} | {:.1}% | {} |\n",
            ext,
            row.compared,
            row.changed,
            row.lines,
            row.changed as f64 * 100.0 / row.lines.max(1) as f64,
            row.surgical,
        ));
    }
    report.push_str("\n## recusas (texto bruto, cortado em 110 chars)\n\n");
    let mut errors_sorted: Vec<(&String, &u64)> = errors.iter().collect();
    errors_sorted.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));
    for (error, count) in errors_sorted.iter().take(30) {
        report.push_str(&format!("- {count}× `{error}`\n"));
    }

    eprintln!("{report}");
    let _ = std::fs::write(root.join("target/write-baseline.txt"), &report);
}
