use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use serde::Deserialize;
use tree_sitter::Parser;
use tupi_kernel::{Tool, ToolError, ToolSpec};

use crate::budget::{self, Section};
use crate::span::{
    ResolveOutcome, SpanResolver, SymbolSpan, hash_matches, hash_prefix, render_candidate,
};
use crate::store::ResponseStore;

#[derive(Debug, Deserialize)]
pub struct EditParams {
    pub path: String,
    pub symbol: String,
    pub expected_hash: String,
    pub new_body: String,
    pub signature: Option<String>,
    pub budget_tokens: Option<usize>,
}

/// Edita o corpo de um símbolo por span de AST.
///
/// O modelo fornece o símbolo e o novo corpo, nunca uma âncora de arquivo
/// inteiro. O hash é a guarda de leitura: uma edição sobre versão que o agente
/// não viu é recusada em vez de sobrescrever em silêncio.
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
                    "`{}` aparece {} vezes: {}. Reenvie qualificado.",
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

        let on_disk = std::fs::read_to_string(&path).map_err(|error| {
            ToolError::Other(format!(
                "a escrita foi feita, mas a verificação não releu {}: {error}",
                path.display()
            ))
        })?;
        if blake3::hash(on_disk.as_bytes()) != blake3::hash(new_content.as_bytes()) {
            return Err(ToolError::Other(format!(
                "verificação pós-escrita falhou: o hash em disco não é o esperado ({})",
                path.display()
            )));
        }
        let verified_table = resolver.table(&path, &on_disk)?;
        let verified = match resolver.resolve(&verified_table, &params.symbol) {
            ResolveOutcome::Found(span) => span,
            _ => {
                return Err(ToolError::Other(
                    "verificação pós-escrita falhou: o símbolo não re-resolve no arquivo novo"
                        .to_string(),
                ));
            }
        };
        let disk_body = &on_disk[verified.body_range.clone()];
        if disk_body != params.new_body {
            return Err(ToolError::Other(
                "verificação pós-escrita falhou: o corpo em disco difere do corpo enviado"
                    .to_string(),
            ));
        }

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

/// Resolve `.` e `..` sem tocar no disco, para detectar fuga mesmo quando o
/// alvo não existe.
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

fn qualified_span(span: &SymbolSpan) -> String {
    match &span.container {
        Some(container) => format!("{container}::{}", span.name),
        None => span.name.clone(),
    }
}

fn splice(
    content: &str,
    span: &SymbolSpan,
    new_body: &str,
    signature: Option<&str>,
) -> Result<String, ToolError> {
    if span.body_range.end > content.len()
        || span.body_range.start > span.body_range.end
        || span.head_range.end > span.body_range.start
    {
        return Err(ToolError::Other(
            "intervalo do símbolo inválido; nada foi escrito".to_string(),
        ));
    }

    let mut spliced = String::with_capacity(
        content.len() - span.body_range.len() + new_body.len() + signature.map_or(0, str::len) + 1,
    );
    spliced.push_str(&content[..span.body_range.start]);
    spliced.push_str(new_body);
    spliced.push_str(&content[span.body_range.end..]);

    if let Some(signature) = signature {
        let signature = signature.trim_end();
        if signature.is_empty() {
            return Err(ToolError::InvalidArguments(
                "`signature` vazia; nada foi escrito".to_string(),
            ));
        }
        let mut with_head = String::with_capacity(spliced.len() + signature.len() + 1);
        with_head.push_str(&spliced[..span.head_range.start]);
        with_head.push_str(signature);
        with_head.push(' ');
        with_head.push_str(&spliced[span.body_range.start..]);
        spliced = with_head;
    }

    Ok(spliced)
}

fn first_parse_error(content: &str) -> Option<usize> {
    let mut parser = Parser::new();
    parser
        .set_language(&tree_sitter_rust::LANGUAGE.into())
        .ok()?;
    let tree = parser.parse(content, None)?;
    let root = tree.root_node();
    if !root.has_error() {
        return None;
    }
    Some(first_error_node(root).map_or(1, |node| node.start_position().row + 1))
}

fn first_error_node<'a>(node: tree_sitter::Node<'a>) -> Option<tree_sitter::Node<'a>> {
    if node.is_error() || node.is_missing() {
        return Some(node);
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if let Some(found) = first_error_node(child) {
            return Some(found);
        }
    }
    None
}

