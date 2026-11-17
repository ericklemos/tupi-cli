//! Tool `archive`: estado e indexação do acervo do modo docs.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use serde_json::Value;
use tupi_kernel::{ModeMask, Tool, ToolError, ToolSpec};

use crate::archive::error::ArchiveError;
use crate::archive::store::{ArchiveDb, SourceStatus};

/// Opções de construção do acesso ao acervo.
#[derive(Clone, Debug)]
pub struct ArchiveOptions {
    pub db_path: PathBuf,
    pub roots: Vec<PathBuf>,
    pub exclude: Vec<String>,
}

/// Acesso compartilhado ao banco do acervo (uma instância por processo).
pub struct ArchiveSession {
    pub options: ArchiveOptions,
    db: Mutex<Option<Arc<ArchiveDb>>>,
}

impl ArchiveSession {
    pub fn new(options: ArchiveOptions) -> Arc<Self> {
        Arc::new(Self {
            options,
            db: Mutex::new(None),
        })
    }

    pub fn db(&self) -> Result<Arc<ArchiveDb>, ToolError> {
        let mut slot = self.db.lock().unwrap();
        if let Some(db) = slot.as_ref() {
            return Ok(Arc::clone(db));
        }
        let db = ArchiveDb::open(&self.options.db_path).map_err(other)?;
        let db = Arc::new(db);
        *slot = Some(Arc::clone(&db));
        Ok(db)
    }

    pub fn derived(&self) -> crate::archive::derive::DerivedCache {
        let base = self
            .options
            .db_path
            .parent()
            .unwrap_or_else(|| std::path::Path::new("."))
            .join("derived");
        crate::archive::derive::DerivedCache::new(base)
    }
}

fn other(error: ArchiveError) -> ToolError {
    ToolError::Other(error.to_string())
}

/// Tool de estado e indexação.
pub struct ArchiveTool {
    session: Arc<ArchiveSession>,
}

impl ArchiveTool {
    pub fn new(session: Arc<ArchiveSession>) -> Self {
        Self { session }
    }
}

impl Tool for ArchiveTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "archive".into(),
            description: "Estado e indexação do acervo. Ações: status, index, aliases.".into(),
            schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "action": { "type": "string", "enum": ["status", "index", "aliases"] },
                    "limit": { "type": "integer" },
                    "alias": { "type": "string" },
                    "canonical": { "type": "string" }
                },
                "required": ["action"]
            }),
        }
    }

    fn call(&self, arguments: Value) -> Result<String, ToolError> {
        let action = arguments
            .get("action")
            .and_then(Value::as_str)
            .ok_or_else(|| ToolError::Other("action obrigatória".into()))?;
        let db = self.session.db()?;
        match action {
            "status" => {
                let sources = db.sources().map_err(other)?;
                let indexed = sources
                    .iter()
                    .filter(|source| source.status == SourceStatus::Indexed)
                    .count();
                let pending = sources
                    .iter()
                    .filter(|source| source.status == SourceStatus::Pending)
                    .count();
                let gaps = sources
                    .iter()
                    .filter(|source| {
                        matches!(source.status, SourceStatus::Gap | SourceStatus::Quarantined)
                    })
                    .count();
                Ok(format!(
                    "{} fontes, {indexed} indexadas, {pending} pendentes, {gaps} lacunas",
                    sources.len()
                ))
            }
            "index" => {
                let limit = arguments.get("limit").and_then(Value::as_u64).unwrap_or(32) as usize;
                let ingest = crate::archive::ingest::ingest(
                    &db,
                    &self.session.options.roots,
                    &self.session.options.exclude,
                )
                .map_err(other)?;
                let derived = self.session.derived();
                let indexer = crate::archive::index::Indexer {
                    db: &db,
                    derived: &derived,
                    transcriber: None,
                    ocr: None,
                    asr: true,
                    ocr_enabled: true,
                };
                let report = indexer.run_batch(limit).map_err(other)?;
                Ok(format!(
                    "ingestão: {} novos, {} alterados, {} removidos; indexação: {} arquivos, {} segmentos, {} lacunas",
                    ingest.added,
                    ingest.changed,
                    ingest.removed,
                    report.files,
                    report.segments,
                    report.gaps
                ))
            }
            "aliases" => {
                let alias = arguments
                    .get("alias")
                    .and_then(Value::as_str)
                    .ok_or_else(|| ToolError::Other("alias obrigatório".into()))?;
                let canonical = arguments
                    .get("canonical")
                    .and_then(Value::as_str)
                    .ok_or_else(|| ToolError::Other("canonical obrigatório".into()))?;
                db.set_alias(alias, canonical).map_err(other)?;
                Ok(format!("alias '{alias}' → '{canonical}'"))
            }
            other_action => Err(ToolError::Other(format!("ação desconhecida: {other_action}"))),
        }
    }
    fn effect(&self, _arguments: &serde_json::Value) -> tupi_kernel::Effect {
        tupi_kernel::Effect::Opaque
    }
    fn mode_mask(&self) -> ModeMask {
        ModeMask::DOCS
    }
}
