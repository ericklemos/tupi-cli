//! Rota literal do `edit`: o endereço é o texto, não a gramática.
//!
//! O casamento é byte a byte e vale para qualquer arquivo de texto (md, yaml,
//! toml, py, ts, go, sql): nenhum parser participa da decisão. A âncora só vale
//! com exatamente uma ocorrência — zero é "o trecho não existe" e mais de uma é
//! a contagem com as linhas onde casou. Nos dois casos a recusa é contrato, não
//! aviso, e nada é escrito.
//!
//! O contrato de guarda é o da rota AST: `expected_hash` do arquivo inteiro
//! antes de tocar no disco, releitura imediata antes do rename e restauração do
//! original se o disco divergir depois da escrita.

use std::path::Path;

use tupi_kernel::ToolError;

use super::params::EditParams;
use crate::budget::{self, Section};
use crate::paths::{atomic_write, resolve_for_write};
use crate::span::{hash_matches, hash_prefix};
use crate::store::ResponseStore;

/// Quantas linhas ambíguas a recusa lista antes de resumir o resto.
const MAX_LINES_LISTED: usize = 10;

/// Linhas de contexto mostradas depois da substituição.
const CONTEXT_LINES: usize = 3;

/// Ocorrência de `old` no conteúdo: deslocamento em bytes e linha (1-based).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Anchor {
    pub offset: usize,
    pub line: usize,
}

impl Anchor {
    /// Última linha tocada por uma ocorrência que começa em `line`.
    pub fn end_line(&self, old: &str) -> usize {
        self.line + old.bytes().filter(|byte| *byte == b'\n').count()
    }
}

/// Linha (1-based) do deslocamento em bytes.
pub fn line_of(content: &str, offset: usize) -> usize {
    content[..offset]
        .bytes()
        .filter(|byte| *byte == b'\n')
        .count()
        + 1
}

/// Todas as ocorrências de `needle`, inclusive as sobrepostas: `aa` em `aaa`
/// casa duas vezes, e a segunda também é ambiguidade.
pub fn occurrences(content: &str, needle: &str) -> Vec<Anchor> {
    let mut found = Vec::new();
    if needle.is_empty() {
        return found;
    }
    let mut from = 0;
    while from < content.len() {
        let Some(relative) = content[from..].find(needle) else {
            break;
        };
        let offset = from + relative;
        found.push(Anchor {
            offset,
            line: line_of(content, offset),
        });
        let mut next = offset + 1;
        while next < content.len() && !content.is_char_boundary(next) {
            next += 1;
        }
        from = next;
    }
    found
}

/// Exige exatamente uma ocorrência de `old`. Zero e mais de uma são recusa: o
/// análogo textual de `StaleHash`, com contagem e linhas no recado.
pub fn single(content: &str, old: &str, path: &str) -> Result<Anchor, ToolError> {
    let found = occurrences(content, old);
    match found.len() {
        0 => Err(ToolError::Other(format!(
            "`old` não existe em `{path}`: o trecho ({} bytes) não aparece byte a byte. \
             A âncora precisa ser copiada literalmente de `read`/`lookup lines`, \
             incluindo indentação e quebras de linha. Nada foi escrito.",
            old.len()
        ))),
        1 => Ok(found[0]),
        count => {
            let listed: Vec<String> = found
                .iter()
                .take(MAX_LINES_LISTED)
                .map(|anchor| anchor.line.to_string())
                .collect();
            let rest = count.saturating_sub(MAX_LINES_LISTED);
            let extra = if rest > 0 {
                format!(" (… +{rest})")
            } else {
                String::new()
            };
            Err(ToolError::Ambiguous(format!(
                "`old` casa {count} vezes em `{path}`, nas linhas {}{extra}. \
                 A âncora precisa casar exatamente uma vez: inclua as linhas vizinhas \
                 até sobrar uma. Nada foi escrito.",
                listed.join(", ")
            )))
        }
    }
}

/// Janela de linhas em torno de `first..=last`, para o resultado mostrar o que
/// mudou sem devolver o arquivo inteiro. Devolve a primeira linha e o texto.
pub fn window(content: &str, first: usize, last: usize, radius: usize) -> (usize, String) {
    let start = first.saturating_sub(radius).max(1);
    let end = last.max(first).saturating_add(radius);
    let mut text = String::new();
    for (index, line) in content.split_inclusive('\n').enumerate() {
        let number = index + 1;
        if number < start {
            continue;
        }
        if number > end {
            break;
        }
        text.push_str(line);
    }
    if !text.is_empty() && !text.ends_with('\n') {
        text.push('\n');
    }
    (start, text)
}

