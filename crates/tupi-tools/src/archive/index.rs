//! Indexação: transforma fontes pendentes em segmentos citáveis.

use std::path::Path;

use super::derive::{Cue, DerivedCache, Ocr, Transcriber};
use super::error::ArchiveError;
use super::segment::{segment_file, SegmentKind, TimeCertainty};
use super::store::{ArchiveDb, SegmentRecord, SourceKind, SourceRecord, SourceStatus};

/// Resultado de uma passada de indexação.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct IndexReport {
    pub files: usize,
    pub segments: usize,
    pub gaps: usize,
}

/// Converte fontes pendentes em segmentos, em lotes.
pub struct Indexer<'a> {
    pub db: &'a ArchiveDb,
    pub derived: &'a DerivedCache,
    pub transcriber: Option<&'a dyn Transcriber>,
    pub ocr: Option<&'a dyn Ocr>,
    pub asr: bool,
    pub ocr_enabled: bool,
}

impl Indexer<'_> {
    pub fn run_batch(&self, limit: usize) -> Result<IndexReport, ArchiveError> {
        let mut report = IndexReport::default();
        for source in self.db.pending(limit)? {
            match self.index_source(&source) {
                Ok(Some((count, mut record))) => {
                    record.status = SourceStatus::Indexed;
                    self.db.put_source(&record)?;
                    report.files += 1;
                    report.segments += count;
                }
                Ok(None) => {
                    let mut record = source.clone();
                    record.status = SourceStatus::Gap;
                    record.reason = Some(self.gap_reason(&source));
                    self.db.put_source(&record)?;
                    report.gaps += 1;
                }
                Err(error) => {
                    super::ingest::fail_source(self.db, &source, &error.to_string())?;
                    report.gaps += 1;
                }
            }
        }
        Ok(report)
    }

    fn gap_reason(&self, source: &SourceRecord) -> String {
        match source.kind {
            SourceKind::Media => "sem transcritor configurado".to_string(),
            SourceKind::Image => "sem OCR configurado".to_string(),
            SourceKind::Text => "arquivo ilegível".to_string(),
        }
    }

    fn index_source(
        &self,
        source: &SourceRecord,
    ) -> Result<Option<(usize, SourceRecord)>, ArchiveError> {
        let path = Path::new(&source.path);
        let fallback = fallback_date(path, source.mtime);
        let mut total_seconds = None;

        let cues: Option<Vec<Cue>> = match source.kind {
            SourceKind::Text => None,
            SourceKind::Image => {
                if !self.ocr_enabled {
                    return Ok(None);
                }
                let Some(ocr) = self.ocr else {
                    return Ok(None);
                };
                Some(self.derived.get_or_compute(&source.hash, || ocr.read(path))?)
            }
            SourceKind::Media => {
                if !self.asr {
                    return Ok(None);
                }
                let Some(transcriber) = self.transcriber else {
                    return Ok(None);
                };
                let cues = self
                    .derived
                    .get_or_compute(&source.hash, || transcriber.transcribe(path))?;
                total_seconds = cues.last().map(|cue| cue.end);
                Some(cues)
            }
        };

        let mut records = Vec::new();
        let mut record_with_day = source.clone();
        match cues {
            None => {
                let text = std::fs::read_to_string(path)?;
                for segment in segment_file(&source.path, &text, fallback) {
                    let ts = segment.ts;
                    let day = day_of(ts);
                    record_with_day.day = Some(day.clone());
                    records.push(build_record(
                        self.db,
                        &source.path,
                        &day,
                        segment.line_start,
                        segment.line_end,
                        ts,
                        segment.certainty,
                        SegmentKind::Text,
                        segment.text,
                    )?);
                }
            }
            Some(cues) => {
                let kind = match source.kind {
                    SourceKind::Image => SegmentKind::Ocr,
                    _ => SegmentKind::Transcript,
                };
                for cue in cues {
                    if cue.text.trim().is_empty() {
                        continue;
                    }
                    let ts = fallback.map(|(ts, _)| ts + cue.start as i64);
                    let day = day_of(ts);
                    record_with_day.day = Some(day.clone());
                    records.push(build_record(
                        self.db,
                        &source.path,
                        &day,
                        1,
                        1,
                        ts,
                        fallback.map(|(_, certainty)| certainty).unwrap_or(TimeCertainty::None),
                        kind,
                        cue.text,
                    )?);
                }
            }
        }

        record_with_day.covered_seconds = total_seconds;
        record_with_day.total_seconds = total_seconds;
        // Só grava os segmentos quando pelo menos um existe; arquivo vazio não
        // é lacuna, é arquivo vazio.
        if !records.is_empty() {
            self.db.put_segments(&source.path, &records)?;
        }
        Ok(Some((records.len(), record_with_day)))
    }
}

