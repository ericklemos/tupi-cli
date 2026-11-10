use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use serde::Deserialize;
use serde_json::Value;

use tupi_kernel::{Tool, ToolError, ToolSpec};

use crate::budget::Section;
use crate::exec::finish;
use crate::graph::{CallGraph, Def};
use crate::store::ResponseStore;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum MapDepth {
    #[default]
    Files,
    Dirs,
}

#[derive(Debug, Deserialize)]
pub struct MapParams {
    #[serde(default)]
    pub depth: MapDepth,
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub include_tests: bool,
    #[serde(default)]
    pub budget_tokens: Option<usize>,
}

pub struct MapTool {
    root_dir: PathBuf,
    store: Arc<ResponseStore>,
    default_budget_tokens: usize,
}

impl MapTool {
    pub fn new(root_dir: PathBuf, store: Arc<ResponseStore>, default_budget_tokens: usize) -> Self {
        Self {
            root_dir,
            store,
            default_budget_tokens,
        }
    }
}

fn filtered<'a>(graph: &'a CallGraph, prefix: Option<&str>, include_tests: bool) -> Vec<&'a Def> {
    graph
        .defs
        .iter()
        .filter(|def| include_tests || !def.is_test)
        .filter(|def| prefix.is_none_or(|prefix| def.relative.starts_with(prefix)))
        .collect()
}

fn incoming(graph: &CallGraph) -> HashMap<&str, usize> {
    let mut counts: HashMap<&str, usize> = HashMap::new();
    for def in &graph.defs {
        for call in &def.calls {
            *counts.entry(call.as_str()).or_default() += 1;
        }
    }
    counts
}

fn files_sections(defs: &[&Def], incoming: &HashMap<&str, usize>) -> Vec<Section> {
    let mut files: HashMap<&str, Vec<&Def>> = HashMap::new();
    for def in defs {
        files.entry(def.relative.as_str()).or_default().push(def);
    }

    let mut rows: Vec<(&str, Vec<&Def>, usize)> = files
        .into_iter()
        .map(|(path, mut defs)| {
            defs.sort_by_key(|def| def.start_line);
            let score = defs
                .iter()
                .map(|def| incoming.get(def.name.as_str()).copied().unwrap_or(0))
                .sum();
            (path, defs, score)
        })
        .collect();
    rows.sort_by(|left, right| right.2.cmp(&left.2).then(left.0.cmp(right.0)));

    rows.into_iter()
        .enumerate()
        .map(|(index, (path, defs, score))| {
            let names: Vec<&str> = defs.iter().take(10).map(|def| def.name.as_str()).collect();
            let mut body = format!("\n{}\n", names.join(", "));
            if defs.len() > names.len() {
                body.push_str(&format!("(+{} símbolos)\n", defs.len() - names.len()));
            }
            Section {
                heading: format!(
                    "#### {path} — {} símbolo(s) · {score} chamada(s)",
                    defs.len()
                ),
                body,
                priority: (240 - index.min(200)) as u8,
            }
        })
        .collect()
}

fn dirs_sections(defs: &[&Def], incoming: &HashMap<&str, usize>) -> Vec<Section> {
    let mut dirs: HashMap<String, (usize, usize, usize)> = HashMap::new();
    for def in defs {
        let dir = def
            .relative
            .rsplit_once('/')
            .map(|(dir, _)| dir.to_string())
            .unwrap_or_else(|| ".".to_string());
        let score = incoming.get(def.name.as_str()).copied().unwrap_or(0);
        let entry = dirs.entry(dir).or_default();
        entry.0 += 1;
        entry.1 += score;
        entry.2 += 1;
    }

    let mut rows: Vec<(String, (usize, usize, usize))> = dirs.into_iter().collect();
    rows.sort_by(|left, right| right.1.1.cmp(&left.1.1).then(left.0.cmp(&right.0)));

    rows.into_iter()
        .enumerate()
        .map(|(index, (dir, (files, score, _)))| Section {
            heading: format!("#### {dir} — {files} símbolo(s) · {score} chamada(s)"),
            body: String::new(),
            priority: (240 - index.min(200)) as u8,
        })
        .collect()
}

