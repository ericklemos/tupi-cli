//! Varredura determinística por shard: sinais, sem julgamento.

use super::error::ArchiveError;
use super::plan::InvestigationPlan;
use super::store::{ArchiveDb, SegmentRecord};
use super::terms::{grams, phonetic};

/// Sinal que produziu um candidato.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Signal {
    Exact,
    Prefix,
    Trigram,
    Phonetic,
    Dense,
}

/// Segmento candidato, com os sinais que o trouxeram.
#[derive(Clone, Debug, PartialEq)]
pub struct Candidate {
    pub segment_id: u64,
    pub shard: String,
    pub signals: Vec<Signal>,
    pub score: f64,
}

fn weight(signal: Signal) -> f64 {
    match signal {
        Signal::Exact => 1.0,
        Signal::Prefix => 0.7,
        Signal::Trigram => 0.5,
        Signal::Phonetic => 0.6,
        Signal::Dense => 0.8,
    }
}

/// Varre um shard (ou todos, com "*") pelas formas do plano.
pub fn scan_shard(
    db: &ArchiveDb,
    plan: &InvestigationPlan,
    shard: &str,
    limit_per_signal: usize,
) -> Result<Vec<Candidate>, ArchiveError> {
    let mut found: std::collections::BTreeMap<u64, Vec<Signal>> = std::collections::BTreeMap::new();
    let push = |ids: Vec<u64>, signal: Signal, found: &mut std::collections::BTreeMap<u64, Vec<Signal>>| {
        for id in ids {
            found.entry(id).or_default().push(signal);
        }
    };

    for form in plan.forms() {
        push(db.terms_containing(&form)?, Signal::Exact, &mut found);
        push(
            db.terms_with_prefix(&form, limit_per_signal)?,
            Signal::Prefix,
            &mut found,
        );
        for gram in grams(&form) {
            push(db.segments_with_gram(&gram)?, Signal::Trigram, &mut found);
        }
        push(
            db.segments_with_phone(&phonetic(&form))?,
            Signal::Phonetic,
            &mut found,
        );
    }

    let mut out = Vec::new();
    for (id, mut signals) in found {
        signals.sort_by_key(|signal| std::cmp::Reverse(*signal as u8));
        signals.dedup();
        let Some(record) = db.segment(id)? else {
            continue;
        };
        if shard != "*" && record.shard != shard {
            continue;
        }
        if !in_range(&record, plan.from, plan.to) {
            continue;
        }
        let score: f64 = signals.iter().map(|signal| weight(*signal)).sum();
        out.push(Candidate {
            segment_id: id,
            shard: record.shard,
            signals,
            score,
        });
    }
    out.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
    Ok(out)
}

fn in_range(record: &SegmentRecord, from: Option<i64>, to: Option<i64>) -> bool {
    let Some(ts) = record.ts else {
        return from.is_none() && to.is_none();
    };
    from.is_none_or(|from| ts >= from) && to.is_none_or(|to| ts <= to)
}

/// Shards do acervo (a janela temporal é filtrada na varredura).
pub fn shards_in_range(
    db: &ArchiveDb,
    from: Option<i64>,
    to: Option<i64>,
) -> Result<Vec<String>, ArchiveError> {
    let _ = (from, to);
    db.shards()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::archive::plan::{EntityPlan, InvestigationPlan};
    use crate::archive::store::SegmentRecord;

    fn plan_with_forms(forms: &[&str]) -> InvestigationPlan {
        InvestigationPlan {
            question: "quem falou do joao?".into(),
            entities: vec![EntityPlan {
                label: "Joao".into(),
                forms: forms.iter().map(|form| form.to_string()).collect(),
            }],
            exclude: vec![],
            from: None,
            to: None,
            language: "pt-BR".into(),
        }
    }

    fn record(db: &ArchiveDb, body: &str) -> u64 {
        let id = db.next_segment_id().unwrap();
        db.put_segments(
            "d1.txt",
            &[SegmentRecord {
                id,
                source: "d1.txt".into(),
                shard: "2025-11-15".into(),
                line_start: 1,
                line_end: 1,
                ts: None,
                certainty: "none".into(),
                kind: "text".into(),
                text: body.into(),
                body: body.into(),
            }],
        )
        .unwrap();
        id
    }

    #[test]
    fn scan_finds_variants_by_exact_prefix_and_phonetic_without_any_model() {
        let dir = tempfile::tempdir().unwrap();
        let db = ArchiveDb::open(&dir.path().join("a.db")).unwrap();
        record(&db, "falei com joao");
        record(&db, "joao silva apareceu");
        record(&db, "juao mandou mensagem");

        let plan = plan_with_forms(&["joao"]);
        let hits = scan_shard(&db, &plan, "2025-11-15", 50).unwrap();
        assert_eq!(hits.len(), 3);
        assert!(hits.iter().any(|hit| hit.signals.contains(&Signal::Phonetic)));
    }
}