#[allow(clippy::too_many_arguments)]
fn build_record(
    db: &ArchiveDb,
    source: &str,
    shard: &str,
    line_start: usize,
    line_end: usize,
    ts: Option<i64>,
    certainty: TimeCertainty,
    kind: SegmentKind,
    text: String,
) -> Result<SegmentRecord, ArchiveError> {
    Ok(SegmentRecord {
        id: db.next_segment_id()?,
        source: source.to_string(),
        shard: shard.to_string(),
        line_start,
        line_end,
        ts,
        certainty: format!("{certainty:?}").to_lowercase(),
        kind: format!("{kind:?}").to_lowercase(),
        body: super::normalize::search_text(&text),
        text,
    })
}

/// Data de fallback: nome do arquivo (AAAA-MM-DD) e, por último, mtime.
fn fallback_date(path: &Path, mtime: u64) -> Option<(i64, TimeCertainty)> {
    let name = path.file_name()?.to_string_lossy();
    if let Some(day) = find_iso_date(&name) {
        return Some((day, TimeCertainty::FromName));
    }
    if mtime > 0 {
        return Some((mtime as i64, TimeCertainty::FromMtime));
    }
    None
}

fn find_iso_date(name: &str) -> Option<i64> {
    let bytes = name.as_bytes();
    for index in 0..bytes.len().saturating_sub(9) {
        let slice = &name[index..index + 10];
        if let Some(ts) = parse_iso_day(slice) {
            return Some(ts);
        }
    }
    None
}

fn parse_iso_day(text: &str) -> Option<i64> {
    let mut parts = text.split('-');
    let year: i64 = parts.next()?.parse().ok()?;
    let month: i64 = parts.next()?.parse().ok()?;
    let day: i64 = parts.next()?.parse().ok()?;
    if !(1900..=2999).contains(&year) || !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    Some(super::segment::days_from_civil(year, month, day) * 86_400)
}

/// Dia (AAAA-MM-DD) do shard a partir do timestamp.
pub fn day_of(ts: Option<i64>) -> String {
    match ts {
        Some(ts) => super::segment::civil_from_days(ts.div_euclid(86_400)),
        None => "sem-data".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(dir: &Path, name: &str, body: &str) {
        std::fs::write(dir.join(name), body).unwrap();
    }

    fn indexer<'a>(db: &'a ArchiveDb, derived: &'a DerivedCache) -> Indexer<'a> {
        Indexer {
            db,
            derived,
            transcriber: None,
            ocr: None,
            asr: true,
            ocr_enabled: true,
        }
    }

    #[test]
    fn indexing_a_text_file_fills_segments_and_marks_it_indexed() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "2025-11-15.txt", "[15/11/2025, 10:00:00] Ana: falei com joao\n");
        let db = ArchiveDb::open(&dir.path().join("a.db")).unwrap();
        let derived = DerivedCache::new(dir.path().join("derived"));
        super::super::ingest::ingest(&db, &[dir.path().to_path_buf()], &[]).unwrap();

        let report = indexer(&db, &derived).run_batch(10).unwrap();
        assert_eq!(report.files, 1);
        assert!(report.segments >= 1);
        assert_eq!(db.segments_len().unwrap(), report.segments);
        assert_eq!(
            db.sources().unwrap()[0].status,
            SourceStatus::Indexed
        );
    }

    #[test]
    fn media_without_a_transcriber_is_counted_as_a_gap() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "audio.mp3", "nao-e-audio-de-verdade");
        let db = ArchiveDb::open(&dir.path().join("a.db")).unwrap();
        let derived = DerivedCache::new(dir.path().join("derived"));
        super::super::ingest::ingest(&db, &[dir.path().to_path_buf()], &[]).unwrap();

        let report = indexer(&db, &derived).run_batch(10).unwrap();
        assert_eq!(report.gaps, 1);
        assert_eq!(db.pending(10).unwrap().len(), 0);
        assert_eq!(db.sources().unwrap()[0].status, SourceStatus::Gap);
    }
}
