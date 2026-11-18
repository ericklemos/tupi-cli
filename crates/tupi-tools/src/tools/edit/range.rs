//! Rota faixa de linhas do `edit`: o endereço é a linha e o texto esperado é a
//! guarda.
//!
//! Diff exigiria aritmética de número de linha no `@@`; a faixa não: é
//! `start`+`end` contados a partir de 1, inclusivos, mais `expect` — o texto
//! atual da faixa, copiado de `read`/`lookup lines` sem a numeração. Sem
//! expectativa a escrita seria às cegas, e faixa sem expectativa é recusa antes
//! de tocar no disco (`EditParams::lines`).
//!
//! O contrato de guarda é o da rota literal e o da rota AST: `expected_hash` do
//! arquivo inteiro antes de tudo, conferência do trecho em disco contra
//! `expect`, releitura imediata antes do rename e restauração do original se o
//! disco divergir depois da escrita. Nenhum parser participa: vale para
//! qualquer arquivo UTF-8, que é exatamente o ponto da rota — linhas repetidas e
//! arquivos grandes, onde a âncora literal é desconfortável.

use std::path::Path;

use tupi_kernel::ToolError;

use super::literal::{fence_language, window};
use super::params::EditParams;
use crate::budget::{self, Section};
use crate::paths::{atomic_write, resolve_for_write};
use crate::span::{hash_matches, hash_prefix};
use crate::store::ResponseStore;

/// Linhas do trecho em disco mostradas na recusa antes de resumir o resto.
const MAX_ACTUAL_LINES_SHOWN: usize = 12;

/// Linhas de contexto devolvidas depois da substituição.
const CONTEXT_LINES: usize = 3;

/// Linhas do arquivo, cada uma com a quebra preservada (`split_inclusive`), que é
/// o que permite recortar por byte.
fn lines_of(content: &str) -> Vec<&str> {
    content.split_inclusive('\n').collect()
}

/// Linha sem a quebra e sem o `\r` do CRLF.
fn bare(line: &str) -> &str {
    line.strip_suffix('\n')
        .unwrap_or(line)
        .strip_suffix('\r')
        .unwrap_or_else(|| line.strip_suffix('\n').unwrap_or(line))
}

/// Texto atual de `start..=end` no formato de `expect`: sem numeração e sem a
/// quebra de linha final. `None` quando a faixa não cabe no arquivo.
pub fn section_text(content: &str, start: usize, end: usize) -> Option<String> {
    let lines = lines_of(content);
    if start == 0 || start > end || end > lines.len() {
        return None;
    }
    let mut text = String::new();
    for (index, line) in lines[start - 1..end].iter().enumerate() {
        if index > 0 {
            text.push('\n');
        }
        text.push_str(bare(line));
    }
    Some(text)
}

/// Deslocamentos em bytes do começo e do fim de `start..=end`, com a quebra da
/// última linha da faixa quando ela existe.
fn section_offsets(content: &str, start: usize, end: usize) -> Option<(usize, usize)> {
    let lines = lines_of(content);
    if start == 0 || start > end || end > lines.len() {
        return None;
    }
    let begin: usize = lines[..start - 1].iter().map(|line| line.len()).sum();
    let finish: usize = lines[..end].iter().map(|line| line.len()).sum();
    Some((begin, finish))
}

/// A faixa em disco precisa ser exatamente `expect`. Divergência é recusa com o
/// trecho atual: a tool não corrige a expectativa do chamador.
pub fn verify(
    content: &str,
    start: usize,
    end: usize,
    expect: &str,
    path: &str,
) -> Result<(), ToolError> {
    let Some(actual) = section_text(content, start, end) else {
        return Err(ToolError::Other(format!(
            "`{path}:{start}-{end}` não existe: o arquivo tem {} linha(s). \
             A faixa é inclusiva e contada a partir de 1. Nada foi escrito.",
            lines_of(content).len()
        )));
    };
    if actual == expect {
        return Ok(());
    }
    let shown: Vec<&str> = actual.lines().take(MAX_ACTUAL_LINES_SHOWN).collect();
    let listed = shown.join("\n");
    let rest = actual
        .lines()
        .count()
        .saturating_sub(MAX_ACTUAL_LINES_SHOWN);
    let extra = if rest > 0 {
        format!("\n… +{rest} linha(s)")
    } else {
        String::new()
    };
    Err(ToolError::Other(format!(
        "`expect` não é o texto atual de `{path}:{start}-{end}`. Em disco:\n```\n{listed}{extra}\n```\n\
         Copie o trecho da faixa inteira, sem a numeração e sem a quebra final, ou corrija `start`/`end`. \
         Nada foi escrito."
    )))
}

/// Substituição por faixa de linhas: `start..=end` sai, `new` entra, com o mesmo
/// contrato de guarda das rotas literal e AST. Sem parser e sem escape.
pub fn edit_range(
    root_dir: &Path,
    store: &ResponseStore,
    default_budget_tokens: usize,
    params: &EditParams,
    start: usize,
    end: usize,
    expect: &str,
    new: &str,
) -> Result<String, ToolError> {
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
        // Faixa não tem "já aplicada" verificável com segurança: hash velho nunca
        // vira passe livre, e o trecho esperado é a única pista.
        return Err(ToolError::StaleHash(format!(
            "hash atual: `{old_prefix}`. Reenvie com expected_hash atualizado — faixa deslocada não é aplicada"
        )));
    }

    verify(&content, start, end, expect, &params.path)?;
    let (begin, finish) = section_offsets(&content, start, end)
        .ok_or_else(|| ToolError::Other("faixa inconsistente: nada foi escrito".to_string()))?;

    let mut new_content = String::with_capacity(content.len() + new.len());
    new_content.push_str(&content[..begin]);
    new_content.push_str(new);
    // `new` sem quebra final no meio do arquivo precisa da quebra, senão a última
    // linha nova cola na primeira linha que vem depois da faixa.
    if !new.is_empty() && !new.ends_with('\n') && finish < content.len() {
        new_content.push('\n');
    }
    new_content.push_str(&content[finish..]);

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
        start + newlines.saturating_sub(1)
    } else {
        start + newlines
    };
    let old_lines = end - start + 1;
    let new_lines = if new.is_empty() {
        0
    } else {
        last_line - start + 1
    };
    let (first, context) = window(&on_disk, start, last_line, CONTEXT_LINES);
    let language = fence_language(&path);
    let sections = vec![
        Section {
            heading: format!("### `{relative}:{start}-{end}` — faixa substituída"),
            body: format!(
                "\n- faixa que saiu: {old_lines} linha(s) em {start}..={end}\n- no lugar: {new_lines} linha(s)\n- arquivo: blake3 `{}` (antes `{old_prefix}`)\n",
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