/// Linguagem da cerca de código, derivada da extensão; vazia quando não há
/// marcador conhecido (o texto continua legível sem ele).
pub fn fence_language(path: &Path) -> &'static str {
    match path.extension().and_then(|ext| ext.to_str()) {
        Some("rs") => "rust",
        Some("md") | Some("markdown") => "markdown",
        Some("yml") | Some("yaml") => "yaml",
        Some("toml") => "toml",
        Some("json") => "json",
        Some("py") => "python",
        Some("ts") => "typescript",
        Some("tsx") => "tsx",
        Some("js") | Some("mjs") | Some("cjs") => "javascript",
        Some("go") => "go",
        Some("sql") => "sql",
        Some("sh") | Some("bash") => "bash",
        Some("html") => "html",
        Some("css") => "css",
        _ => "",
    }
}

/// Substituição literal única: `old` sai, `new` entra, mesmo contrato de guarda
/// da rota AST. Sem parser e sem escape — 0 ou >1 ocorrências é recusa.
pub fn edit_literal(
    root_dir: &Path,
    store: &ResponseStore,
    default_budget_tokens: usize,
    params: &EditParams,
    old: &str,
    new: &str,
) -> Result<String, ToolError> {
    if old == new {
        return Err(ToolError::InvalidArguments(
            "`old` e `new` são idênticos: não há substituição a fazer; nada foi escrito".to_string(),
        ));
    }

    let path = resolve_for_write(root_dir, &params.path)?;
    let original = std::fs::read(&path)
        .map_err(|error| ToolError::Other(format!("falha ao ler {}: {error}", path.display())))?;
    let content = String::from_utf8(original.clone()).map_err(|_| {
        ToolError::Other(format!(
            "{} não é UTF-8 válido; nada foi escrito",
            params.path
        ))
    })?;
    let old_prefix = hash_prefix(&original);

    if !hash_matches(&original, &params.expected_hash) {
        // Já aplicada: a âncora sumiu e o texto novo está no lugar. Nada é
        // escrito e o hash velho não vira passe livre.
        if occurrences(&content, old).is_empty() && occurrences(&content, new).len() == 1 {
            return Ok(format!(
                "### `{}` — já aplicada\n\nArquivo: blake3 `{old_prefix}`; nada foi escrito.\n",
                params.path
            ));
        }
        return Err(ToolError::StaleHash(format!(
            "hash atual: `{old_prefix}`. Reenvie com expected_hash atualizado"
        )));
    }

    let anchor = single(&content, old, &params.path)?;
    let mut new_content = String::with_capacity(content.len() + new.len());
    new_content.push_str(&content[..anchor.offset]);
    new_content.push_str(new);
    new_content.push_str(&content[anchor.offset + old.len()..]);

    // O hash e o rename não são atômicos juntos: reler imediatamente antes de
    // escrever evita o clobber de um format-on-save que caia na janela.
    let current = std::fs::read(&path)
        .map_err(|error| ToolError::Other(format!("falha ao reler {}: {error}", path.display())))?;
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

    let root = root_dir
        .canonicalize()
        .unwrap_or_else(|_| root_dir.to_path_buf());
    let relative = path
        .strip_prefix(&root)
        .unwrap_or(&path)
        .to_string_lossy()
        .replace('\\', "/");
    let newlines = new.bytes().filter(|byte| *byte == b'\n').count();
    let last_line = if new.is_empty() || new.ends_with('\n') {
        anchor.line + newlines.saturating_sub(1)
    } else {
        anchor.line + newlines
    };
    let old_lines = anchor.end_line(old) - anchor.line + 1;
    let new_lines = if new.is_empty() {
        0
    } else {
        last_line - anchor.line + 1
    };
    let (first, context) = window(&on_disk, anchor.line, last_line, CONTEXT_LINES);
    let language = fence_language(&path);
    let sections = vec![
        Section {
            heading: format!("### `{relative}:{}` — `old` substituído", anchor.line),
            body: format!(
                "\n- âncora: {old_lines} linha(s) na linha {}\n- no lugar: {new_lines} linha(s)\n- arquivo: blake3 `{}` (antes `{old_prefix}`)\n",
                anchor.line,
                hash_prefix(on_disk.as_bytes())
            ),
            priority: 255,
        },
        Section {
            heading: format!(
                "#### `{relative}:{first}-{}`",
                last_line.saturating_add(CONTEXT_LINES)
            ),
            body: format!("```{language}\n{context}```\n"),
            priority: 254,
        },
    ];

    let budget_tokens = params.budget_tokens.unwrap_or(default_budget_tokens);
    let selection = budget::select(&sections, budget_tokens);
    let handle = if selection.omitted.is_empty() {
        None
    } else {
        Some(store.insert(selection.omitted_body()))
    };
    Ok(budget::render(&selection, handle.as_deref()))
}