fn atomic_write(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    static COUNTER: AtomicU64 = AtomicU64::new(0);

    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("file");
    let tmp = dir.join(format!(
        ".{file_name}.tupi-tmp-{}-{}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));

    let permissions = std::fs::metadata(path)
        .ok()
        .map(|metadata| metadata.permissions());
    let result = (|| {
        let mut file = std::fs::File::create(&tmp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        if let Some(permissions) = permissions {
            std::fs::set_permissions(&tmp, permissions)?;
        }
        std::fs::rename(&tmp, path)
    })();

    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

impl Tool for EditTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "edit".to_string(),
            description: "Edita o corpo de um símbolo por span de AST, com expected_hash do read. Recusa hash divergente, símbolo ambíguo ou sintaxe inválida.".to_string(),
            schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Caminho relativo ao diretório raiz do projeto."
                    },
                    "symbol": {
                        "type": "string",
                        "description": "Nome simples (`verify`) ou qualificado (`Auth::verify`, `Default::new`)."
                    },
                    "expected_hash": {
                        "type": "string",
                        "description": "blake3 do arquivo inteiro, como `read` imprime no cabeçalho (prefixo de 12 caracteres aceito)."
                    },
                    "new_body": {
                        "type": "string",
                        "description": "Novo bloco `{ … }` completo, chaves incluídas. O splice é de bytes crus: sem reindentação e sem formatação."
                    },
                    "signature": {
                        "type": "string",
                        "description": "Opcional: substitui tudo o que antecede o `{` (assinatura, genéricos, where). Sem ele, a assinatura fica como está."
                    },
                    "budget_tokens": {
                        "type": "integer",
                        "minimum": 1,
                        "description": "Teto de tokens (excedente em peek)."
                    }
                },
                "required": ["path", "symbol", "expected_hash", "new_body"]
            }),
        }
    }

    fn call(&self, arguments: serde_json::Value) -> Result<String, ToolError> {
        let params: EditParams = serde_json::from_value(arguments)
            .map_err(|error| ToolError::InvalidArguments(error.to_string()))?;
        self.edit(params)
    }

    fn describe_call(&self, arguments: &serde_json::Value) -> String {
        let mut parts = Vec::new();
        if let Some(path) = arguments.get("path").and_then(|value| value.as_str()) {
            parts.push(format!("path={path}"));
        }
        if let Some(symbol) = arguments.get("symbol").and_then(|value| value.as_str()) {
            parts.push(format!("symbol={symbol}"));
        }
        if parts.is_empty() {
            "edit".to_string()
        } else {
            parts.join(" · ")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::Arc;
    use tempfile::{TempDir, tempdir};
    use tupi_kernel::{Tool, ToolError};

    use crate::budget::DEFAULT_BUDGET_TOKENS;
    use crate::span::hash_prefix;
    use crate::store::ResponseStore;

    fn fixture() -> TempDir {
        let dir = tempdir().unwrap();
        fs::write(
            dir.path().join("auth.rs"),
            "use std::io;\n\npub fn verify(raw: &str) -> bool {\n    !raw.is_empty()\n}\n\npub fn logout() {\n}\n",
        )
        .unwrap();
        dir
    }

    fn tool_for(dir: &std::path::Path) -> EditTool {
        EditTool::new(
            dir.to_path_buf(),
            Arc::new(ResponseStore::new(
                std::time::Duration::from_secs(60),
                1024 * 1024,
            )),
            DEFAULT_BUDGET_TOKENS,
        )
    }

    fn params(dir: &TempDir, symbol: &str, body: &str) -> EditParams {
        let content = fs::read(dir.path().join("auth.rs")).unwrap();
        EditParams {
            path: "auth.rs".into(),
            symbol: symbol.into(),
            expected_hash: hash_prefix(&content),
            new_body: body.into(),
            signature: None,
            budget_tokens: None,
        }
    }

    #[test]
    fn an_edit_replaces_the_body_and_leaves_the_rest_byte_identical() {
        let dir = fixture();
        let tool = tool_for(dir.path());
        let out = tool
            .edit(params(&dir, "verify", "{\n    raw.len() > 3\n}"))
            .unwrap();

        let after = fs::read_to_string(dir.path().join("auth.rs")).unwrap();
        assert_eq!(
            after,
            "use std::io;\n\npub fn verify(raw: &str) -> bool {\n    raw.len() > 3\n}\n\npub fn logout() {\n}\n"
        );
        assert!(out.contains("editado"), "{out}");
        assert!(out.contains(&hash_prefix(after.as_bytes())), "{out}");
    }

    #[test]
    fn a_stale_hash_never_writes() {
        let dir = fixture();
        let target = dir.path().join("auth.rs");
        let before = fs::read_to_string(&target).unwrap();
        let mut request = params(&dir, "verify", "{\n    true\n}");
        request.expected_hash = "deadbeef".into();

        let error = tool_for(dir.path()).edit(request).unwrap_err();

        assert!(matches!(error, ToolError::StaleHash(_)), "{error:?}");
        assert_eq!(fs::read_to_string(&target).unwrap(), before);
    }

    #[test]
    fn an_already_applied_body_with_a_stale_hash_is_idempotent() {
        let dir = fixture();
        let tool = tool_for(dir.path());
        let old_hash = params(&dir, "verify", "{}").expected_hash;
        tool.edit(params(&dir, "verify", "{\n    raw.len() > 3\n}"))
            .unwrap();
        let after = fs::read_to_string(dir.path().join("auth.rs")).unwrap();

        let mut again = params(&dir, "verify", "{\n    raw.len() > 3\n}");
        again.expected_hash = old_hash;
        let out = tool.edit(again).unwrap();

        assert!(out.contains("já aplicada"), "{out}");
        assert_eq!(
            fs::read_to_string(dir.path().join("auth.rs")).unwrap(),
            after
        );
    }

    #[test]
    fn an_unknown_symbol_is_reported() {
        let dir = fixture();
        let error = tool_for(dir.path())
            .edit(params(&dir, "nope", "{}"))
            .unwrap_err();
        assert!(matches!(error, ToolError::SymbolNotFound(_)), "{error:?}");
    }

    #[test]
    fn an_ambiguous_symbol_is_refused_and_nothing_is_written() {
        let dir = tempdir().unwrap();
        let content = "impl A {\n    fn new() {}\n}\n\nimpl B {\n    fn new() {}\n}\n";
        fs::write(dir.path().join("auth.rs"), content).unwrap();
        let request = params(&dir, "new", "{\n    1\n}");

        let error = tool_for(dir.path()).edit(request).unwrap_err();

        assert!(matches!(error, ToolError::Ambiguous(_)), "{error:?}");
        assert_eq!(
            fs::read_to_string(dir.path().join("auth.rs")).unwrap(),
            content
        );
    }

    #[test]
    fn a_signature_replaces_the_head() {
        let dir = fixture();
        let mut request = params(&dir, "verify", "{\n    1\n}");
        request.signature = Some("pub fn verify(raw: &str) -> u8".into());

        tool_for(dir.path()).edit(request).unwrap();

        let after = fs::read_to_string(dir.path().join("auth.rs")).unwrap();
        assert!(
            after.contains("pub fn verify(raw: &str) -> u8 {\n    1\n}"),
            "{after}"
        );
    }

    #[test]
    fn a_body_without_braces_is_rejected() {
        let dir = fixture();
        let before = fs::read_to_string(dir.path().join("auth.rs")).unwrap();
        let error = tool_for(dir.path())
            .edit(params(&dir, "verify", "return true;"))
            .unwrap_err();

        assert!(matches!(error, ToolError::InvalidArguments(_)), "{error:?}");
        assert_eq!(
            fs::read_to_string(dir.path().join("auth.rs")).unwrap(),
            before
        );
    }

    #[test]
    fn invalid_syntax_does_not_write() {
        let dir = fixture();
        let before = fs::read_to_string(dir.path().join("auth.rs")).unwrap();
        let error = tool_for(dir.path())
            .edit(params(&dir, "verify", "{\n    let ;\n}"))
            .unwrap_err();

        assert!(matches!(error, ToolError::InvalidSyntax(_)), "{error:?}");
        assert_eq!(
            fs::read_to_string(dir.path().join("auth.rs")).unwrap(),
            before
        );
    }

    #[test]
    fn a_struct_body_is_editable() {
        let dir = tempdir().unwrap();
        fs::write(
            dir.path().join("point.rs"),
            "struct Point {\n    x: u8,\n}\n",
        )
        .unwrap();
        let content = fs::read(dir.path().join("point.rs")).unwrap();
        let tool = tool_for(dir.path());
        tool.edit(EditParams {
            path: "point.rs".into(),
            symbol: "Point".into(),
            expected_hash: hash_prefix(&content),
            new_body: "{\n    x: u8,\n    y: u8,\n}".into(),
            signature: None,
            budget_tokens: None,
        })
        .unwrap();

        let after = fs::read_to_string(dir.path().join("point.rs")).unwrap();
        assert_eq!(after, "struct Point {\n    x: u8,\n    y: u8,\n}\n");
    }

    #[test]
    fn a_path_outside_the_root_is_rejected() {
        let dir = fixture();
        let mut request = params(&dir, "verify", "{}");
        request.path = "../fora.rs".into();
        let error = tool_for(dir.path()).edit(request).unwrap_err();
        assert!(matches!(error, ToolError::OutsideRoot(_)), "{error:?}");
    }

    #[test]
    fn a_non_utf8_file_is_rejected() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("bin.rs"), [0xff, 0xfe]).unwrap();
        let error = tool_for(dir.path())
            .edit(EditParams {
                path: "bin.rs".into(),
                symbol: "x".into(),
                expected_hash: "deadbeef".into(),
                new_body: "{}".into(),
                signature: None,
                budget_tokens: None,
            })
            .unwrap_err();
        assert!(matches!(error, ToolError::Other(_)), "{error:?}");
    }

    #[test]
    fn describe_shows_path_and_symbol() {
        let dir = fixture();
        let tool = tool_for(dir.path());
        assert_eq!(
            tool.describe_call(&serde_json::json!({
                "path": "auth.rs",
                "symbol": "Auth::verify"
            })),
            "path=auth.rs · symbol=Auth::verify"
        );
    }
}
