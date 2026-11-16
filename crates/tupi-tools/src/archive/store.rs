//! Banco do acervo em redb: fontes, segmentos, termos e julgamentos.

use std::fs::OpenOptions;
use std::path::{Path, PathBuf};

use redb::{
    Database, MultimapTableDefinition, ReadableDatabase, ReadableMultimapTable, ReadableTable,
    ReadableTableMetadata, TableDefinition,
};
use serde::{Deserialize, Serialize};

use super::error::ArchiveError;

pub const SCHEMA_VERSION: u32 = 1;

const META: TableDefinition<&str, &str> = TableDefinition::new("archive_meta");
const SOURCES: TableDefinition<&str, &[u8]> = TableDefinition::new("archive_sources");
const SEGMENTS: TableDefinition<u64, &[u8]> = TableDefinition::new("archive_segments");
const SRCSEG: MultimapTableDefinition<&str, u64> = MultimapTableDefinition::new("archive_srcseg");
const TERMS: MultimapTableDefinition<&str, u64> = MultimapTableDefinition::new("archive_terms");
const GRAMS: MultimapTableDefinition<&str, u64> = MultimapTableDefinition::new("archive_grams");
const PHONES: MultimapTableDefinition<&str, u64> = MultimapTableDefinition::new("archive_phones");
const ALIASES: TableDefinition<&str, &str> = TableDefinition::new("archive_aliases");
const VERDICTS: TableDefinition<(&str, &str, u64), &[u8]> =
    TableDefinition::new("archive_verdicts");
const JOBS: TableDefinition<(&str, &str), &str> = TableDefinition::new("archive_jobs");

/// Tipo do arquivo no acervo.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum SourceKind {
    Text,
    Image,
    Media,
}

/// Estado de processamento de uma fonte.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum SourceStatus {
    Pending,
    Indexed,
    Quarantined,
    Gap,
}

/// Arquivo do acervo e o que já foi extraído dele.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SourceRecord {
    pub path: String,
    pub hash: String,
    pub mtime: u64,
    pub size: u64,
    pub kind: SourceKind,
    pub status: SourceStatus,
    pub attempts: u32,
    pub reason: Option<String>,
    pub covered_seconds: Option<f64>,
    pub total_seconds: Option<f64>,
    /// Dia (AAAA-MM-DD) usado como shard, com o grau de certeza.
    pub day: Option<String>,
}

/// Segmento persistido, com o texto original e o normalizado.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SegmentRecord {
    pub id: u64,
    pub source: String,
    pub shard: String,
    pub line_start: usize,
    pub line_end: usize,
    pub ts: Option<i64>,
    pub certainty: String,
    pub kind: String,
    pub text: String,
    pub body: String,
}

/// Banco do acervo, com trava de instância única ao lado do arquivo.
pub struct ArchiveDb {
    db: Database,
    lock: PathBuf,
}

