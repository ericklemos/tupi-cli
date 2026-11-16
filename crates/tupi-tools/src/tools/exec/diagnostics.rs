use super::*;

pub(crate) struct Diagnostics {
    pub(crate) errors: Vec<String>,
    pub(crate) warnings: Vec<String>,
}

pub(super) fn diagnostics(stderr: &str) -> Diagnostics {
    let lines: Vec<&str> = stderr.lines().collect();
    let mut errors = Vec::new();
    let mut warnings = Vec::new();
    let mut index = 0;

    while index < lines.len() {
        let line = lines[index];
        let is_error = line.starts_with("error");
        let is_warning = line.starts_with("warning");
        if !is_error && !is_warning {
            index += 1;
            continue;
        }

        let mut block = String::new();
        while index < lines.len() {
            let current = lines[index];
            if !block.is_empty() && (current.starts_with("error") || current.starts_with("warning"))
            {
                break;
            }
            block.push_str(current);
            block.push('\n');
            index += 1;
            if block.len() > 4_000 || current.trim().is_empty() {
                break;
            }
        }

        if is_error {
            errors.push(block.trim_end().to_string());
        } else {
            warnings.push(block.trim_end().to_string());
        }
    }

    Diagnostics { errors, warnings }
}

pub(super) fn test_lines(stdout: &str) -> Vec<String> {
    let mut lines = Vec::new();
    for line in stdout.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("test result:")
            || trimmed == "failures:"
            || (trimmed.starts_with("---- ") && trimmed.ends_with(" stdout ----"))
        {
            lines.push(trimmed.to_string());
            if lines.len() >= 12 {
                break;
            }
        }
    }
    lines
}

pub(crate) fn sections_for(
    args: &[String],
    raw: &Output,
    cache: CacheNote,
) -> Vec<Section> {
    let command = command_line(args);
    let status = if raw.timed_out {
        "timeout"
    } else if raw.success {
        "ok"
    } else {
        "falhou"
    };
    let diag = diagnostics(&raw.stderr);
    let tests = test_lines(&raw.stdout);

    // Um hit de cache não pode se passar por execução fresca: `raw.duration`
    // é a duração da execução *original*, então a idade do hit precisa
    // aparecer junto dela.
    let timing = match cache.age {
        Some(age) => {
            let seconds = age.as_secs();
            let ago = if seconds >= 60 {
                format!("{}m{}s", seconds / 60, seconds % 60)
            } else {
                format!("{seconds}s")
            };
            format!(
                "em cache há {ago} · execução original: {:.1}s",
                raw.duration.as_secs_f64()
            )
        }
        None => format!("{:.1}s", raw.duration.as_secs_f64()),
    };

    // Uma linha só: veredito do cache e custo da chave (walk + blake3, feito
    // antes do cargo). No miss, o total da chamada. Tokens são requisito de
    // projeto: no máximo uma linha a mais por chamada.
    let cache_line = match (&cache.off, cache.age, cache.key_cost) {
        (Some(off), _, _) => format!("cache desligado: {}", off.reason()),
        (None, Some(_), Some(cost)) => {
            format!("cache: hit · chave {:.1}ms", cost.as_secs_f64() * 1000.0)
        }
        (None, Some(_), None) => "cache: hit".to_string(),
        (None, None, Some(cost)) => format!(
            "cache: miss · chave {:.1}ms · total {:.1}s",
            cost.as_secs_f64() * 1000.0,
            (cost + raw.duration).as_secs_f64()
        ),
        (None, None, None) => "cache: desligado".to_string(),
    };

    let mut summary = String::new();
    summary.push('\n');
    summary.push_str(&format!("- comando: {command}\n"));
    summary.push_str(&format!("- status: {status} · {timing}\n"));
    summary.push_str(&format!("- {cache_line}\n"));
    if !diag.errors.is_empty() || !diag.warnings.is_empty() {
        summary.push_str(&format!(
            "- erros: {} · avisos: {}\n",
            diag.errors.len(),
            diag.warnings.len()
        ));
    }
    if !tests.is_empty() {
        summary.push_str("- testes:\n");
        for line in &tests {
            summary.push_str(&format!("  - {line}\n"));
        }
    }

    for block in diag.errors.iter().take(MAX_ERRORS_SHOWN) {
        summary.push_str("\n\x7e\x7e\x7etext\n");
        summary.push_str(&clip(block, 1_600, 200));
        summary.push_str("\n\x7e\x7e\x7e\n");
    }
    for block in diag.warnings.iter().take(MAX_WARNINGS_SHOWN) {
        summary.push_str("\n\x7e\x7e\x7etext\n");
        summary.push_str(&clip(block, 1_000, 0));
        summary.push_str("\n\x7e\x7e\x7e\n");
    }

    let mut sections = vec![Section {
        heading: format!("### {command} — {status}"),
        body: summary,
        priority: 255,
    }];

    // Os erros do compilador são o payload: um teto no resumo não pode
    // esconder o oitavo erro atrás de uma seção que o orçamento descarta.
    let mut remaining_errors = String::new();
    for block in diag.errors.iter().skip(MAX_ERRORS_SHOWN) {
        remaining_errors.push_str("\n\x7e\x7e\x7etext\n");
        remaining_errors.push_str(&clip(block, 1_600, 200));
        remaining_errors.push_str("\n\x7e\x7e\x7e\n");
    }
    if !remaining_errors.is_empty() {
        sections.push(Section {
            heading: "#### erros restantes".to_string(),
            body: remaining_errors,
            priority: 200,
        });
    }

    let combined = format!(
        "{}\n{}",
        clip(raw.stderr.trim_end(), 8_000, 2_000),
        clip(raw.stdout.trim_end(), 8_000, 8_000)
    );
    let needs_detail = diag.warnings.len() > MAX_WARNINGS_SHOWN
        || raw.stdout.contains("failures:")
        || (!raw.success && diag.errors.is_empty() && !combined.trim().is_empty());

    if needs_detail {
        sections.push(Section {
            heading: "#### saida completa".to_string(),
            body: format!("\n\x7e\x7e\x7etext\n{}\n\x7e\x7e\x7e\n", combined.trim()),
            priority: 80,
        });
    }

    sections
}
