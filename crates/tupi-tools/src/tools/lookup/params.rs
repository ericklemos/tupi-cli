//! Params, schema e parse por `kind` da tool `lookup`.
//!
//! O lote é heterogêneo: cada item declara `kind` (`symbol`, `skeleton`,
//! `lines`, `text`, `semantic`, `graph`) e os campos que aquele `kind` exige.
//! Faltou o campo obrigatório? A recusa nomeia o campo, o item e a alternativa.
//! `mode` é atalho de op única: o valor é o `kind` e os campos da op vêm na
//! raiz.

use serde::de::Error as _;
use serde::{Deserialize, Deserializer};
use serde_json::{json, Value};

/// Nome da tool; o registro usa esta constante para não divergir do schema.
pub const NAME: &str = "lookup";

/// Descrição exposta ao modelo (uma linha densa, ordem de prioridade).
pub const DESCRIPTION: &str = "Um lote heterogêneo de operações num único orçamento: `kind` escolhe symbol (corpo do símbolo), skeleton (esqueleto do arquivo), lines (faixa de linhas), text (literal exato), semantic (busca densa) ou graph (quem chama). Item com problema vira erro no topo e não derruba o resto; alvo já entregue nesta sessão aponta para o corpo anterior (`repeat: allow` força o corpo).";

/// Os `kind` aceitos, na ordem em que o schema os enumera.
pub const OP_KINDS: [&str; 6] = [
    "symbol",
    "skeleton",
    "lines",
    "text",
    "semantic",
    "graph",
];

/// O que um item do lote pede.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpKind {
    /// Corpo de um símbolo (`path` + `symbol`).
    Symbol,
    /// Esqueleto do arquivo (`path`).
    Skeleton,
    /// Faixa de linhas (`path`, `start`/`end` opcionais).
    Lines,
    /// Busca textual literal (`text`).
    Text,
    /// Busca densa por descrição (`query`).
    Semantic,
    /// Quem chama e o que o símbolo consome (`symbol`).
    Graph,
}

impl OpKind {
    pub fn as_str(self) -> &'static str {
        match self {
            OpKind::Symbol => "symbol",
            OpKind::Skeleton => "skeleton",
            OpKind::Lines => "lines",
            OpKind::Text => "text",
            OpKind::Semantic => "semantic",
            OpKind::Graph => "graph",
        }
    }

    /// Lista ensinada em toda recusa: `symbol|skeleton|lines|text|semantic|graph`.
    pub fn list() -> String {
        OP_KINDS.join("|")
    }

    /// Aceita o valor minúsculo do schema e a forma capitalizada por hábito.
    pub fn parse(text: &str) -> Option<Self> {
        match text.trim().to_ascii_lowercase().as_str() {
            "symbol" => Some(OpKind::Symbol),
            "skeleton" => Some(OpKind::Skeleton),
            "lines" => Some(OpKind::Lines),
            "text" => Some(OpKind::Text),
            "semantic" => Some(OpKind::Semantic),
            "graph" => Some(OpKind::Graph),
            _ => None,
        }
    }

    /// Campo que a recusa deve nomear primeiro quando ele falta.
    fn missing_hint(self) -> &'static str {
        match self {
            OpKind::Symbol => {
                "o corpo exige `path` e `symbol`; para o arquivo inteiro use `kind: \"skeleton\"`"
            }
            OpKind::Skeleton => {
                "`skeleton` exige `path`; para o corpo de um símbolo use `kind: \"symbol\"`"
            }
            OpKind::Lines => {
                "`lines` exige `path`; `start`/`end` recortam a faixa (padrão: início do arquivo)"
            }
            OpKind::Text => {
                "`text` exige o literal exato procurado; para descrição em linguagem natural use `kind: \"semantic\"`"
            }
            OpKind::Semantic => {
                "`semantic` exige `query` (descrição em linguagem natural); para literal exato use `kind: \"text\"`"
            }
            OpKind::Graph => {
                "`graph` exige `symbol` — ou só `path`, que usa as defs do arquivo/prefixo como raízes; para o corpo do símbolo use `kind: \"symbol\"`"
            }
        }
    }
}

/// Detalhe da op `semantic`: espelha `search.detail` (nenhum modo é rebaixado).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum LookupDetail {
    #[default]
    Auto,
    Full,
    Signatures,
}

