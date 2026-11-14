mod params;

mod splice;

#[cfg(test)]
mod tests;

mod tool;

use splice::{first_parse_error, qualified_span, splice};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use tree_sitter::Parser;
use tupi_kernel::{Tool, ToolError, ToolSpec};

use crate::budget::{self, Section};
use crate::paths::{atomic_write, normalize};
use crate::span::{
    ResolveOutcome, SpanResolver, SymbolSpan, hash_matches, hash_prefix, render_candidate,
};
use crate::store::ResponseStore;

pub use params::EditParams;

pub struct EditTool {
    root_dir: PathBuf,
    store: Arc<ResponseStore>,
    default_budget_tokens: usize,
}

impl EditTool {
    pub fn new(root_dir: PathBuf, store: Arc<ResponseStore>, default_budget_tokens: usize) -> Self {
        Self {
            root_dir,
            store,
            default_budget_tokens,
        }
    }

    pub fn edit(&self, params: EditParams) -> Result<String, ToolError> {
        let path = self.resolve(&params.path)?;
        if path.extension().and_then(|ext| ext.to_str()) != Some("rs") {
            return Err(ToolError::Other(format!(
                "edição de símbolos só é suportada em Rust: {}",
                params.path
            )));
        }

        let original = std::fs::read(&path).map_err(|error| {
            ToolError::Other(format!("falha ao ler {}: {error}", path.display()))
        })?;
        let content = String::from_utf8(original.clone()).map_err(|_| {
            ToolError::Other(format!(
                "{} não é UTF-8 válido; nada foi escrito",
                params.path
            ))
        })?;
        let old_prefix = hash_prefix(&original);
        let resolver = SpanResolver::new();

        if !hash_matches(&original, &params.expected_hash) {
            let table = resolver.table(&path, &content)?;
            if let ResolveOutcome::Found(span) = resolver.resolve(&table, &params.symbol)
                && content[span.body_range.clone()].trim() == params.new_body.trim()
            {
                return Ok(format!(
                    "### `{}` — já aplicada\n\nArquivo: blake3 `{old_prefix}`; nada foi escrito.\n",
                    params.symbol.trim()
                ));
            }
            return Err(ToolError::StaleHash(format!(
                "hash atual: `{old_prefix}`. Reenvie com expected_hash atualizado"
            )));
        }

        let trimmed = params.new_body.trim();
        if !trimmed.starts_with('{') || !trimmed.ends_with('}') {
            return Err(ToolError::InvalidArguments(
                "`new_body` precisa ser um bloco `{ … }` completo".to_string(),
            ));
        }

        let table = resolver.table(&path, &content)?;
        let span = match resolver.resolve(&table, &params.symbol) {
            ResolveOutcome::Found(span) => span,
            ResolveOutcome::Ambiguous(candidates) => {
                let rendered: Vec<String> = candidates.iter().map(render_candidate).collect();
                return Err(ToolError::Ambiguous(format!(
                    "`{}` aparece {} vezes: {}. Reenvie com `nome@linha`.",
                    params.symbol,
                    candidates.len(),
                    rendered.join(", ")
                )));
            }
            ResolveOutcome::NotFound { suggestion } => {
                let hint = suggestion
                    .map(|name| format!(" Mais próximo: `{name}`."))
                    .unwrap_or_default();
                return Err(ToolError::SymbolNotFound(format!(
                    "`{}` não encontrado em `{}`.{hint}",
                    params.symbol, params.path
                )));
            }
            ResolveOutcome::NoBody { candidates } => {
                return Err(ToolError::Other(format!(
                    "símbolo `{}` não tem bloco `{{…}}` ({}). Nada foi escrito.",
                    params.symbol,
                    candidates.join(", ")
                )));
            }
        };

        let new_content = splice(
            &content,
            &span,
            &params.new_body,
            params.signature.as_deref(),
        )?;
        if let Some(line) = first_parse_error(&new_content) {
            return Err(ToolError::InvalidSyntax(format!(
                "o arquivo não parsearia depois da edição (erro na linha {line}); nada foi escrito"
            )));
        }

        // O hash e o rename não são atômicos juntos: um format-on-save pode cair
        // na janela. Reler imediatamente antes de escrever evita o clobber.
        let current = std::fs::read(&path).map_err(|error| {
            ToolError::Other(format!("falha ao reler {}: {error}", path.display()))
        })?;
        if current != original {
            return Err(ToolError::StaleHash(format!(
                "arquivo mudou durante a edição; hash atual: `{}`",
                hash_prefix(&current)
            )));
        }

        atomic_write(&path, new_content.as_bytes()).map_err(|error| {
            ToolError::Other(format!("falha ao escrever {}: {error}", path.display()))
        })?;

        // Depois da escrita, qualquer divergência restaura o original: um edit
        // rejeitado nunca pode deixar o arquivo alterado.
        let on_disk = match std::fs::read_to_string(&path) {
            Ok(on_disk) => on_disk,
            Err(error) => {
                let _ = atomic_write(&path, &original);
                return Err(ToolError::Other(format!(
                    "a escrita foi feita, mas a verificação não releu {}: {error}; o arquivo foi restaurado",
                    path.display()
                )));
            }
        };
        if blake3::hash(on_disk.as_bytes()) != blake3::hash(new_content.as_bytes()) {
            let _ = atomic_write(&path, &original);
            return Err(ToolError::Other(format!(
                "verificação pós-escrita falhou: o hash em disco não é o esperado ({}); o arquivo foi restaurado",
                path.display()
            )));
        }
        let verified = resolver
            .table(&path, &on_disk)?
            .spans
            .into_iter()
            .find(|span| on_disk[span.body_range.clone()].trim() == params.new_body.trim());
        let Some(verified) = verified else {
            let _ = atomic_write(&path, &original);
            return Err(ToolError::Other(
                "verificação pós-escrita falhou: o bloco em disco não é o enviado; o arquivo foi restaurado"
                    .to_string(),
            ));
        };
        let disk_body = &on_disk[verified.body_range.clone()];

        let root = self
            .root_dir
            .canonicalize()
            .unwrap_or_else(|_| self.root_dir.clone());
        let relative = path
            .strip_prefix(&root)
            .unwrap_or(&path)
            .to_string_lossy()
            .replace('\\', "/");
        let qualified = qualified_span(&verified);
        let sections = vec![
            Section {
                heading: format!(
                    "### `{qualified}` — `{relative}:{}-{}` editado",
                    verified.start_line, verified.end_line
                ),
                body: format!(
                    "\nArquivo: blake3 `{}` (antes `{old_prefix}`)\n",
                    hash_prefix(on_disk.as_bytes())
                ),
                priority: 255,
            },
            Section {
                heading: format!("#### corpo de `{qualified}`"),
                body: format!("```rust\n{disk_body}\n```\n"),
                priority: 254,
            },
        ];

        let budget = params.budget_tokens.unwrap_or(self.default_budget_tokens);
        let selection = budget::select(&sections, budget);
        let handle = if selection.omitted.is_empty() {
            None
        } else {
            Some(self.store.insert(selection.omitted_body()))
        };
        Ok(budget::render(&selection, handle.as_deref()))
    }

    fn resolve(&self, requested: &str) -> Result<PathBuf, ToolError> {
        let candidate = Path::new(requested);
        let joined = if candidate.is_absolute() {
            candidate.to_path_buf()
        } else {
            self.root_dir.join(candidate)
        };
        let root = self
            .root_dir
            .canonicalize()
            .map_err(|error| ToolError::Other(format!("root inválido: {error}")))?;
        let normalized = normalize(&joined);
        if !normalized.starts_with(&root) {
            return Err(ToolError::OutsideRoot(requested.to_string()));
        }
        let canonical = normalized
            .canonicalize()
            .map_err(|_| ToolError::NotFound(requested.to_string()))?;
        if !canonical.starts_with(&root) {
            return Err(ToolError::OutsideRoot(requested.to_string()));
        }
        Ok(canonical)
    }
}