impl Tool for MapTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "map".to_string(),
            description:
                "Mapa do projeto por centralidade: arquivos/símbolos ou resumo por diretório."
                    .to_string(),
            schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "depth": {
                        "type": "string",
                        "enum": ["files", "dirs"],
                        "default": "files",
                        "description": "files: símbolos por arquivo; dirs: um resumo por diretório."
                    },
                    "path": {
                        "type": "string",
                        "description": "Opcional: restringe o mapa a um prefixo de caminho."
                    },
                    "include_tests": {
                        "type": "boolean",
                        "default": false,
                        "description": "Inclui símbolos de teste."
                    },
                    "budget_tokens": {
                        "type": "integer",
                        "minimum": 1,
                        "description": "Teto de tokens (excedente em peek)."
                    }
                }
            }),
        }
    }

    fn call(&self, arguments: Value) -> Result<String, ToolError> {
        let params: MapParams = serde_json::from_value(arguments)
            .map_err(|error| ToolError::InvalidArguments(error.to_string()))?;
        let root = self
            .root_dir
            .canonicalize()
            .unwrap_or_else(|_| self.root_dir.clone());
        let prefix = params
            .path
            .as_deref()
            .map(str::trim)
            .filter(|prefix| !prefix.is_empty());
        if let Some(prefix) = prefix
            && (prefix.starts_with('/') || prefix.split('/').any(|part| part == ".."))
        {
            return Err(ToolError::OutsideRoot(prefix.to_string()));
        }

        let graph = CallGraph::build(&root);
        let incoming = incoming(&graph);
        let defs = filtered(&graph, prefix, params.include_tests);
        let tests = graph
            .defs
            .iter()
            .filter(|def| def.is_test)
            .filter(|def| prefix.is_none_or(|prefix| def.relative.starts_with(prefix)))
            .count();

        let mut sections = Vec::new();
        sections.push(Section {
            heading: format!(
                "### mapa — {} arquivo(s) · {} símbolo(s) ({} teste(s) omitido(s))",
                {
                    let mut files: Vec<&str> =
                        defs.iter().map(|def| def.relative.as_str()).collect();
                    files.sort();
                    files.dedup();
                    files.len()
                },
                defs.len(),
                if params.include_tests { 0 } else { tests }
            ),
            body: String::new(),
            priority: 255,
        });
        sections.extend(match params.depth {
            MapDepth::Files => files_sections(&defs, &incoming),
            MapDepth::Dirs => dirs_sections(&defs, &incoming),
        });

        let budget_tokens = params.budget_tokens.unwrap_or(self.default_budget_tokens);
        Ok(finish(&self.store, sections, budget_tokens))
    }

    fn describe_call(&self, arguments: &Value) -> String {
        let depth = arguments
            .get("depth")
            .and_then(Value::as_str)
            .unwrap_or("files");
        let path = arguments.get("path").and_then(Value::as_str).unwrap_or(".");
        format!("map: {path} ({depth})")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(dir: &std::path::Path, relative: &str, content: &str) {
        let path = dir.join(relative);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, content).unwrap();
    }

    fn tool(dir: &std::path::Path) -> MapTool {
        MapTool::new(
            dir.to_path_buf(),
            Arc::new(ResponseStore::new(
                std::time::Duration::from_secs(60),
                1 << 20,
            )),
            4000,
        )
    }

    #[test]
    fn a_file_map_lists_symbols_by_path() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "src/a.rs", "pub fn helper() {}\n");
        write(dir.path(), "src/b.rs", "pub fn caller() { helper(); }\n");
        let out = Tool::call(&tool(dir.path()), serde_json::json!({})).unwrap();
        assert!(out.contains("src/a.rs"));
        assert!(out.contains("src/b.rs"));
        assert!(out.contains("helper"));
    }

    #[test]
    fn a_dir_map_has_no_symbol_names() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "src/a.rs", "pub fn helper() {}\n");
        let out = Tool::call(&tool(dir.path()), serde_json::json!({"depth": "dirs"})).unwrap();
        assert!(out.contains("src"));
        assert!(!out.contains("helper"));
    }

    #[test]
    fn tests_are_counted_not_listed_by_default() {
        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            "src/a.rs",
            "pub fn helper() {}\n#[test]\nfn it_works() {}\n",
        );
        let out = Tool::call(&tool(dir.path()), serde_json::json!({})).unwrap();
        assert!(out.contains("1 teste(s) omitido(s)"));
        assert!(!out.contains("it_works"));
    }
}
