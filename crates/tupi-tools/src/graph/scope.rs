//! Resolução de nomes para as arestas de chamada.
//!
//! Ligar chamada por nome solto transforma cada `new()` em chamador de todos os
//! `new` do repositório. A resolução precisa saber onde a chamada foi escrita e
//! o que aquele arquivo trouxe para o escopo.
//!
//! A ordem é a do Rust:
//!
//! 1. definição no mesmo arquivo;
//! 2. definição alcançável por `use` explícito ou glob;
//! 3. definição exposta pelo caminho da própria chamada;
//! 4. definição no mesmo crate;
//! 5. qualquer lugar, só quando o nome é inequívoco.
//!
//! A regra 5 é o último recurso: nome ambíguo sem evidência local vira
//! `Ambiguous`, não vira aresta.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use petgraph::stable_graph::NodeIndex;

use crate::graph::model::ProjectCodeGraph;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImportTarget {
    File(PathBuf),
    External,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportBinding {
    pub target: ImportTarget,
    /// Nome original no arquivo importado, antes de `as`.
    pub item: String,
}

#[derive(Debug, Default, Clone)]
pub struct FileScope {
    pub imports: HashMap<String, ImportBinding>,
    pub globs: Vec<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UseItem {
    Named { name: String, path: String },
    Glob { path: String },
}

#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportDecl {
    pub owner: PathBuf,
    pub module: String,
    pub item: UseItem,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Resolution {
    Resolved(NodeIndex),
    Ambiguous { candidates: usize },
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallSite {
    pub name: String,
    pub line: usize,
    pub path: Option<String>,
    pub receiver: Option<String>,
}

#[derive(Debug, Default, Clone)]
pub struct ScopeResolver {
    module_files: HashMap<String, PathBuf>,
    file_modules: HashMap<PathBuf, String>,
    crate_roots: HashMap<String, String>,
    crate_prefixes: HashMap<String, String>,
    file_crates: HashMap<PathBuf, String>,
    file_prefixes: HashMap<PathBuf, String>,
}

impl ScopeResolver {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register_file(&mut self, relative: &Path, crate_name: &str) {
        let module = module_path_of(relative);
        if module.is_empty() {
            return;
        }

        self.module_files
            .insert(module.clone(), relative.to_path_buf());
        self.file_modules
            .insert(relative.to_path_buf(), module.clone());
        self.file_crates
            .insert(relative.to_path_buf(), crate_name.to_string());
        let prefix = if let Some((head, _)) = module.split_once("/src") {
            format!("{head}/src/")
        } else if module.starts_with("src/") {
            "src/".to_string()
        } else {
            // Kotlin sem `src/`: o mesmo "crate" é o primeiro segmento do caminho.
            module
                .split_once('/')
                .map(|(head, _)| format!("{head}/"))
                .unwrap_or_else(|| module.clone())
        };
        self.file_prefixes.insert(relative.to_path_buf(), prefix);

        for alias in module_aliases(&module) {
            self.module_files.insert(alias, relative.to_path_buf());
        }

        if let Some(root) = crate_root_of(&module) {
            self.crate_roots
                .insert(crate_name.to_string(), root.clone());
            self.crate_roots.insert(crate_name.replace('-', "_"), root);
            if let Some(prefix) = crate_prefix_of(relative) {
                self.crate_prefixes.insert(crate_name.to_string(), prefix);
            }
        }
    }

    pub fn unregister_file(&mut self, relative: &Path) {
        let Some(module) = self.file_modules.remove(relative) else {
            return;
        };
        remove_if_owned(&mut self.module_files, &module, relative);
        self.file_crates.remove(relative);
        self.file_prefixes.remove(relative);
        for alias in module_aliases(&module) {
            remove_if_owned(&mut self.module_files, &alias, relative);
        }

        if let Some(root) = crate_root_of(&module) {
            let still_claimed = self
                .file_modules
                .values()
                .filter_map(|m| crate_root_of(m))
                .any(|other| other == root);
            if !still_claimed {
                let gone: Vec<String> = self
                    .crate_roots
                    .iter()
                    .filter(|(_, kept)| **kept == root)
                    .map(|(name, _)| name.clone())
                    .collect();
                for name in gone {
                    self.crate_roots.remove(&name);
                    self.crate_prefixes.remove(&name);
                }
            }
        }
    }

    pub fn file_module(&self, relative: &Path) -> Option<&str> {
        self.file_modules.get(relative).map(String::as_str)
    }

    /// Module where a `use` written in `relative` lives. `mod.rs`, `lib.rs` and
    /// `main.rs` name their directory, so `super` climbs from there.
    pub fn caller_module(&self, relative: &Path) -> Option<String> {
        let module = self.file_module(relative)?;
        let leaf = module.rsplit_once('/').map(|(_, leaf)| leaf)?;
        if matches!(leaf, "lib" | "main" | "mod") {
            Some(
                module
                    .rsplit_once('/')
                    .map(|(head, _)| head)
                    .unwrap_or("")
                    .to_string(),
            )
        } else {
            Some(module.to_string())
        }
    }

    pub fn module_prefix(&self, relative: &Path) -> Option<&str> {
        self.file_prefixes.get(relative).map(String::as_str)
    }

    pub fn resolve_import(&self, file: &Path, path: &str) -> Option<ImportTarget> {
        let module = self.caller_module(file)?;
        self.resolve_import_in(file, &module, path)
    }

    pub fn resolve_import_in(&self, file: &Path, module: &str, path: &str) -> Option<ImportTarget> {
        let segments = split_path(path);
        if segments.is_empty() {
            return None;
        }

        let head = segments[0];
        let base = if head == "crate" {
            let root = self
                .crate_roots
                .get(&crate_name_for(file))
                .cloned()
                .or_else(|| src_root_of(file))?;
            join(&root, &segments[1..])
        } else if head == "super" {
            let mut owner = module.to_string();
            let mut rest: &[&str] = &segments;
            while rest.first().is_some_and(|segment| *segment == "super") {
                owner = owner
                    .rsplit_once('/')
                    .map(|(head, _)| head.to_string())
                    .unwrap_or_default();
                rest = &rest[1..];
            }
            join(&owner, rest)
        } else if head == "self" {
            join(module, &segments[1..])
        } else if let Some(root) = self.crate_roots.get(head) {
            join(root, &segments[1..])
        } else {
            // No crate root (`src/main.rs`/`src/lib.rs`), `a` é `crate::a`, não
            // um irmão de `src`. Fora do root, o caminho é relativo ao módulo
            // que o contém.
            let parent = if self.crate_roots.values().any(|root| root == module) {
                module.to_string()
            } else {
                module
                    .rsplit_once('/')
                    .map(|(head, _)| head)
                    .unwrap_or("")
                    .to_string()
            };
            let sibling = join(&parent, &segments);
            if self.is_under_prefix(&sibling) {
                sibling
            } else {
                return Some(ImportTarget::External);
            }
        };

        let mut candidate = base;
        loop {
            if let Some(file) = self.module_files.get(&candidate) {
                return Some(ImportTarget::File(file.clone()));
            }
            candidate.truncate(candidate.rfind('/')?);
        }
    }

    fn is_under_prefix(&self, module: &str) -> bool {
        self.module_files.contains_key(module)
            || self
                .module_files
                .keys()
                .any(|key| key.starts_with(&format!("{module}/")))
    }

    fn crate_of(&self, file: &Path) -> Option<&str> {
        self.file_crates.get(file).map(String::as_str)
    }
}

pub fn pick_target(
    call_name: &str,
    caller_file: &Path,
    scope: &FileScope,
    resolver: &ScopeResolver,
    call_path: Option<&str>,
    definitions: &[NodeIndex],
    graph: &ProjectCodeGraph,
) -> Resolution {
    if definitions.is_empty() {
        return Resolution::Unknown;
    }

    let file_of = |index: NodeIndex| {
        graph
            .graph
            .node_weight(index)
            .map(|node| Path::new(node.relative.as_str()))
            .unwrap_or_else(|| Path::new(""))
    };

    let mut same_file = definitions
        .iter()
        .copied()
        .filter(|&index| file_of(index) == caller_file);
    if let (Some(only), None) = (same_file.next(), same_file.next()) {
        return Resolution::Resolved(only);
    }

    if let Some(binding) = scope.imports.get(call_name) {
        return match &binding.target {
            ImportTarget::External => Resolution::Unknown,
            ImportTarget::File(file) => match definitions
                .iter()
                .copied()
                .find(|&index| file_of(index) == file)
            {
                Some(node) => Resolution::Resolved(node),
                None => resolve_within(caller_file, scope, resolver, call_path, definitions, graph),
            },
        };
    }

    resolve_within(caller_file, scope, resolver, call_path, definitions, graph)
}

fn resolve_within(
    caller_file: &Path,
    scope: &FileScope,
    resolver: &ScopeResolver,
    call_path: Option<&str>,
    definitions: &[NodeIndex],
    graph: &ProjectCodeGraph,
) -> Resolution {
    let file_of = |index: NodeIndex| {
        graph
            .graph
            .node_weight(index)
            .map(|node| Path::new(node.relative.as_str()))
            .unwrap_or_else(|| Path::new(""))
    };

    if let Some(path) = call_path {
        let mut candidates = vec![path];
        if let Some((head, _)) = path.rsplit_once("::") {
            candidates.push(head);
        }
        for candidate in candidates {
            if let Some(ImportTarget::File(file)) = resolver.resolve_import(caller_file, candidate)
                && let Some(node) = definitions
                    .iter()
                    .copied()
                    .find(|&index| file_of(index) == file)
            {
                return Resolution::Resolved(node);
            }
        }
    }

    let from_globs: Vec<NodeIndex> = definitions
        .iter()
        .copied()
        .filter(|&index| scope.globs.iter().any(|glob| glob == file_of(index)))
        .collect();
    if let [only] = from_globs.as_slice() {
        return Resolution::Resolved(*only);
    }

    if let (Some(prefix), Some(crate_name)) = (
        resolver.module_prefix(caller_file),
        resolver.crate_of(caller_file),
    ) {
        let in_crate: Vec<NodeIndex> = definitions
            .iter()
            .copied()
            .filter(|&index| {
                let file = file_of(index);
                resolver
                    .file_module(file)
                    .is_some_and(|other| other.starts_with(prefix))
                    && resolver.crate_of(file) == Some(crate_name)
            })
            .collect();
        if let [only] = in_crate.as_slice() {
            return Resolution::Resolved(*only);
        }
    }

    let mut nodes: Vec<NodeIndex> = definitions.to_vec();
    nodes.sort_unstable();
    nodes.dedup();
    match nodes.as_slice() {
        [only] => Resolution::Resolved(*only),
        _ => Resolution::Ambiguous {
            candidates: nodes.len(),
        },
    }
}

#[allow(dead_code)]
pub fn last_segment(path: &str) -> Option<&str> {
    path.rsplit("::")
        .next()
        .map(str::trim)
        .filter(|segment| !segment.is_empty())
}

fn module_path_of(relative: &Path) -> String {
    let text = relative.to_string_lossy().replace('\\', "/");
    text.strip_suffix(".rs")
        .or_else(|| text.strip_suffix(".kt"))
        .or_else(|| text.strip_suffix(".kts"))
        .unwrap_or(&text)
        .to_string()
}

fn module_aliases(module: &str) -> Vec<String> {
    match module.rsplit_once('/') {
        Some((head, "lib" | "main" | "mod")) => vec![head.to_string()],
        _ => Vec::new(),
    }
}

fn crate_root_of(module: &str) -> Option<String> {
    module
        .rsplit_once('/')
        .filter(|(_, leaf)| *leaf == "lib" || *leaf == "main")
        .map(|(head, _)| head.to_string())
}

fn crate_prefix_of(file: &Path) -> Option<String> {
    let text = file.to_string_lossy().replace('\\', "/");
    let at = text.find("/src/")?;
    Some(text[..at + 1].to_string())
}

fn remove_if_owned(map: &mut HashMap<String, PathBuf>, key: &str, owner: &Path) {
    if map.get(key).is_some_and(|mapped| mapped == owner) {
        map.remove(key);
    }
}

fn join(base: &str, rest: &[&str]) -> String {
    let mut out = base.to_string();
    for segment in rest {
        if out.is_empty() {
            out.push_str(segment);
        } else {
            out.push('/');
            out.push_str(segment);
        }
    }
    out
}

fn split_path(path: &str) -> Vec<&str> {
    path.split("::")
        .map(str::trim)
        .filter(|segment| !segment.is_empty() && *segment != "*")
        .collect()
}

pub fn crate_name_for(file: &Path) -> String {
    let text = file.to_string_lossy().replace('\\', "/");
    let segments: Vec<&str> = text.split('/').collect();
    if let Some(at) = segments.iter().position(|segment| *segment == "crates")
        && let Some(name) = segments.get(at + 1)
    {
        return (*name).to_string();
    }
    if let Some(at) = segments.iter().position(|segment| *segment == "src")
        && at > 0
    {
        return segments[at - 1].to_string();
    }
    "crate".to_string()
}

fn src_root_of(file: &Path) -> Option<String> {
    let text = file.to_string_lossy().replace('\\', "/");
    let mut root = String::new();
    for segment in text.split('/') {
        if !root.is_empty() {
            root.push('/');
        }
        root.push_str(segment);
        if segment == "src" {
            return Some(root);
        }
    }
    None
}

pub fn expand_use(raw: &str) -> Vec<UseItem> {
    let mut out = Vec::new();
    let text = raw
        .trim()
        .trim_start_matches("use ")
        .trim_end_matches(';')
        .trim();
    expand_into(text, &mut out);
    out
}

fn expand_into(text: &str, out: &mut Vec<UseItem>) {
    let text = text.trim();
    if text.is_empty() {
        return;
    }

    if let Some(open) = text.find('{') {
        let prefix = text[..open].trim().trim_end_matches(':').trim();
        let Some(close) = text[open + 1..].find('}') else {
            return;
        };
        let inner_end = open + 1 + close;
        let inner = &text[open + 1..inner_end];
        for part in split_top_level_commas(inner) {
            let combined = if prefix.is_empty() {
                part
            } else {
                format!("{prefix}::{part}")
            };
            expand_into(&combined, out);
        }
        return;
    }

    let (path, alias) = match text.split_once(" as ") {
        Some((path, alias)) => (path.trim(), Some(alias.trim())),
        None => (text, None),
    };

    if let Some(module) = path.strip_suffix('*') {
        let module = module.trim().trim_end_matches(':').trim();
        if !module.is_empty() {
            out.push(UseItem::Glob {
                path: module.to_string(),
            });
        }
        return;
    }

    let name = alias
        .filter(|alias| !alias.is_empty())
        .or_else(|| path.rsplit("::").next())
        .unwrap_or(path);
    if !name.is_empty() {
        out.push(UseItem::Named {
            name: name.to_string(),
            path: path.to_string(),
        });
    }
}

fn split_top_level_commas(text: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut depth = 0usize;
    let mut current = String::new();
    for character in text.chars() {
        match character {
            '{' => {
                depth += 1;
                current.push(character);
            }
            '}' => {
                depth = depth.saturating_sub(1);
                current.push(character);
            }
            ',' if depth == 0 => {
                if !current.trim().is_empty() {
                    parts.push(current.trim().to_string());
                }
                current.clear();
            }
            _ => current.push(character),
        }
    }
    if !current.trim().is_empty() {
        parts.push(current.trim().to_string());
    }
    parts
}

#[cfg(test)]
mod tests {
    use super::*;

    fn resolver() -> ScopeResolver {
        let mut resolver = ScopeResolver::new();
        for path in [
            "src/lib.rs",
            "src/a.rs",
            "src/helper.rs",
            "src/nested/mod.rs",
            "src/nested/helper.rs",
        ] {
            let path = Path::new(path);
            resolver.register_file(path, &crate_name_for(path));
        }
        resolver
    }

    #[test]
    fn crate_and_self_and_super_resolve_to_files() {
        let resolver = resolver();
        assert_eq!(
            resolver.resolve_import_in(Path::new("src/lib.rs"), "src", "crate::a::helper"),
            Some(ImportTarget::File(PathBuf::from("src/a.rs")))
        );
        assert_eq!(
            resolver.resolve_import_in(
                Path::new("src/nested/mod.rs"),
                "src/nested",
                "super::helper"
            ),
            Some(ImportTarget::File(PathBuf::from("src/helper.rs")))
        );
        assert_eq!(
            resolver.resolve_import_in(
                Path::new("src/nested/mod.rs"),
                "src/nested",
                "self::helper"
            ),
            Some(ImportTarget::File(PathBuf::from("src/nested/helper.rs")))
        );
    }

    #[test]
    fn an_external_crate_resolves_to_external() {
        let resolver = resolver();
        assert_eq!(
            resolver.resolve_import_in(Path::new("src/lib.rs"), "src/lib", "tracing::info"),
            Some(ImportTarget::External)
        );
    }

    #[test]
    fn an_alias_keeps_the_local_name() {
        let items = expand_use("use crate::a::Thing as Other;");
        assert_eq!(
            items,
            vec![UseItem::Named {
                name: "Other".to_string(),
                path: "crate::a::Thing".to_string()
            }]
        );
    }

    #[test]
    fn brace_groups_expand() {
        let items = expand_use("use crate::a::{b, c as d};");
        assert_eq!(items.len(), 2);
        assert!(
            items
                .iter()
                .any(|item| matches!(item, UseItem::Named { name, .. } if name == "b"))
        );
        assert!(
            items
                .iter()
                .any(|item| matches!(item, UseItem::Named { name, .. } if name == "d"))
        );
    }

    #[test]
    fn a_malformed_use_does_not_panic() {
        assert!(expand_use("use a::}{b;").is_empty());
    }

    #[test]
    fn a_glob_is_a_glob() {
        let items = expand_use("use crate::a::*;");
        assert_eq!(
            items,
            vec![UseItem::Glob {
                path: "crate::a".to_string()
            }]
        );
    }
}