impl ArchiveDb {
    /// Abre (ou cria) o banco. Falha com `Locked` se outro `ArchiveDb` vivo
    /// já usa o mesmo caminho.
    pub fn open(path: &Path) -> Result<Self, ArchiveError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let lock = path.with_extension("lock");
        OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&lock)
            .map_err(|_| ArchiveError::Locked(lock.clone()))?;

        let db = Database::create(path).map_err(|error| ArchiveError::Db(error.to_string()))?;
        let store = Self { db, lock };
        store.write_schema_version()?;
        Ok(store)
    }

    pub fn schema_version(&self) -> u32 {
        self.get_meta("schema_version")
            .ok()
            .flatten()
            .and_then(|value| value.parse().ok())
            .unwrap_or(0)
    }

    fn write_schema_version(&self) -> Result<(), ArchiveError> {
        let txn = self.begin_write()?;
        {
            let mut meta = txn.open_table(META).map_err(db_err)?;
            meta.insert("schema_version", SCHEMA_VERSION.to_string().as_str())
                .map_err(db_err)?;
            // Criar as tabelas na abertura evita "table does not exist" na
            // primeira leitura de um acervo recém-criado.
            txn.open_table(SOURCES).map_err(db_err)?;
            txn.open_table(SEGMENTS).map_err(db_err)?;
            txn.open_multimap_table(SRCSEG).map_err(db_err)?;
            txn.open_multimap_table(TERMS).map_err(db_err)?;
            txn.open_multimap_table(GRAMS).map_err(db_err)?;
            txn.open_multimap_table(PHONES).map_err(db_err)?;
            txn.open_table(ALIASES).map_err(db_err)?;
            txn.open_table(VERDICTS).map_err(db_err)?;
            txn.open_table(JOBS).map_err(db_err)?;
        }
        txn.commit().map_err(db_err)
    }

    pub fn put_meta(&self, key: &str, value: &str) -> Result<(), ArchiveError> {
        let txn = self.begin_write()?;
        {
            let mut meta = txn.open_table(META).map_err(db_err)?;
            meta.insert(key, value).map_err(db_err)?;
        }
        txn.commit().map_err(db_err)
    }

    pub fn get_meta(&self, key: &str) -> Result<Option<String>, ArchiveError> {
        let txn = self.db.begin_read().map_err(db_err)?;
        let table = txn.open_table(META).map_err(db_err)?;
        Ok(table
            .get(key)
            .map_err(db_err)?
            .map(|value| value.value().to_string()))
    }

    fn begin_write(&self) -> Result<redb::WriteTransaction, ArchiveError> {
        self.db.begin_write().map_err(db_err)
    }

    /// Grava (ou substitui) uma fonte.
    pub fn put_source(&self, record: &SourceRecord) -> Result<(), ArchiveError> {
        let bytes = serde_json::to_vec(record).map_err(|error| ArchiveError::Db(error.to_string()))?;
        let txn = self.begin_write()?;
        {
            let mut table = txn.open_table(SOURCES).map_err(db_err)?;
            table.insert(record.path.as_str(), bytes.as_slice()).map_err(db_err)?;
        }
        txn.commit().map_err(db_err)
    }

    pub fn source(&self, path: &str) -> Result<Option<SourceRecord>, ArchiveError> {
        let txn = self.db.begin_read().map_err(db_err)?;
        let table = txn.open_table(SOURCES).map_err(db_err)?;
        let Some(raw) = table.get(path).map_err(db_err)? else {
            return Ok(None);
        };
        serde_json::from_slice(raw.value()).map(Some).map_err(|error| ArchiveError::Db(error.to_string()))
    }

    pub fn sources(&self) -> Result<Vec<SourceRecord>, ArchiveError> {
        let txn = self.db.begin_read().map_err(db_err)?;
        let table = txn.open_table(SOURCES).map_err(db_err)?;
        let mut out = Vec::new();
        for entry in table.iter().map_err(db_err)? {
            let (_, raw) = entry.map_err(db_err)?;
            out.push(
                serde_json::from_slice(raw.value())
                    .map_err(|error| ArchiveError::Db(error.to_string()))?,
            );
        }
        Ok(out)
    }

    /// Fontes pendentes, na ordem do caminho.
    pub fn pending(&self, limit: usize) -> Result<Vec<SourceRecord>, ArchiveError> {
        let mut out = self
            .sources()?
            .into_iter()
            .filter(|source| source.status == SourceStatus::Pending)
            .collect::<Vec<_>>();
        out.sort_by(|a, b| a.path.cmp(&b.path));
        out.truncate(limit);
        Ok(out)
    }

    /// Remove a fonte e todos os seus segmentos e postings.
    pub fn remove_source(&self, path: &str) -> Result<(), ArchiveError> {
        let ids = self.segment_ids_of(path)?;
        let txn = self.begin_write()?;
        {
            let mut sources = txn.open_table(SOURCES).map_err(db_err)?;
            sources.remove(path).map_err(db_err)?;
            let mut segments = txn.open_table(SEGMENTS).map_err(db_err)?;
            let mut srcseg = txn.open_multimap_table(SRCSEG).map_err(db_err)?;
            let mut terms = txn.open_multimap_table(TERMS).map_err(db_err)?;
            let mut grams = txn.open_multimap_table(GRAMS).map_err(db_err)?;
            let mut phones = txn.open_multimap_table(PHONES).map_err(db_err)?;
            for id in &ids {
                if let Some(raw) = segments.remove(*id).map_err(db_err)? {
                    let record: SegmentRecord = serde_json::from_slice(raw.value())
                        .map_err(|error| ArchiveError::Db(error.to_string()))?;
                    for token in record.body.split_whitespace() {
                        let _ = terms.remove(token, *id);
                        let key = super::terms::phonetic(token);
                        let _ = phones.remove(key.as_str(), *id);
                        for gram in super::terms::grams(token) {
                            let _ = grams.remove(gram.as_str(), *id);
                        }
                    }
                }
            }
            srcseg.remove_all(path).map_err(db_err)?;
        }
        txn.commit().map_err(db_err)
    }

    /// Ids dos segmentos de uma fonte.
    pub fn segment_ids_of(&self, path: &str) -> Result<Vec<u64>, ArchiveError> {
        let txn = self.db.begin_read().map_err(db_err)?;
        let table = txn.open_multimap_table(SRCSEG).map_err(db_err)?;
        let mut out = Vec::new();
        for entry in table.get(path).map_err(db_err)? {
            out.push(entry.map_err(db_err)?.value());
        }
        Ok(out)
    }

    /// Grava os segmentos de uma fonte e seus postings, numa transação.
    pub fn put_segments(&self, source: &str, records: &[SegmentRecord]) -> Result<(), ArchiveError> {
        let txn = self.begin_write()?;
        {
            let mut segments = txn.open_table(SEGMENTS).map_err(db_err)?;
            let mut srcseg = txn.open_multimap_table(SRCSEG).map_err(db_err)?;
            let mut terms = txn.open_multimap_table(TERMS).map_err(db_err)?;
            let mut grams = txn.open_multimap_table(GRAMS).map_err(db_err)?;
            let mut phones = txn.open_multimap_table(PHONES).map_err(db_err)?;
            for record in records {
                let bytes = serde_json::to_vec(record)
                    .map_err(|error| ArchiveError::Db(error.to_string()))?;
                segments.insert(record.id, bytes.as_slice()).map_err(db_err)?;
                srcseg.insert(source, record.id).map_err(db_err)?;
                for token in record.body.split_whitespace() {
                    terms.insert(token, record.id).map_err(db_err)?;
                    phones
                        .insert(super::terms::phonetic(token).as_str(), record.id)
                        .map_err(db_err)?;
                    for gram in super::terms::grams(token) {
                        grams.insert(gram.as_str(), record.id).map_err(db_err)?;
                    }
                }
            }
        }
        txn.commit().map_err(db_err)
    }

    pub fn segment(&self, id: u64) -> Result<Option<SegmentRecord>, ArchiveError> {
        let txn = self.db.begin_read().map_err(db_err)?;
        let table = txn.open_table(SEGMENTS).map_err(db_err)?;
        let Some(raw) = table.get(id).map_err(db_err)? else {
            return Ok(None);
        };
        serde_json::from_slice(raw.value())
            .map(Some)
            .map_err(|error| ArchiveError::Db(error.to_string()))
    }

    /// Próximo id de segmento (contador no meta).
    pub fn next_segment_id(&self) -> Result<u64, ArchiveError> {
        let current = self
            .get_meta("next_segment")?
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or(1);
        self.put_meta("next_segment", &(current + 1).to_string())?;
        Ok(current)
    }

    pub fn segments_len(&self) -> Result<usize, ArchiveError> {
        let txn = self.db.begin_read().map_err(db_err)?;
        let table = txn.open_table(SEGMENTS).map_err(db_err)?;
        Ok(table.len().map_err(db_err)? as usize)
    }

    pub fn terms_containing(&self, needle: &str) -> Result<Vec<u64>, ArchiveError> {
        let txn = self.db.begin_read().map_err(db_err)?;
        let table = txn.open_multimap_table(TERMS).map_err(db_err)?;
        let mut out = Vec::new();
        if let Ok(values) = table.get(needle).map_err(db_err) {
            for entry in values {
                out.push(entry.map_err(db_err)?.value());
            }
        }
        Ok(out)
    }

    pub fn terms_with_prefix(&self, prefix: &str, limit: usize) -> Result<Vec<u64>, ArchiveError> {
        let txn = self.db.begin_read().map_err(db_err)?;
        let table = txn.open_multimap_table(TERMS).map_err(db_err)?;
        let mut out = Vec::new();
        for entry in table.iter().map_err(db_err)? {
            let (key, values) = entry.map_err(db_err)?;
            let key = key.value();
            if !key.starts_with(prefix) {
                if key > prefix {
                    break;
                }
                continue;
            }
            for value in values {
                out.push(value.map_err(db_err)?.value());
                if out.len() >= limit {
                    return Ok(out);
                }
            }
            if out.len() >= limit {
                break;
            }
        }
        Ok(out)
    }

    pub fn segments_with_gram(&self, gram: &str) -> Result<Vec<u64>, ArchiveError> {
        let txn = self.db.begin_read().map_err(db_err)?;
        let table = txn.open_multimap_table(GRAMS).map_err(db_err)?;
        let mut out = Vec::new();
        if let Ok(values) = table.get(gram).map_err(db_err) {
            for entry in values {
                out.push(entry.map_err(db_err)?.value());
            }
        }
        Ok(out)
    }

    pub fn segments_with_phone(&self, phone: &str) -> Result<Vec<u64>, ArchiveError> {
        let txn = self.db.begin_read().map_err(db_err)?;
        let table = txn.open_multimap_table(PHONES).map_err(db_err)?;
        let mut out = Vec::new();
        if let Ok(values) = table.get(phone).map_err(db_err) {
            for entry in values {
                out.push(entry.map_err(db_err)?.value());
            }
        }
        Ok(out)
    }

    pub fn set_alias(&self, alias: &str, canonical: &str) -> Result<(), ArchiveError> {
        let txn = self.begin_write()?;
        {
            let mut table = txn.open_table(ALIASES).map_err(db_err)?;
            table.insert(alias, canonical).map_err(db_err)?;
        }
        txn.commit().map_err(db_err)
    }

    pub fn alias(&self, alias: &str) -> Result<Option<String>, ArchiveError> {
        let txn = self.db.begin_read().map_err(db_err)?;
        let table = txn.open_table(ALIASES).map_err(db_err)?;
        Ok(table
            .get(alias)
            .map_err(db_err)?
            .map(|value| value.value().to_string()))
    }

    pub fn put_verdict(
        &self,
        plan_hash: &str,
        model: &str,
        segment_id: u64,
        payload: &[u8],
    ) -> Result<(), ArchiveError> {
        let txn = self.begin_write()?;
        {
            let mut table = txn.open_table(VERDICTS).map_err(db_err)?;
            table
                .insert((plan_hash, model, segment_id), payload)
                .map_err(db_err)?;
        }
        txn.commit().map_err(db_err)
    }

    pub fn verdict(
        &self,
        plan_hash: &str,
        model: &str,
        segment_id: u64,
    ) -> Result<Option<Vec<u8>>, ArchiveError> {
        let txn = self.db.begin_read().map_err(db_err)?;
        let table = txn.open_table(VERDICTS).map_err(db_err)?;
        Ok(table
            .get((plan_hash, model, segment_id))
            .map_err(db_err)?
            .map(|value| value.value().to_vec()))
    }

    pub fn set_job(&self, shard: &str, plan_hash: &str, status: &str) -> Result<(), ArchiveError> {
        let txn = self.begin_write()?;
        {
            let mut table = txn.open_table(JOBS).map_err(db_err)?;
            table.insert((shard, plan_hash), status).map_err(db_err)?;
        }
        txn.commit().map_err(db_err)
    }

    pub fn job(&self, shard: &str, plan_hash: &str) -> Result<Option<String>, ArchiveError> {
        let txn = self.db.begin_read().map_err(db_err)?;
        let table = txn.open_table(JOBS).map_err(db_err)?;
        Ok(table
            .get((shard, plan_hash))
            .map_err(db_err)?
            .map(|value| value.value().to_string()))
    }

    pub fn shards(&self) -> Result<Vec<String>, ArchiveError> {
        let txn = self.db.begin_read().map_err(db_err)?;
        let table = txn.open_table(SEGMENTS).map_err(db_err)?;
        let mut out = std::collections::BTreeSet::new();
        for entry in table.iter().map_err(db_err)? {
            let (_, raw) = entry.map_err(db_err)?;
            let record: SegmentRecord = serde_json::from_slice(raw.value())
                .map_err(|error| ArchiveError::Db(error.to_string()))?;
            out.insert(record.shard);
        }
        Ok(out.into_iter().collect())
    }
}

