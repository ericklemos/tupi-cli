//! Adjudicação: o LLM decide se o candidato é a mesma pessoa/assunto.

use serde::{Deserialize, Serialize};
use tupi_kernel::generate::Generator;

use super::error::ArchiveError;
use super::plan::{parse_json, InvestigationPlan};
use super::scan::Candidate;
use super::store::ArchiveDb;

/// Veredito de um candidato.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    Match,
    NoMatch,
    Unsure,
}

impl Verdict {
    fn as_str(self) -> &'static str {
        match self {
            Verdict::Match => "match",
            Verdict::NoMatch => "no_match",
            Verdict::Unsure => "duvida",
        }
    }

    fn parse(raw: &str) -> Verdict {
        match raw {
            "match" => Verdict::Match,
            "duvida" | "unsure" => Verdict::Unsure,
            _ => Verdict::NoMatch,
        }
    }
}

/// Julgamento persistido.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Judgment {
    pub segment_id: u64,
    pub verdict: Verdict,
    pub entity: Option<String>,
    pub reason: String,
}

#[derive(Serialize, Deserialize)]
struct StoredJudgment {
    segment_id: u64,
    verdict: String,
    entity: Option<String>,
    reason: String,
}

#[derive(Deserialize)]
struct RawJudgment {
    segment_id: u64,
    verdict: String,
    #[serde(default)]
    entity: Option<String>,
    #[serde(default)]
    reason: String,
}

/// Julga os candidatos em lotes, pulando o que já está no cache.
#[allow(clippy::too_many_arguments)]
pub fn judge_batch(
    db: &ArchiveDb,
    plan_hash: &str,
    model: &str,
    plan: &InvestigationPlan,
    candidates: &[Candidate],
    context_of: impl Fn(u64) -> String,
    generator: &dyn Generator,
    batch: usize,
) -> Result<Vec<Judgment>, ArchiveError> {
    let mut out = Vec::new();
    let mut pending: Vec<&Candidate> = Vec::new();

    for candidate in candidates {
        match db.verdict(plan_hash, model, candidate.segment_id)? {
            Some(raw) => {
                let stored: StoredJudgment = serde_json::from_slice(&raw)
                    .map_err(|error| ArchiveError::Db(error.to_string()))?;
                out.push(Judgment {
                    segment_id: stored.segment_id,
                    verdict: Verdict::parse(&stored.verdict),
                    entity: stored.entity,
                    reason: stored.reason,
                });
            }
            None => pending.push(candidate),
        }
    }

    let batch = batch.max(1);
    for chunk in pending.chunks(batch) {
        let mut listing = String::new();
        for candidate in chunk {
            listing.push_str(&format!(
                "id {}: {}\n",
                candidate.segment_id,
                context_of(candidate.segment_id)
            ));
        }
        let labels = plan
            .entities
            .iter()
            .map(|entity| entity.label.clone())
            .collect::<Vec<_>>()
            .join(", ");
        let prompt = format!(
            "Entidade/tema: {labels}. Responda SOMENTE um JSON array de objetos com segment_id, verdict (match, no_match ou duvida), entity e reason. Trechos:\n{listing}"
        );
        let raw = generator
            .generate(&prompt, 1024)
            .map_err(|error| ArchiveError::Contract(error.to_string()))?;
        let parsed: Vec<RawJudgment> = parse_json(&raw)?;
        for item in parsed {
            let verdict = Verdict::parse(&item.verdict);
            let stored = StoredJudgment {
                segment_id: item.segment_id,
                verdict: verdict.as_str().to_string(),
                entity: item.entity.clone(),
                reason: item.reason.clone(),
            };
            let payload = serde_json::to_vec(&stored)
                .map_err(|error| ArchiveError::Db(error.to_string()))?;
            db.put_verdict(plan_hash, model, item.segment_id, &payload)?;
            out.push(Judgment {
                segment_id: item.segment_id,
                verdict,
                entity: item.entity,
                reason: item.reason,
            });
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;
    use crate::archive::plan::EntityPlan;
    use tupi_kernel::generate::GenerateError;

    struct Fake {
        text: String,
        calls: AtomicUsize,
    }

    impl Generator for Fake {
        fn model_id(&self) -> &str {
            "fake"
        }
        fn generate(&self, _: &str, _: usize) -> Result<String, GenerateError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(self.text.clone())
        }
    }

    fn plan() -> InvestigationPlan {
        InvestigationPlan {
            question: "quem falou do joao?".into(),
            entities: vec![EntityPlan {
                label: "Joao".into(),
                forms: vec!["joao".into()],
            }],
            exclude: vec![],
            from: None,
            to: None,
            language: "pt-BR".into(),
        }
    }

    #[test]
    fn a_judged_candidate_is_not_sent_to_the_model_again() {
        let dir = tempfile::tempdir().unwrap();
        let db = ArchiveDb::open(&dir.path().join("a.db")).unwrap();
        let id = db.next_segment_id().unwrap();
        db.put_segments(
            "d1.txt",
            &[crate::archive::store::SegmentRecord {
                id,
                source: "d1.txt".into(),
                shard: "2025-11-15".into(),
                line_start: 1,
                line_end: 1,
                ts: None,
                certainty: "none".into(),
                kind: "text".into(),
                text: "joao".into(),
                body: "joao".into(),
            }],
        )
        .unwrap();

        let generator = Fake {
            text: format!(
                r#"[{{"segment_id":{id},"verdict":"match","entity":"Joao","reason":"nome"}}]"#
            ),
            calls: AtomicUsize::new(0),
        };
        let candidates = vec![Candidate {
            segment_id: id,
            shard: "2025-11-15".into(),
            signals: vec![],
            score: 1.0,
        }];

        let first = judge_batch(
            &db,
            "p1",
            "m",
            &plan(),
            &candidates,
            |_| "joao".to_string(),
            &generator,
            8,
        )
        .unwrap();
        assert_eq!(first.len(), 1);
        assert_eq!(first[0].verdict, Verdict::Match);
        assert_eq!(generator.calls.load(Ordering::SeqCst), 1);

        let second = judge_batch(
            &db,
            "p1",
            "m",
            &plan(),
            &candidates,
            |_| "joao".to_string(),
            &generator,
            8,
        )
        .unwrap();
        assert_eq!(second.len(), 1);
        assert_eq!(generator.calls.load(Ordering::SeqCst), 1);
    }
}
