//! Ingestão incremental do acervo: hash, mudanças, remoções e quarentena.

use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use super::error::ArchiveError;
use super::store::{ArchiveDb, SourceKind, SourceRecord, SourceStatus};

/// Resultado de uma passada de ingestão.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct IngestReport {
    pub added: usize,
    pub changed: usize,
    pub removed: usize,
    pub failed: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RecordOutcome {
    New,
    Changed,
    Same,
}

const ATTEMPT_LIMIT: u32 = 3;

/// Caminha as raízes e mantém `sources` em dia com o disco.
pub fn ingest(
    db: &ArchiveDb,
    roots: &[PathBuf],
    excludes: &[String],
) -> Result<IngestReport, ArchiveError> {
    let mut report = IngestReport::default();
    let mut seen: Vec<String> = Vec::new();

    for root in roots {
        let walker = ignore::WalkBuilder::new(root)
            .hidden(false)
            .git_ignore(false)
            .git_global(false)
            .git_exclude(false)
            .build();
        for entry in walker {
            let Ok(entry) = entry else {
                report.failed += 1;
                continue;
            };
            if !entry.file_type().is_some_and(|kind| kind.is_file()) {
                continue;
            }
            let path = entry.path();
            let shown = path.to_string_lossy().to_string();
            if is_excluded(&shown, excludes) {
                continue;
            }
            let Some(kind) = kind_of(path) else {
                continue;
            };
            seen.push(shown.clone());
            match record_file(db, path, &shown, kind) {
                Ok(RecordOutcome::New) => report.added += 1,
                Ok(RecordOutcome::Changed) => report.changed += 1,
                Ok(RecordOutcome::Same) => {}
                Err(_) => report.failed += 1,
            }
        }
    }

    for source in db.sources()? {
        if !seen.contains(&source.path) {
            db.remove_source(&source.path)?;
            report.removed += 1;
        }
    }

    Ok(report)
}

fn record_file(
    db: &ArchiveDb,
    path: &Path,
    shown: &str,
    kind: SourceKind,
) -> Result<RecordOutcome, ArchiveError> {
    let bytes = std::fs::read(path)?;
    let hash = blake3::hash(&bytes).to_hex().to_string();
    let metadata = std::fs::metadata(path)?;
    let mtime = metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or(0);

    let existing = db.source(shown)?;
    let (record, outcome) = match existing {
        Some(record) if record.hash == hash => return Ok(RecordOutcome::Same),
        Some(mut record) => {
            record.hash = hash;
            record.size = metadata.len();
            record.mtime = mtime;
            record.status = SourceStatus::Pending;
            record.attempts = 0;
            record.reason = None;
            record.covered_seconds = None;
            record.total_seconds = None;
            (record, RecordOutcome::Changed)
        }
        None => (
            SourceRecord {
                path: shown.to_string(),
                hash,
                mtime,
                size: metadata.len(),
                kind,
                status: SourceStatus::Pending,
                attempts: 0,
                reason: None,
                covered_seconds: None,
                total_seconds: None,
                day: None,
            },
            RecordOutcome::New,
        ),
    };
    db.put_source(&record)?;
    Ok(outcome)
}

/// Marca a fonte como quarentena depois de falhas repetidas.
pub fn fail_source(db: &ArchiveDb, source: &SourceRecord, reason: &str) -> Result<(), ArchiveError> {
    let mut record = source.clone();
    record.attempts += 1;
    record.reason = Some(reason.to_string());
    if record.attempts >= ATTEMPT_LIMIT {
        record.status = SourceStatus::Quarantined;
    }
    db.put_source(&record)
}

fn is_excluded(path: &str, excludes: &[String]) -> bool {
    excludes.iter().any(|pattern| {
        let needle = pattern.trim_start_matches("**/").trim();
        !needle.is_empty() && path.contains(needle)
    })
}

fn kind_of(path: &Path) -> Option<SourceKind> {
    let extension = path.extension()?.to_string_lossy().to_ascii_lowercase();
    match extension.as_str() {
        "txt" | "md" | "jsonl" | "csv" | "log" => Some(SourceKind::Text),
        "png" | "jpg" | "jpeg" | "webp" | "gif" | "bmp" => Some(SourceKind::Image),
        "mp4" | "mov" | "mkv" | "webm" | "mp3" | "ogg" | "opus" | "m4a" | "wav" => {
            Some(SourceKind::Media)
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file_key(path: &Path) -> String {
        path.to_string_lossy().to_string()
    }

    #[test]
    fn second_ingest_touches_nothing_and_removal_drops_the_source() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("acervo");
        std::fs::create_dir_all(&root).unwrap();
        let file = root.join("dia1.txt");
        std::fs::write(&file, "conteudo").unwrap();
        let db = ArchiveDb::open(&dir.path().join("a.db")).unwrap();

        assert_eq!(ingest(&db, &[root.clone()], &[]).unwrap().added, 1);
        let second = ingest(&db, &[root.clone()], &[]).unwrap();
        assert_eq!(second.added, 0);
        assert_eq!(second.changed, 0);
        assert_eq!(
            db.source(&file_key(&file)).unwrap().unwrap().status,
            SourceStatus::Pending
        );

        std::fs::remove_file(&file).unwrap();
        assert_eq!(ingest(&db, &[root.clone()], &[]).unwrap().removed, 1);
        assert!(db.sources().unwrap().is_empty());
    }

    #[test]
    fn editing_a_file_marks_it_changed() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("acervo");
        std::fs::create_dir_all(&root).unwrap();
        let file = root.join("dia1.txt");
        std::fs::write(&file, "um").unwrap();
        let db = ArchiveDb::open(&dir.path().join("a.db")).unwrap();
        ingest(&db, &[root.clone()], &[]).unwrap();
        std::fs::write(&file, "dois").unwrap();
        assert_eq!(ingest(&db, &[root.clone()], &[]).unwrap().changed, 1);
    }
}
