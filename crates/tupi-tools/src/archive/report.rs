//! Dossiê em disco: markdown para ler, JSON para verificar.

use std::path::{Path, PathBuf};

use super::error::ArchiveError;
use super::normalize::fold;
use super::reduce::Dossier;

/// Grava `<slug>.md` e `<slug>.json` em `dir`.
pub fn write(dir: &Path, dossier: &Dossier) -> Result<PathBuf, ArchiveError> {
    std::fs::create_dir_all(dir)?;
    let slug = slug_of(&dossier.question);
    let md_path = dir.join(format!("{slug}.md"));
    std::fs::write(&md_path, to_markdown(dossier))?;
    let json = serde_json::to_string_pretty(&to_json(dossier))
        .map_err(|error| ArchiveError::Db(error.to_string()))?;
    std::fs::write(dir.join(format!("{slug}.json")), json)?;
    Ok(md_path)
}

fn to_json(dossier: &Dossier) -> serde_json::Value {
    let rows = |rows: &[super::reduce::MatchRow]| {
        rows.iter()
            .map(|row| {
                serde_json::json!({
                    "segment_id": row.segment_id,
                    "source": row.source,
                    "locator": row.locator,
                    "entity": row.entity,
                    "reason": row.reason,
                })
            })
            .collect::<Vec<_>>()
    };
    serde_json::json!({
        "question": dossier.question,
        "matches": rows(&dossier.matches),
        "unsure": rows(&dossier.unsure),
        "coverage": {
            "sources_total": dossier.coverage.sources_total,
            "sources_indexed": dossier.coverage.sources_indexed,
            "gaps": dossier.coverage.gaps.iter().map(|gap| serde_json::json!({
                "source": gap.source,
                "reason": gap.reason,
            })).collect::<Vec<_>>(),
        },
    })
}

fn slug_of(question: &str) -> String {
    let folded = fold(question);
    let mut slug = String::new();
    for ch in folded.chars() {
        if ch.is_ascii_alphanumeric() {
            slug.push(ch);
        } else if !slug.ends_with('-') {
            slug.push('-');
        }
    }
    let slug = slug.trim_matches('-').to_string();
    if slug.is_empty() {
        "investigacao".to_string()
    } else {
        slug
    }
}

fn to_markdown(dossier: &Dossier) -> String {
    let mut out = format!("# {}\n\n", dossier.question);
    out.push_str(&format!(
        "Cobertura: {} de {} fontes indexadas.\n\n",
        dossier.coverage.sources_indexed, dossier.coverage.sources_total
    ));
    out.push_str(&format!("## Ocorrências ({})\n\n", dossier.matches.len()));
    for row in &dossier.matches {
        let entity = row
            .entity
            .as_ref()
            .map(|entity| format!(" ({entity})"))
            .unwrap_or_default();
        out.push_str(&format!(
            "- {}:{} — {} — {}\n",
            row.source, row.locator, entity, row.reason
        ));
    }
    if !dossier.unsure.is_empty() {
        out.push_str(&format!("\n## Dúvidas ({})\n\n", dossier.unsure.len()));
        for row in &dossier.unsure {
            out.push_str(&format!("- {} — {}\n", row.locator, row.reason));
        }
    }
    out.push_str("\n## Lacunas\n\n");
    if dossier.coverage.gaps.is_empty() {
        out.push_str("Nenhuma: todo o acervo declarado foi indexado.\n");
    } else {
        for gap in &dossier.coverage.gaps {
            out.push_str(&format!("- {}: {}\n", gap.source, gap.reason));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::archive::reduce::{Coverage, Gap, MatchRow};

    #[test]
    fn the_dossier_declares_gaps_and_cites_every_match() {
        let dossier = Dossier {
            question: "quem falou do joao?".into(),
            matches: vec![MatchRow {
                segment_id: 1,
                source: "dia1.txt".into(),
                locator: "dia1.txt:3".into(),
                entity: Some("Joao".into()),
                reason: "nome".into(),
            }],
            unsure: vec![],
            coverage: Coverage {
                sources_total: 2,
                sources_indexed: 1,
                gaps: vec![Gap {
                    source: "audio.mp3".into(),
                    reason: "nao transcrito".into(),
                }],
            },
        };
        let dir = tempfile::tempdir().unwrap();
        let path = write(dir.path(), &dossier).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("nao transcrito"));
        assert!(text.contains("dia1.txt:3"));
    }
}
