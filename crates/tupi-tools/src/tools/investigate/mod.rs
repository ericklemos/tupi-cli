//! Tool `investigate`: plano, varredura, julgamento e dossiê.

use std::path::PathBuf;
use std::sync::Arc;

use serde_json::Value;
use tupi_kernel::generate::GeneratorSlot;
use tupi_kernel::{ModeMask, Tool, ToolError, ToolSpec};

use crate::archive::error::ArchiveError;
use crate::archive::judge::judge_batch;
use crate::archive::plan::{build, InvestigationPlan};
use crate::archive::reduce::{coverage_of, reduce};
use crate::archive::report;
use crate::archive::scan::{scan_shard, Candidate};
use crate::archive::store::{ArchiveDb, SegmentRecord};

use super::archive::ArchiveSession;

/// Opções da tool de investigação.
#[derive(Clone)]
pub struct InvestigateOptions {
    pub reports_dir: PathBuf,
    pub model: String,
    pub batch: usize,
}

/// Investigação sobre o acervo.
pub struct InvestigateTool {
    session: Arc<ArchiveSession>,
    generator: Option<Arc<GeneratorSlot>>,
    options: InvestigateOptions,
}

impl InvestigateTool {
    pub fn new(
        session: Arc<ArchiveSession>,
        generator: Option<Arc<GeneratorSlot>>,
        options: InvestigateOptions,
    ) -> Self {
        Self {
            session,
            generator,
            options,
        }
    }

    fn generator(&self) -> Result<Arc<dyn tupi_kernel::generate::Generator>, ToolError> {
        self.generator
            .as_ref()
            .and_then(|slot| slot.get())
            .ok_or_else(|| ToolError::Other("nenhum gerador configurado para o modo docs".into()))
    }

    fn load_plan(&self, db: &ArchiveDb, hash: &str) -> Result<InvestigationPlan, ToolError> {
        let raw = db
            .get_meta(&format!("plan:{hash}"))
            .map_err(other)?
            .ok_or_else(|| ToolError::NotFound(hash.to_string()))?;
        serde_json::from_str(&raw).map_err(|error| ToolError::Other(error.to_string()))
    }

    fn store_plan(&self, db: &ArchiveDb, plan: &InvestigationPlan) -> Result<String, ToolError> {
        let hash = plan.hash();
        let raw = serde_json::to_string(plan).map_err(|error| ToolError::Other(error.to_string()))?;
        db.put_meta(&format!("plan:{hash}"), &raw).map_err(other)?;
        Ok(hash)
    }

    fn candidates(&self, db: &ArchiveDb, plan: &InvestigationPlan) -> Result<Vec<Candidate>, ToolError> {
        scan_shard(db, plan, "*", 512).map_err(other)
    }

    fn segments_of(&self, db: &ArchiveDb, candidates: &[Candidate]) -> Result<Vec<SegmentRecord>, ToolError> {
        let mut out = Vec::new();
        for candidate in candidates {
            if let Some(record) = db.segment(candidate.segment_id).map_err(other)? {
                out.push(record);
            }
        }
        Ok(out)
    }

    fn context_of(db: &ArchiveDb, id: u64) -> String {
        db.segment(id)
            .ok()
            .flatten()
            .map(|record| record.text)
            .unwrap_or_default()
    }

    fn judge(
        &self,
        db: &ArchiveDb,
        plan: &InvestigationPlan,
        candidates: &[Candidate],
        batch: usize,
    ) -> Result<usize, ToolError> {
        let generator = self.generator()?;
        let hash = plan.hash();
        let judgments = judge_batch(
            db,
            &hash,
            &self.options.model,
            plan,
            candidates,
            |id| Self::context_of(db, id),
            generator.as_ref(),
            batch,
        )
        .map_err(other)?;
        Ok(judgments.len())
    }
}

fn other(error: ArchiveError) -> ToolError {
    ToolError::Other(error.to_string())
}

impl Tool for InvestigateTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "investigate".into(),
            description: "Investiga o acervo por uma pessoa ou assunto. Ações: plan, run, status, report, cancel.".into(),
            schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "action": { "type": "string", "enum": ["plan", "run", "status", "report", "cancel"] },
                    "question": { "type": "string" },
                    "plan_hash": { "type": "string" },
                    "from": { "type": "integer" },
                    "to": { "type": "integer" },
                    "batch": { "type": "integer" }
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
            "plan" => {
                let question = arguments
                    .get("question")
                    .and_then(Value::as_str)
                    .ok_or_else(|| ToolError::Other("question obrigatória".into()))?;
                let from = arguments.get("from").and_then(Value::as_i64);
                let to = arguments.get("to").and_then(Value::as_i64);
                let generator = self.generator()?;
                let plan = build(question, from, to, "pt-BR", generator.as_ref()).map_err(other)?;
                let hash = self.store_plan(&db, &plan)?;
                Ok(format!(
                    "plano {} — formas: {}",
                    &hash[..12.min(hash.len())],
                    plan.forms().join(", ")
                ))
            }
            "status" | "run" | "report" | "cancel" => {
                let hash = arguments
                    .get("plan_hash")
                    .and_then(Value::as_str)
                    .ok_or_else(|| ToolError::Other("plan_hash obrigatório".into()))?;
                let plan = self.load_plan(&db, hash)?;
                let candidates = self.candidates(&db, &plan)?;
                let total = candidates.len();
                match action {
                    "status" => Ok(format!("{} candidatos no plano {hash}", total)),
                    "cancel" => {
                        db.set_job("*", hash, "cancelado").map_err(other)?;
                        Ok("investigação cancelada".into())
                    }
                    "run" => {
                        let batch = arguments
                            .get("batch")
                            .and_then(Value::as_u64)
                            .map(|value| value as usize)
                            .unwrap_or(self.options.batch);
                        let judged = self.judge(&db, &plan, &candidates, batch)?;
                        db.set_job("*", hash, "rodando").map_err(other)?;
                        Ok(format!("julgados {judged} de {total}"))
                    }
                    _ => {
                        let batch = self.options.batch.max(1);
                        let mut guard = 0;
                        while guard < 64 {
                            let before = self.judge(&db, &plan, &candidates, batch)?;
                            if before < batch || before >= total {
                                break;
                            }
                            guard += 1;
                        }
                        let generator = self.generator()?;
                        let judgments = judge_batch(
                            &db,
                            &plan.hash(),
                            &self.options.model,
                            &plan,
                            &candidates,
                            |id| Self::context_of(&db, id),
                            generator.as_ref(),
                            batch,
                        )
                        .map_err(other)?;
                        let segments = self.segments_of(&db, &candidates)?;
                        let coverage = coverage_of(&db.sources().map_err(other)?);
                        let dossier = reduce(&plan, &judgments, &segments, coverage);
                        let path = report::write(&self.options.reports_dir, &dossier).map_err(other)?;
                        db.set_job("*", &plan.hash(), "concluido").map_err(other)?;
                        Ok(format!("dossiê em {}", path.display()))
                    }
                }
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
