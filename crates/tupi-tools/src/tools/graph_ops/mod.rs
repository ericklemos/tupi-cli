//! A tool `graph`: perguntas pontuais sobre o grafo de chamadas e o histórico.
//!
//! Absorve `assert`, `impact`, `trace`, `diff` e `why`. Não implementa motor
//! próprio — guarda as cinco tools e despacha por `kind`, como o `lookup` faz
//! com `read` e `search`. O alvo de arquivo/consumidores fica em
//! `lookup` (`kind: "graph"`), que é leitura; aqui são veredito, impacto,
//! caminho, delta e histórico.

use std::path::PathBuf;
use std::sync::Arc;

use serde::Deserialize;
use serde_json::{json, Value};

use tupi_kernel::{Tool, ToolError, ToolSpec};

use crate::assert::AssertTool;
use crate::diff::DiffTool;
use crate::impact::ImpactTool;
use crate::store::ResponseStore;
use crate::trace::TraceTool;
use crate::why::WhyTool;

/// Os `kind` aceitos, na ordem em que o schema os enumera.
pub const KINDS: [&str; 5] = ["assert", "impact", "trace", "diff", "why"];

/// Qual pergunta do grafo/histórico este item faz.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GraphKind {
    /// SIM/NÃO: o símbolo existe, é público ou é usado fora do módulo.
    Assert,
    /// Chamadores reversos que quebram se a assinatura mudar.
    Impact,
    /// Caminho de chamadas entre dois símbolos.
    Trace,
    /// Símbolos alterados desde uma revisão.
    Diff,
    /// Commits que tocaram um símbolo.
    Why,
}

impl GraphKind {
    pub fn as_str(self) -> &'static str {
        match self {
            GraphKind::Assert => "assert",
            GraphKind::Impact => "impact",
            GraphKind::Trace => "trace",
            GraphKind::Diff => "diff",
            GraphKind::Why => "why",
        }
    }

    fn list() -> String {
        KINDS.join("|")
    }
}

/// Campos por `kind`; o parse recusa o que falta e ensina a alternativa.
#[derive(Debug, Deserialize)]
pub struct GraphParams {
    pub kind: GraphKind,
    /// assert/impact/why: o símbolo alvo.
    #[serde(default)]
    pub symbol: Option<String>,
    /// assert: `exists` (padrão), `public` ou `used_outside`.
    #[serde(default)]
    pub assert: Option<String>,
    /// assert: prefixo de caminho considerado "dentro" em `used_outside`.
    #[serde(default)]
    pub scope: Option<String>,
    /// impact: inclui chamadores de teste.
    #[serde(default)]
    pub include_tests: Option<bool>,
    /// trace: símbolo de origem.
    #[serde(default)]
    pub from: Option<String>,
    /// trace: símbolo de destino.
    #[serde(default)]
    pub to: Option<String>,
    /// trace: saltos máximos (padrão 6).
    #[serde(default)]
    pub max_depth: Option<usize>,
    /// diff: revisão base (padrão HEAD).
    #[serde(default)]
    pub base: Option<String>,
    /// diff: prefixo de caminho.
    #[serde(default)]
    pub path: Option<String>,
    /// diff: seções desejadas (`impact`, `tests`).
    #[serde(default)]
    pub include: Option<Vec<String>>,
    /// why: commits máximos (padrão 10).
    #[serde(default)]
    pub max_commits: Option<usize>,
    #[serde(default)]
    pub budget_tokens: Option<usize>,
}

impl GraphParams {
    fn require(&self, field: &str, value: Option<&String>) -> Result<String, ToolError> {
        value
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
            .ok_or_else(|| self.missing(field))
    }

    fn missing(&self, field: &str) -> ToolError {
        ToolError::InvalidArguments(format!(
            "graph: `{}` exige o campo `{field}` — campos por kind: assert (symbol [+assert, scope]), \
             impact (symbol [+include_tests]), trace (from, to [+max_depth]), diff ([base, path, include]), \
             why (symbol [+max_commits])",
            self.kind.as_str()
        ))
    }

    fn assert_kind(&self) -> Result<&'static str, ToolError> {
        match self.assert.as_deref() {
            None => Ok("exists"),
            Some("exists") => Ok("exists"),
            Some("public") => Ok("public"),
            Some("used_outside") => Ok("used_outside"),
            Some(text) => Err(ToolError::InvalidArguments(format!(
                "graph: `assert` tem veredito desconhecido `{text}` — use exists|public|used_outside"
            ))),
        }
    }
}

pub struct GraphTool {
    assert: AssertTool,
    impact: ImpactTool,
    trace: TraceTool,
    diff: DiffTool,
    why: WhyTool,
}

