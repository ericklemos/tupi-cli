//! Execução por `kind`: cada op reusa o motor que já existe —
//! `ReadTool::prepare` + `render` para leitura e `SearchTool` (via `Tool::call`)
//! para busca textual e densa. O lote inteiro passa por **um** orçamento em
//! [`Engines::run`]; item com problema vira seção de topo (priority 255) sem
//! derrubar o que deu certo.

use std::collections::BTreeMap;

use tupi_kernel::{Tool, ToolError};

use super::params::{LookupOp, LookupParams, OpKind};
use crate::support::budget::Section;
use crate::support::dense::{Item, Track};
use crate::support::ledger::{self, DeliveryLedger};
use crate::support::scanner::LexicalScanner;
use crate::support::store::ResponseStore;
use crate::tools::read::{ReadParams, ReadTool};
use crate::tools::search::SearchTool;

/// Prioridade das seções que deram certo: a ordem pedida vence o empate (o
/// `sort_by` da fusão é estável) e 255 fica reservado para a recusa.
const BASE_PRIORITY: u8 = 240;
const REFUSAL_PRIORITY: u8 = 255;
/// Piso do quinhão de cada op: uma op nunca recebe zero, senão o cap por item
/// vira corte seco sem corpo nenhum.
const MIN_SHARE: usize = 1;

/// Os dois motores que `lookup` funde. Emprestados, porque quem monta as tools
/// (registro) é outro; aqui só se executa.
pub(crate) struct Engines<'a> {
    read: &'a ReadTool,
    search: &'a SearchTool,
    root_dir: &'a std::path::Path,
}

impl<'a> Engines<'a> {
    pub(crate) fn new(
        read: &'a ReadTool,
        search: &'a SearchTool,
        root_dir: &'a std::path::Path,
    ) -> Self {
        Self {
            read,
            search,
            root_dir,
        }
    }

    /// Um orçamento para o lote inteiro: cada op recebe um quinhão para o seu
    /// cap por item e a seleção final é feita uma única vez, com handle do
    /// `ResponseStore` para o que não coube (`peek` recupera).
    pub(crate) fn run(
        &self,
        params: &LookupParams,
        default_budget_tokens: usize,
        ledger: &DeliveryLedger,
        store: &ResponseStore,
    ) -> String {
        let budget_tokens = params.budget_tokens.unwrap_or(default_budget_tokens).max(MIN_SHARE);
        let share = (budget_tokens / params.ops.len().max(1)).max(MIN_SHARE);
        let repeat = params.repeat.as_deref();

        let mut items: Vec<Item> = Vec::with_capacity(params.ops.len());
        let mut refusals = 0usize;
        for (position, op) in params.ops.iter().enumerate() {
            let priority = BASE_PRIORITY.saturating_sub(position.min(200) as u8);
            match self.items_for(op, priority, share, repeat) {
                Ok(mut produced) => items.append(&mut produced),
                Err(error) => {
                    // Erro é por item: a recusa vira seção de topo e o que deu
                    // certo continua na saída.
                    refusals += 1;
                    items.push(Item {
                        section: refusal(position, op.kind(), &error),
                        track: None,
                    });
                }
            }
        }

        super::render::output(
            items,
            budget_tokens,
            refusals,
            self.search.backtrack.frontier_count(),
            ledger,
            store,
        )
    }

    fn items_for(
        &self,
        op: &LookupOp,
        priority: u8,
        share: usize,
        repeat: Option<&str>,
    ) -> Result<Vec<Item>, ToolError> {
        match op {
            LookupOp::Symbol { .. } | LookupOp::Skeleton { .. } => {
                self.read_op(op, priority, repeat)
            }
            LookupOp::Lines {
                path,
                start,
                end,
                tail,
                head,
                rev,
            } => self.lines_op(
                path,
                *start,
                *end,
                *tail,
                *head,
                rev.as_deref(),
                priority,
                repeat,
            ),
            LookupOp::Text {
                text,
                alternatives,
                path,
                glob,
                exclude,
                regex,
                max_results,
            } => self.text_op(
                text,
                alternatives,
                path.as_deref(),
                glob.as_deref(),
                exclude.as_deref(),
                *regex,
                *max_results,
                priority,
            ),
            LookupOp::Semantic {
                query,
                keywords,
                path,
                detail,
                max_results,
            } => {
                let body = self.search.call(serde_json::json!({
                    "query": query,
                    "keywords": keywords,
                    "path": path,
                    "detail": detail.map(|detail| detail.as_str()),
                    "max_results": max_results,
                    "budget_tokens": share,
                    "repeat": repeat,
                }))?;
                Ok(vec![plain(
                    format!("semântica: {}", query.trim()),
                    body,
                    priority,
                )])
            }
            LookupOp::Graph {
                symbol,
                path,
                depth,
            } => super::graph::sections(
                self.root_dir,
                Some(symbol.as_str()),
                path.as_deref(),
                *depth,
                priority,
            ),
        }
    }