impl Drop for ArchiveDb {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.lock);
    }
}

fn db_err(error: impl std::fmt::Display) -> ArchiveError {
    ArchiveError::Db(error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opening_twice_on_the_same_path_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("archive.db");
        let _first = ArchiveDb::open(&path).unwrap();
        assert!(matches!(ArchiveDb::open(&path), Err(ArchiveError::Locked(_))));
    }

    #[test]
    fn schema_is_created_once_and_reopening_keeps_data() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("archive.db");
        {
            let db = ArchiveDb::open(&path).unwrap();
            assert_eq!(db.schema_version(), SCHEMA_VERSION);
            db.put_meta("hello", "mundo").unwrap();
        }
        let db = ArchiveDb::open(&path).unwrap();
        assert_eq!(db.schema_version(), SCHEMA_VERSION);
        assert_eq!(db.get_meta("hello").unwrap().as_deref(), Some("mundo"));
    }

    #[test]
    fn segments_round_trip_with_postings() {
        let dir = tempfile::tempdir().unwrap();
        let db = ArchiveDb::open(&dir.path().join("a.db")).unwrap();
        let id = db.next_segment_id().unwrap();
        let record = SegmentRecord {
            id,
            source: "d1.txt".into(),
            shard: "2025-11-15".into(),
            line_start: 1,
            line_end: 1,
            ts: None,
            certainty: "none".into(),
            kind: "text".into(),
            text: "falei com Joao".into(),
            body: "falei com joao".into(),
        };
        db.put_segments("d1.txt", &[record]).unwrap();
        assert_eq!(db.segments_len().unwrap(), 1);
        assert_eq!(db.terms_containing("joao").unwrap(), vec![id]);
        assert!(db.terms_with_prefix("joa", 10).unwrap().contains(&id));
        let key = crate::archive::terms::phonetic("joao");
        assert!(db.segments_with_phone(&key).unwrap().contains(&id));
        db.remove_source("d1.txt").unwrap();
        assert_eq!(db.segments_len().unwrap(), 0);
    }
}