impl GraphTool {
    pub fn new(root_dir: PathBuf, store: Arc<ResponseStore>, default_budget_tokens: usize) -> Self {
        Self {
            assert: AssertTool::new(root_dir.clone(), store.clone(), default_budget_tokens),
            impact: ImpactTool::new(root_dir.clone(), store.clone(), default_budget_tokens),
            trace: TraceTool::new(root_dir.clone(), store.clone(), default_budget_tokens),
            diff: DiffTool::new(root_dir.clone(), store.clone(), default_budget_tokens),
            why: WhyTool::new(root_dir, store, default_budget_tokens),
        }
    }
}

impl Tool for GraphTool {
    fn effect(&self, _arguments: &serde_json::Value) -> tupi_kernel::Effect {
        tupi_kernel::Effect::ReadOnly
    }

    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "graph".to_string(),
            description: "Pergunta pontual sobre o grafo de chamadas ou o histórico, escolhida por `kind`: assert (o símbolo existe/é público/é usado fora), impact (chamadores que quebram), trace (caminho A→B), diff (símbolos alterados desde uma revisão) ou why (commits que tocaram o símbolo).".to_string(),
            schema: json!({
                "type": "object",
                "properties": {
                    "kind": { "type": "string", "enum": KINDS },
                    "symbol": { "type": "string", "description": "assert/impact/why: nome exato do símbolo." },
                    "assert": { "type": "string", "enum": ["exists", "public", "used_outside"], "description": "assert: veredito desejado (padrão exists)." },
                    "scope": { "type": "string", "description": "assert: prefixo considerado dentro (padrão: o arquivo do símbolo)." },
                    "include_tests": { "type": "boolean", "description": "impact: inclui chamadores de teste." },
                    "from": { "type": "string", "description": "trace: origem." },
                    "to": { "type": "string", "description": "trace: destino." },
                    "max_depth": { "type": "integer", "minimum": 1, "maximum": 12, "description": "trace: saltos (padrão 6)." },
                    "base": { "type": "string", "description": "diff: revisão base (padrão HEAD)." },
                    "path": { "type": "string", "description": "diff: prefixo de caminho." },
                    "include": { "type": "array", "items": { "type": "string", "enum": ["impact", "tests"] }, "description": "diff: seções." },
                    "max_commits": { "type": "integer", "minimum": 1, "maximum": 50, "description": "why: commits (padrão 10)." },
                    "budget_tokens": { "type": "integer" }
                },
                "required": ["kind"]
            }),
        }
    }

    fn call(&self, arguments: Value) -> Result<String, ToolError> {
        let params: GraphParams = serde_json::from_value(arguments)
            .map_err(|error| ToolError::InvalidArguments(error.to_string()))?;
        let budget = params.budget_tokens;
        match params.kind {
            GraphKind::Assert => {
                let symbol = params.require("symbol", params.symbol.as_ref())?;
                let kind = params.assert_kind()?;
                self.assert.call(json!({
                    "symbol": symbol,
                    "kind": kind,
                    "scope": params.scope,
                    "budget_tokens": budget,
                }))
            }
            GraphKind::Impact => {
                let symbol = params.require("symbol", params.symbol.as_ref())?;
                self.impact.call(json!({
                    "symbol_name": symbol,
                    "include_tests": params.include_tests.unwrap_or(false),
                    "budget_tokens": budget,
                }))
            }
            GraphKind::Trace => {
                let from = params.require("from", params.from.as_ref())?;
                let to = params.require("to", params.to.as_ref())?;
                self.trace.call(json!({
                    "from": from,
                    "to": to,
                    "max_depth": params.max_depth.unwrap_or(6),
                    "budget_tokens": budget,
                }))
            }
            GraphKind::Diff => self.diff.call(json!({
                "base": params.base.unwrap_or_else(|| "HEAD".to_string()),
                "path": params.path,
                "include": params.include,
                "budget_tokens": budget,
            })),
            GraphKind::Why => {
                let symbol = params.require("symbol", params.symbol.as_ref())?;
                self.why.call(json!({
                    "symbol": symbol,
                    "max_commits": params.max_commits.unwrap_or(10),
                    "budget_tokens": budget,
                }))
            }
        }
    }

    fn describe_call(&self, arguments: &Value) -> String {
        let kind = arguments
            .get("kind")
            .and_then(Value::as_str)
            .unwrap_or("?");
        let target = arguments
            .get("symbol")
            .or_else(|| arguments.get("from"))
            .and_then(Value::as_str)
            .unwrap_or("");
        if target.is_empty() {
            return format!("graph {kind}");
        }
        format!("graph {kind}: {target}")
    }

    fn mode_mask(&self) -> tupi_kernel::ModeMask {
        tupi_kernel::ModeMask::EXECUTION.union(tupi_kernel::ModeMask::SDD)
    }
}