    fn read_op(
        &self,
        op: &LookupOp,
        priority: u8,
        repeat: Option<&str>,
    ) -> Result<Vec<Item>, ToolError> {
        let (path, symbol, rev) = match op {
            LookupOp::Symbol { path, symbol, rev } => {
                (path.clone(), Some(symbol.clone()), rev.clone())
            }
            LookupOp::Skeleton { path, rev } => (path.clone(), None, rev.clone()),
            _ => {
                return Err(ToolError::InvalidArguments(
                    "op de leitura esperada".to_string(),
                ));
            }
        };
        let read = ReadParams {
            path,
            symbol: symbol.clone(),
            budget_tokens: None,
            repeat: repeat.map(str::to_string),
            rev: rev.clone(),
        };

        if let Some(reference) = rev.as_deref().map(str::trim).filter(|rev| !rev.is_empty()) {
            let (_reference, sections) = self.read.rev_sections(&read, reference)?;
            return Ok(sections
                .into_iter()
                .map(|section| Item {
                    section: with_priority(section, priority),
                    track: None,
                })
                .collect());
        }

        let (full_path, content, relative) = self.read.prepare(&read)?;
        let rendered = self.read.render(&read, &full_path, &content, &relative)?;
        if rendered.is_empty() {
            return Err(ToolError::SymbolNotFound(format!(
                "símbolo `{}` não encontrado em `{relative}`",
                symbol.as_deref().unwrap_or_default()
            )));
        }

        let file_hash = ledger::content_hash(&content);
        let id = symbol.unwrap_or_else(|| relative.clone());
        let repeat_allowed = repeat == Some("allow");
        Ok(rendered
            .into_iter()
            .map(|rendered| {
                let text = rendered.section.body.clone();
                Item {
                    section: with_priority(rendered.section, priority),
                    track: Some(Track {
                        tool: "read",
                        path: full_path.clone(),
                        id: id.clone(),
                        file_hash,
                        text,
                        complete: true,
                        start_line: 0,
                        repeat_allowed,
                    }),
                }
            })
            .collect())
    }