impl LookupDetail {
    pub fn as_str(self) -> &'static str {
        match self {
            LookupDetail::Auto => "auto",
            LookupDetail::Full => "full",
            LookupDetail::Signatures => "signatures",
        }
    }

    fn parse(text: &str) -> Option<Self> {
        match text.trim().to_ascii_lowercase().as_str() {
            "auto" => Some(LookupDetail::Auto),
            "full" => Some(LookupDetail::Full),
            "signatures" => Some(LookupDetail::Signatures),
            _ => None,
        }
    }
}

/// Uma op já validada: o `kind` decide quais campos existem.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LookupOp {
    Symbol {
        path: String,
        symbol: String,
        rev: Option<String>,
    },
    Skeleton {
        path: String,
        rev: Option<String>,
    },
    Lines {
        path: String,
        start: Option<usize>,
        end: Option<usize>,
        tail: Option<usize>,
        head: Option<usize>,
        rev: Option<String>,
    },
    Text {
        text: String,
        alternatives: Vec<String>,
        path: Option<String>,
        glob: Option<String>,
        exclude: Option<String>,
        regex: bool,
        max_results: Option<usize>,
    },
    Semantic {
        query: String,
        keywords: Vec<String>,
        path: Option<String>,
        detail: Option<LookupDetail>,
        max_results: Option<usize>,
    },
    /// `symbol` é o alvo; `path` sozinho usa as defs do arquivo/prefixo como
    /// raízes. Nenhum dos dois? o parse recusa.
    Graph {
        symbol: Option<String>,
        path: Option<String>,
        depth: Option<usize>,
    },
}

impl LookupOp {
    pub fn kind(&self) -> OpKind {
        match self {
            LookupOp::Symbol { .. } => OpKind::Symbol,
            LookupOp::Skeleton { .. } => OpKind::Skeleton,
            LookupOp::Lines { .. } => OpKind::Lines,
            LookupOp::Text { .. } => OpKind::Text,
            LookupOp::Semantic { .. } => OpKind::Semantic,
            LookupOp::Graph { .. } => OpKind::Graph,
        }
    }
}

/// Um lote de ops com um único orçamento e um único ledger.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LookupParams {
    pub ops: Vec<LookupOp>,
    pub budget_tokens: Option<usize>,
    pub repeat: Option<String>,
}

impl LookupParams {
    /// Primeira op do lote; o atalho `mode` garante que sempre há uma.
    pub fn single(&self) -> &LookupOp {
        &self.ops[0]
    }
}

/// Item cru do lote: todos os campos opcionais, validados por `kind` no parse.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct RawOp {
    kind: Option<String>,
    path: Option<String>,
    symbol: Option<String>,
    rev: Option<String>,
    start: Option<usize>,
    end: Option<usize>,
    tail: Option<usize>,
    head: Option<usize>,
    text: Option<String>,
    alternatives: Vec<String>,
    glob: Option<String>,
    exclude: Option<String>,
    regex: Option<bool>,
    query: Option<String>,
    keywords: Vec<String>,
    detail: Option<String>,
    max_results: Option<usize>,
    depth: Option<usize>,
}

/// Chamada crua: `ops`, o atalho `mode` com os campos na raiz, e o orçamento.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct RawLookup {
    ops: Vec<RawOp>,
    mode: Option<String>,
    path: Option<String>,
    symbol: Option<String>,
    rev: Option<String>,
    start: Option<usize>,
    end: Option<usize>,
    tail: Option<usize>,
    head: Option<usize>,
    text: Option<String>,
    alternatives: Vec<String>,
    glob: Option<String>,
    exclude: Option<String>,
    regex: Option<bool>,
    query: Option<String>,
    keywords: Vec<String>,
    detail: Option<String>,
    max_results: Option<usize>,
    depth: Option<usize>,
    budget_tokens: Option<usize>,
    repeat: Option<String>,
}

impl<'de> Deserialize<'de> for LookupParams {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        RawLookup::deserialize(deserializer)?
            .resolve()
            .map_err(D::Error::custom)
    }
}

