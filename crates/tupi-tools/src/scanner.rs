use std::path::{Path, PathBuf};

use grep_regex::RegexMatcherBuilder;
use grep_searcher::{SearcherBuilder, Sink, SinkMatch};
use ignore::types::TypesBuilder;
use ignore::WalkBuilder;
use tupi_kernel::ToolError;

/// Mais de 1 MiB é quase sempre gerado/minificado: caro de ler e ruim de exibir.
const MAX_FILE_BYTES: u64 = 1024 * 1024;

/// Teto de linhas casadas por arquivo. O AST deduplica depois; o teto evita
/// varrer um arquivo inteiro quando uma palavra comum casa em tudo.
const MAX_MATCHES_PER_FILE: usize = 200;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawMatch {
    pub file_path: PathBuf,
    pub line_number: usize,
    pub byte_offset: u64,
    pub line_content: String,
}

pub struct LexicalScanner {
    root_dir: PathBuf,
}

impl LexicalScanner {
    pub fn new(root_dir: PathBuf) -> Self {
        Self { root_dir }
    }

    /// Varre o projeto (ou um prefixo) por linhas que contenham qualquer termo.
    /// A ordem é determinística: caminho e depois linha.
    pub fn search(
        &self,
        terms: &[String],
        path_prefix: Option<&str>,
    ) -> Result<Vec<RawMatch>, ToolError> {
        if terms.is_empty() {
            return Ok(Vec::new());
        }

        let root = self
            .root_dir
            .canonicalize()
            .map_err(|error| ToolError::Other(format!("root inválido: {error}")))?;

        let base = match path_prefix
            .map(str::trim)
            .filter(|prefix| !prefix.is_empty())
        {
            Some(prefix) => {
                let candidate = Path::new(prefix);
                let joined = if candidate.is_absolute() {
                    candidate.to_path_buf()
                } else {
                    root.join(candidate)
                };
                let normalized = normalize(&joined);
                if !normalized.starts_with(&root) {
                    return Err(ToolError::OutsideRoot(prefix.to_string()));
                }
                let canonical = normalized
                    .canonicalize()
                    .map_err(|_| ToolError::NotFound(prefix.to_string()))?;
                if !canonical.starts_with(&root) {
                    return Err(ToolError::OutsideRoot(prefix.to_string()));
                }
                canonical
            }
            None => root,
        };

        let mut types = TypesBuilder::new();
        for pattern in ["*.rs", "*.kt", "*.kts"] {
            types
                .add("code", pattern)
                .map_err(|error| ToolError::Other(format!("filtro de extensão: {error}")))?;
        }
        types.select("code");
        let types = types
            .build()
            .map_err(|error| ToolError::Other(format!("filtro de extensão: {error}")))?;
        let escaped: Vec<String> = terms
            .iter()
            .map(|term| term.trim())
            .filter(|term| !term.is_empty())
            .map(regex::escape)
            .collect();
        if escaped.is_empty() {
            return Ok(Vec::new());
        }
        let pattern = escaped.join("|");
        let matcher = RegexMatcherBuilder::new()
            .case_insensitive(true)
            .build(&pattern)
            .map_err(|error| ToolError::Other(format!("padrão de busca: {error}")))?;

        let mut builder = WalkBuilder::new(&base);
        builder
            .types(types)
            .standard_filters(true)
            .hidden(true)
            .require_git(false)
            .git_ignore(true)
            .git_global(true)
            .git_exclude(true)
            .filter_entry(|entry| !excluded(entry));

        let mut matches = Vec::new();
        for entry in builder.build().flatten() {
            if !entry.file_type().is_some_and(|kind| kind.is_file()) {
                continue;
            }
            if entry
                .metadata()
                .map(|metadata| metadata.len() > MAX_FILE_BYTES)
                .unwrap_or(true)
            {
                continue;
            }
            let path = entry.path();
            let mut file_matches = Vec::new();
            let sink = MatchCollector {
                matches: &mut file_matches,
                file_path: path,
            };
            let mut searcher = SearcherBuilder::new()
                .line_number(true)
                .multi_line(false)
                .build();
            if searcher.search_path(&matcher, path, sink).is_err() {
                continue;
            }
            matches.extend(file_matches);
        }

        matches.sort_by(|a, b| {
            a.file_path
                .cmp(&b.file_path)
                .then_with(|| a.line_number.cmp(&b.line_number))
        });
        Ok(matches)
    }
}

struct MatchCollector<'a> {
    matches: &'a mut Vec<RawMatch>,
    file_path: &'a Path,
}

impl Sink for MatchCollector<'_> {
    type Error = std::io::Error;

    fn matched(
        &mut self,
        _searcher: &grep_searcher::Searcher,
        mat: &SinkMatch<'_>,
    ) -> Result<bool, Self::Error> {
        if self.matches.len() >= MAX_MATCHES_PER_FILE {
            return Ok(false);
        }
        self.matches.push(RawMatch {
            file_path: self.file_path.to_path_buf(),
            line_number: mat.line_number().unwrap_or(1) as usize,
            byte_offset: mat.absolute_byte_offset(),
            line_content: String::from_utf8_lossy(mat.bytes()).to_string(),
        });
        Ok(true)
    }
}

/// Resolve `.` e `..` sem tocar no disco.
fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::Prefix(_) | std::path::Component::RootDir => {
                out.push(component.as_os_str());
            }
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                out.pop();
            }
            std::path::Component::Normal(part) => out.push(part),
        }
    }
    out
}

fn excluded(entry: &ignore::DirEntry) -> bool {
    if !entry.file_type().is_some_and(|kind| kind.is_dir()) {
        return false;
    }
    matches!(
        entry.file_name().to_str(),
        Some("target" | "build" | ".gradle" | "node_modules" | ".git")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_term_list_finds_nothing() {
        let scanner = LexicalScanner::new(std::env::temp_dir());
        assert!(scanner.search(&[], None).unwrap().is_empty());
    }
}
