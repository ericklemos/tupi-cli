//! Redução: julgamentos viram dossiê com cobertura e lacunas.

use super::judge::{Judgment, Verdict};
use super::plan::InvestigationPlan;
use super::store::{SegmentRecord, SourceRecord, SourceStatus};

/// Lacuna declarada do acervo.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Gap {
    pub source: String,
    pub reason: String,
}

/// O que o índice cobriu.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Coverage {
    pub sources_total: usize,
    pub sources_indexed: usize,
    pub gaps: Vec<Gap>,
}

/// Ocorrência citável do dossiê.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MatchRow {
    pub segment_id: u64,
    pub source: String,
    pub locator: String,
    pub entity: Option<String>,
    pub reason: String,
}

/// Resultado final da investigação.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Dossier {
    pub question: String,
    pub matches: Vec<MatchRow>,
    pub unsure: Vec<MatchRow>,
    pub coverage: Coverage,
}

/// Monta cobertura a partir das fontes do acervo.
pub fn coverage_of(sources: &[SourceRecord]) -> Coverage {
    let mut coverage = Coverage {
        sources_total: sources.len(),
        sources_indexed: 0,
        gaps: Vec::new(),
    };
    for source in sources {
        match source.status {
            SourceStatus::Indexed => coverage.sources_indexed += 1,
            SourceStatus::Gap | SourceStatus::Quarantined => coverage.gaps.push(Gap {
                source: source.path.clone(),
                reason: source
                    .reason
                    .clone()
                    .unwrap_or_else(|| format!("{:?}", source.status).to_lowercase()),
            }),
            SourceStatus::Pending => coverage.gaps.push(Gap {
                source: source.path.clone(),
                reason: "ainda não indexado".to_string(),
            }),
        }
    }
    coverage
}

/// Deduplica, ordena e agrupa os julgamentos.
pub fn reduce(
    plan: &InvestigationPlan,
    judgments: &[Judgment],
    segments: &[SegmentRecord],
    coverage: Coverage,
) -> Dossier {
    let mut matches = Vec::new();
    let mut unsure = Vec::new();
    let mut seen = Vec::new();
    for judgment in judgments {
        if seen.contains(&judgment.segment_id) {
            continue;
        }
        seen.push(judgment.segment_id);
        let Some(segment) = segments.iter().find(|segment| segment.id == judgment.segment_id) else {
            continue;
        };
        let row = MatchRow {
            segment_id: segment.id,
            source: segment.source.clone(),
            locator: format!("{}:{}", segment.source, segment.line_start),
            entity: judgment.entity.clone(),
            reason: judgment.reason.clone(),
        };
        match judgment.verdict {
            Verdict::Match => matches.push(row),
            Verdict::Unsure => unsure.push(row),
            Verdict::NoMatch => {}
        }
    }
    Dossier {
        question: plan.question.clone(),
        matches,
        unsure,
        coverage,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plan() -> InvestigationPlan {
        InvestigationPlan {
            question: "quem falou do joao?".into(),
            entities: vec![],
            exclude: vec![],
            from: None,
            to: None,
            language: "pt-BR".into(),
        }
    }

    fn segment(id: u64) -> SegmentRecord {
        SegmentRecord {
            id,
            source: "dia1.txt".into(),
            shard: "2025-11-15".into(),
            line_start: 3,
            line_end: 3,
            ts: None,
            certainty: "none".into(),
            kind: "text".into(),
            text: "joao".into(),
            body: "joao".into(),
        }
    }

    #[test]
    fn the_dossier_declares_gaps_and_cites_every_match() {
        let judgments = vec![Judgment {
            segment_id: 1,
            verdict: Verdict::Match,
            entity: Some("Joao".into()),
            reason: "nome".into(),
        }];
        let coverage = Coverage {
            sources_total: 2,
            sources_indexed: 1,
            gaps: vec![Gap {
                source: "audio.mp3".into(),
                reason: "nao transcrito".into(),
            }],
        };
        let dossier = reduce(&plan(), &judgments, &[segment(1)], coverage);
        assert_eq!(dossier.matches.len(), 1);
        assert_eq!(dossier.matches[0].locator, "dia1.txt:3");
        assert_eq!(dossier.coverage.gaps[0].reason, "nao transcrito");
    }
}