impl RawLookup {
    fn resolve(self) -> Result<LookupParams, String> {
        let ops = match self.mode {
            Some(mode) => {
                if !self.ops.is_empty() {
                    return Err(format!(
                        "lookup: `mode` é atalho de op única — mande `ops: [{{kind, ...}}]` ou `mode` com os campos da op na raiz, nunca os dois (kinds: {})",
                        OpKind::list()
                    ));
                }
                let kind = OpKind::parse(&mode).ok_or_else(|| unknown_kind(1, &mode))?;
                vec![resolve_op(
                    RawOp {
                        kind: Some(kind.as_str().to_string()),
                        path: self.path,
                        symbol: self.symbol,
                        rev: self.rev,
                        start: self.start,
                        end: self.end,
                        tail: self.tail,
                        head: self.head,
                        text: self.text,
                        alternatives: self.alternatives,
                        glob: self.glob,
                        exclude: self.exclude,
                        regex: self.regex,
                        query: self.query,
                        keywords: self.keywords,
                        detail: self.detail,
                        max_results: self.max_results,
                        depth: self.depth,
                    },
                    1,
                )?]
            }
            None => {
                if self.ops.is_empty() {
                    return Err(format!(
                        "lookup: lote vazio — mande `ops: [{{kind, ...}}]` (kinds: {}), ou `mode` + os campos da op única",
                        OpKind::list()
                    ));
                }
                let mut ops = Vec::with_capacity(self.ops.len());
                for (index, raw) in self.ops.into_iter().enumerate() {
                    ops.push(resolve_op(raw, index + 1)?);
                }
                ops
            }
        };
        Ok(LookupParams {
            ops,
            budget_tokens: self.budget_tokens,
            repeat: self.repeat,
        })
    }
}

fn unknown_kind(position: usize, text: &str) -> String {
    format!(
        "lookup: op {position} tem `kind: {text:?}` desconhecido — use um destes: {}",
        OpKind::list()
    )
}

fn missing_field(position: usize, kind: OpKind, field: &str) -> String {
    format!(
        "lookup: op {position} ({}) exige o campo `{field}` — {}",
        kind.as_str(),
        kind.missing_hint()
    )
}

fn resolve_op(raw: RawOp, position: usize) -> Result<LookupOp, String> {
    let kind = match raw.kind.as_deref() {
        Some(text) => OpKind::parse(text).ok_or_else(|| unknown_kind(position, text))?,
        None => {
            return Err(format!(
                "lookup: op {position} sem `kind` — use um destes: {}",
                OpKind::list()
            ))
        }
    };
    let require = |field: &str, value: Option<String>| {
        value.ok_or_else(|| missing_field(position, kind, field))
    };
    let detail = match raw.detail.as_deref() {
        Some(text) => Some(LookupDetail::parse(text).ok_or_else(|| {
            format!(
                "lookup: op {position} (semantic) tem `detail: {text:?}` desconhecido — use auto|full|signatures"
            )
        })?),
        None => None,
    };
    let op = match kind {
        OpKind::Symbol => LookupOp::Symbol {
            path: require("path", raw.path)?,
            symbol: require("symbol", raw.symbol)?,
            rev: raw.rev,
        },
        OpKind::Skeleton => LookupOp::Skeleton {
            path: require("path", raw.path)?,
            rev: raw.rev,
        },
        OpKind::Lines => LookupOp::Lines {
            path: require("path", raw.path)?,
            start: raw.start,
            end: raw.end,
            tail: raw.tail,
            head: raw.head,
            rev: raw.rev,
        },
        OpKind::Text => LookupOp::Text {
            text: require("text", raw.text)?,
            alternatives: raw.alternatives,
            path: raw.path,
            glob: raw.glob,
            exclude: raw.exclude,
            regex: raw.regex.unwrap_or(false),
            max_results: raw.max_results,
        },
        OpKind::Semantic => LookupOp::Semantic {
            query: require("query", raw.query)?,
            keywords: raw.keywords,
            path: raw.path,
            detail,
            max_results: raw.max_results,
        },
        OpKind::Graph => {
            // `symbol` é o alvo; `path` sozinho aponta o arquivo/prefixo cujas
            // defs viram raízes (o que a antiga `context_graph` chamava de
            // target de arquivo).
            let symbol = raw
                .symbol
                .map(|symbol| symbol.trim().to_string())
                .filter(|symbol| !symbol.is_empty());
            if symbol.is_none() && raw.path.is_none() {
                return Err(missing_field(position, kind, "symbol"));
            }
            LookupOp::Graph {
                symbol,
                path: raw.path,
                depth: raw.depth,
            }
        }
    };
    Ok(op)
}