        #[allow(clippy::too_many_arguments)]
    fn lines_op(
        &self,
        path: &str,
        start: Option<usize>,
        end: Option<usize>,
        tail: Option<usize>,
        head: Option<usize>,
        rev: Option<&str>,
        priority: u8,
        repeat: Option<&str>,
    ) -> Result<Vec<Item>, ToolError> {
        let read = ReadParams {
            path: path.to_string(),
            symbol: None,
            budget_tokens: None,
            repeat: repeat.map(str::to_string),
            rev: rev.map(str::to_string),
        };
        let (full_path, content, relative) =
            match rev.map(str::trim).filter(|rev| !rev.is_empty()) {
                Some(reference) => self.read.prepare_rev(&read, reference)?,
                None => self.read.prepare(&read)?,
            };

        let total = content.lines().count();
        let last = total.max(1);
        // `head`/`tail` são atalhos da mesma faixa: um deles, sozinho, decide as
        // pontas; com `start`/`end` a faixa é explícita.
        let (first_line, end_line) = if let Some(head) = head.filter(|count| *count > 0) {
            (1, head.min(last))
        } else if let Some(tail) = tail.filter(|count| *count > 0) {
            (last.saturating_sub(tail).saturating_add(1).max(1), last)
        } else {
            let first_line = start.unwrap_or(1).clamp(1, last);
            let end_line = end
                .unwrap_or(first_line.saturating_add(199))
                .clamp(first_line, last);
            (first_line, end_line)
        };
        let body: String = content
            .lines()
            .enumerate()
            .skip(first_line - 1)
            .take(end_line - first_line + 1)
            .map(|(index, line)| format!("{:>5}  {line}", index + 1))
            .collect::<Vec<_>>()
            .join("\n");
        let body = format!("```\n{body}\n```");

        Ok(vec![Item {
            section: Section {
                heading: format!("{relative}:{first_line}-{end_line} (de {total} linhas)"),
                body: body.clone(),
                priority,
            },
            track: Some(Track {
                tool: "lookup",
                path: full_path,
                id: format!("{relative}:{first_line}-{end_line}"),
                file_hash: ledger::content_hash(&content),
                text: body,
                complete: end_line >= total,
                start_line: first_line,
                repeat_allowed: repeat == Some("allow"),
            }),
        }])
    }
    /// `kind: text` — literal exato agrupado por arquivo, no formato
    /// `caminho:linha:texto`, com total e corte anunciados.
    #[allow(clippy::too_many_arguments)]
    fn text_op(
        &self,
        text: &str,
        alternatives: &[String],
        path: Option<&str>,
        glob: Option<&str>,
        exclude: Option<&str>,
        regex: bool,
        max_results: Option<usize>,
        priority: u8,
    ) -> Result<Vec<Item>, ToolError> {
        if regex {
            return Err(ToolError::InvalidArguments(
                "`regex: true` não existe: `text` casa literal exato — use `alternatives` \
                 (lista de literais) ou `kind: semantic` para linguagem natural."
                    .to_string(),
            ));
        }
        let mut terms = vec![text.to_string()];
        terms.extend(alternatives.iter().cloned());
        terms.retain(|term| !term.is_empty());
        let scanner = LexicalScanner::new(self.search.root_dir.clone());
        let matches = scanner.search(&terms, path)?;
        let total = matches.len();
        let mut by_file: BTreeMap<String, Vec<(usize, String)>> = BTreeMap::new();
        let mut filtered = 0usize;
        for hit in matches {
            let relative = hit
                .file_path
                .strip_prefix(&self.search.root_dir)
                .map(|path| path.to_string_lossy().replace('\\', "/"))
                .unwrap_or_else(|_| hit.file_path.display().to_string());
            if let Some(pattern) = exclude
                && glob_matches(pattern, &relative)
            {
                filtered += 1;
                continue;
            }
            if let Some(pattern) = glob
                && !glob_matches(pattern, &relative)
            {
                filtered += 1;
                continue;
            }
            by_file
                .entry(relative)
                .or_default()
                .push((hit.line_number, hit.line_content.trim_end().to_string()));
        }
        let cap = max_results.unwrap_or(80).max(1);
        let mut body = String::new();
        let mut emitted = 0usize;
        for (relative, hits) in &by_file {
            if emitted >= cap {
                break;
            }
            body.push_str(relative);
            body.push('\n');
            for (line, content) in hits {
                if emitted >= cap {
                    break;
                }
                body.push_str(&format!("  {relative}:{line}: {content}\n"));
                emitted += 1;
            }
        }
        let kept = total.saturating_sub(filtered);
        if emitted < kept {
            body.push_str(&format!(
                "\n> {emitted} de {kept} casamentos; o resto sai por `peek` ou estreitando `path`/`glob`.\n"
            ));
        }
        if body.is_empty() {
            body.push_str("nenhum casamento literal.\n");
        }
        Ok(vec![plain(
            format!("texto: {}", terms.join(" | ")),
            body,
            priority,
        )])
    }
}

/// Casamento de globo mínimo: `*` casa qualquer trecho e `?` um caractere;
/// suficiente para os filtros `glob`/`exclude` da varredura textual.
fn glob_matches(pattern: &str, candidate: &str) -> bool {
    let pattern: Vec<char> = pattern.chars().collect();
    let candidate: Vec<char> = candidate.chars().collect();
    let (mut pi, mut ci) = (0usize, 0usize);
    let mut star: Option<usize> = None;
    let mut mark = 0usize;
    while ci < candidate.len() {
        if pi < pattern.len() && (pattern[pi] == '?' || pattern[pi] == candidate[ci]) {
            pi += 1;
            ci += 1;
        } else if pi < pattern.len() && pattern[pi] == '*' {
            star = Some(pi);
            mark = ci;
            pi += 1;
        } else if let Some(at) = star {
            pi = at + 1;
            mark += 1;
            ci = mark;
        } else {
            return false;
        }
    }
    while pi < pattern.len() && pattern[pi] == '*' {
        pi += 1;
    }
    pi == pattern.len()
}

fn plain(heading: String, body: String, priority: u8) -> Item {
    Item {
        section: Section {
            heading,
            body,
            priority,
        },
        track: None,
    }
}

fn with_priority(mut section: Section, priority: u8) -> Section {
    section.priority = priority;
    section
}

/// Erro é por item: vira seção de topo e nomeia a op que falhou, sem esconder
/// as seções que deram certo.
fn refusal(position: usize, kind: OpKind, error: &ToolError) -> Section {
    Section {
        heading: format!("lookup op {position} ({}) recusada", kind.as_str()),
        body: format!("{error}\n\n> o item com erro não derruba o lote; as demais seções seguem abaixo."),
        priority: REFUSAL_PRIORITY,
    }
}