/// Schema da tool: um objeto com `ops` (ou `mode`) e um só orçamento.
pub fn schema() -> Value {
        json!({
            "type": "object",
            "properties": {
                "ops": {
                    "type": "array",
                    "description": "Campos por kind: symbol path+symbol, skeleton path, lines path (+start/end ou tail/head), text text (+glob/exclude/alternatives), semantic query, graph symbol (ou só path).",
                    "items": {
                        "type": "object",
                        "properties": {
                            "kind": { "type": "string", "enum": OP_KINDS },
                            "path": { "type": "string", "description": "Relativo ao root; em text/semantic é prefixo e, em graph sem `symbol`, é o alvo-arquivo." },
                            "symbol": { "type": "string", "description": "Simples ou qualificado (Foo::new)." },
                            "rev": { "type": "string" },
                            "start": { "type": "integer" },
                            "end": { "type": "integer" },
                            "tail": { "type": "integer", "description": "lines: últimas N." },
                            "head": { "type": "integer", "description": "lines: primeiras N." },
                            "text": { "type": "string" },
                            "alternatives": { "type": "array", "items": { "type": "string" } },
                            "glob": { "type": "string", "description": "text: mantém estes caminhos." },
                            "exclude": { "type": "string", "description": "text: descarta estes." },
                            "regex": { "type": "boolean", "description": "Recusado: use alternatives." },
                            "query": { "type": "string", "description": "semantic: linguagem natural." },
                            "keywords": { "type": "array", "items": { "type": "string" } },
                            "detail": { "type": "string", "enum": ["auto", "full", "signatures"] },
                            "max_results": { "type": "integer" },
                            "depth": { "type": "integer", "description": "graph: saltos no grafo (1..3)." }
                        },
                        "required": ["kind"]
                    }
                },
                "mode": {
                    "type": "string",
                    "enum": OP_KINDS,
                    "description": "Atalho de op única: o kind aqui e os campos da op na raiz (path, symbol, text, query…). Não combine com ops."
                },
                "budget_tokens": { "type": "integer" },
                "repeat": { "type": "string", "enum": ["allow"] }
            }
        })
    }

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(value: Value) -> Result<LookupParams, String> {
        serde_json::from_value::<LookupParams>(value).map_err(|error| error.to_string())
    }

    fn error(value: Value) -> String {
        parse(value).expect_err("a chamada deveria ser recusada")
    }

    #[test]
    fn an_op_without_its_required_field_is_refused_naming_the_field() {
        let message = error(json!({ "ops": [{ "kind": "symbol", "path": "a.rs" }] }));
        assert!(message.contains("op 1"), "{message}");
        assert!(message.contains("(symbol)"), "{message}");
        assert!(message.contains("`symbol`"), "{message}");

        let message = error(json!({ "ops": [{ "kind": "skeleton" }] }));
        assert!(message.contains("exige o campo `path`"), "{message}");

        let message = error(json!({ "ops": [{ "kind": "text" }] }));
        assert!(message.contains("exige o campo `text`"), "{message}");

        let message = error(json!({ "ops": [{ "kind": "semantic" }] }));
        assert!(message.contains("exige o campo `query`"), "{message}");

        let message = error(json!({ "ops": [{ "kind": "graph" }] }));
        assert!(message.contains("exige o campo `symbol`"), "{message}");
    }

    #[test]
    fn the_item_position_names_the_op_that_failed() {
        let message = error(json!({
            "ops": [
                { "kind": "skeleton", "path": "a.rs" },
                { "kind": "text" }
            ]
        }));
        assert!(message.contains("op 2"), "{message}");
    }

    #[test]
    fn a_missing_kind_lists_the_valid_ones() {
        let message = error(json!({ "ops": [{ "path": "a.rs" }] }));
        assert!(message.contains("sem `kind`"), "{message}");
        assert!(message.contains("symbol|skeleton|lines|text|semantic|graph"), "{message}");
    }

    #[test]
    fn an_unknown_kind_is_refused_with_the_list() {
        let message = error(json!({ "ops": [{ "kind": "grep", "text": "x" }] }));
        assert!(message.contains("desconhecido"), "{message}");
        assert!(message.contains("semantic|graph"), "{message}");
    }

    #[test]
    fn mode_builds_a_single_equivalent_op() {
        let params = parse(json!({ "mode": "semantic", "query": "x", "budget_tokens": 900 }))
            .expect("mode deveria gerar uma op");
        assert_eq!(
            params.ops,
            vec![LookupOp::Semantic {
                query: "x".to_string(),
                keywords: Vec::new(),
                path: None,
                detail: None,
                max_results: None,
            }]
        );
        assert_eq!(params.budget_tokens, Some(900));
    }

    #[test]
    fn mode_is_the_kind_and_its_fields_are_checked() {
        let params = parse(json!({ "mode": "symbol", "path": "a.rs", "symbol": "new" }))
            .expect("mode com os campos da op deveria passar");
        assert_eq!(
            params.single(),
            &LookupOp::Symbol {
                path: "a.rs".to_string(),
                symbol: "new".to_string(),
                rev: None,
            }
        );

        let message = error(json!({ "mode": "symbol", "path": "a.rs" }));
        assert!(message.contains("op 1 (symbol)"), "{message}");
        assert!(message.contains("`symbol`"), "{message}");
    }

    #[test]
    fn mode_and_ops_together_are_refused() {
        let message = error(json!({
            "mode": "skeleton",
            "ops": [{ "kind": "skeleton", "path": "a.rs" }]
        }));
        assert!(message.contains("atalho de op única"), "{message}");
    }

    #[test]
    fn an_empty_batch_teaches_the_kinds() {
        let message = error(json!({}));
        assert!(message.contains("lote vazio"), "{message}");
        assert!(message.contains("semantic|graph"), "{message}");
    }

    #[test]
    fn every_kind_parses_its_required_fields() {
        let params = parse(json!({
            "ops": [
                { "kind": "symbol", "path": "a.rs", "symbol": "new" },
                { "kind": "skeleton", "path": "a.rs" },
                { "kind": "lines", "path": "a.rs", "start": 2, "end": 9 },
                { "kind": "text", "text": "lookup", "path": "crates" },
                { "kind": "semantic", "query": "busca", "keywords": ["lookup"] },
                { "kind": "graph", "symbol": "new", "depth": 2 }
            ],
            "budget_tokens": 2000,
            "repeat": "allow"
        }))
        .expect("cada kind deveria aceitar seus campos");
        let kinds: Vec<OpKind> = params.ops.iter().map(LookupOp::kind).collect();
        assert_eq!(
            kinds,
            vec![
                OpKind::Symbol,
                OpKind::Skeleton,
                OpKind::Lines,
                OpKind::Text,
                OpKind::Semantic,
                OpKind::Graph,
            ]
        );
        assert_eq!(params.repeat.as_deref(), Some("allow"));
    }

    #[test]
    fn detail_keeps_the_search_modes() {
        let params = parse(json!({
            "ops": [{ "kind": "semantic", "query": "x", "detail": "Signatures" }]
        }))
        .expect("detail capitalizado deveria passar");
        match params.single() {
            LookupOp::Semantic { detail, .. } => {
                assert_eq!(*detail, Some(LookupDetail::Signatures))
            }
            other => panic!("esperava semantic, veio {other:?}"),
        }

        let message = error(json!({
            "ops": [{ "kind": "semantic", "query": "x", "detail": "signature" }]
        }));
        assert!(message.contains("auto|full|signatures"), "{message}");
    }

    #[test]
    fn the_schema_lists_every_kind_and_requires_it_per_op() {
        let schema = schema();
        let kinds = schema["properties"]["ops"]["items"]["properties"]["kind"]["enum"]
            .as_array()
            .expect("enum de kind");
        let listed: Vec<&str> = kinds.iter().filter_map(Value::as_str).collect();
        assert_eq!(listed, OP_KINDS.to_vec());
        assert_eq!(
            schema["properties"]["ops"]["items"]["required"],
            json!(["kind"])
        );
        assert_eq!(
            schema["properties"]["mode"]["enum"],
            json!(OP_KINDS)
        );
        assert!(schema["properties"]["budget_tokens"].is_object());
    }
}
