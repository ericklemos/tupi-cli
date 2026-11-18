//! Âncora viva em documento: `caminho:linha` conferida contra o disco.
//!
//! A prosa que cita código carrega o único controle objetivo que ela tem: o
//! endereço `caminho:linha`. Aqui cada âncora encontrada em `docs/**/*.md` é
//! resolvida contra o arquivo atual e recebe um veredito — viva, arquivo
//! ausente, linha que não existe ou arquivo que mudou. O formato opcional
//! `caminho:linha@blake3` acrescenta a prova de identidade: sem ele a âncora
//! só prova que a linha existe hoje, com ele prova que o arquivo é o mesmo.
//!
//! O check não julga estilo nem interpreta a linguagem do arquivo citado: lê
//! bytes, conta linhas e compara hash. Por isso vale para md, rs, ts, py, sql
//! ou qualquer texto, e nunca depende de parser.

use std::path::{Path, PathBuf};

/// Uma âncora `caminho:linha` (hash opcional) como escrita no documento.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Anchor {
    pub path: String,
    pub line: usize,
    /// Prefixo blake3 declarado depois de `@`, já em minúsculas.
    pub hash: Option<String>,
}

/// Por que a âncora está morta.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reason {
    /// O arquivo citado não existe na raiz do projeto.
    MissingFile,
    /// O arquivo existe mas não pôde ser lido como bytes.
    Unreadable(String),
    /// A linha citada está fora do arquivo.
    LineOutOfRange { lines: usize },
    /// O hash declarado não é o hash atual do arquivo.
    Changed { expected: String, actual: String },
}

impl Reason {
    /// Motivo em uma linha, no vocabulário da recusa que ensina.
    pub fn describe(&self) -> String {
        match self {
            Reason::MissingFile => "arquivo não existe".to_string(),
            Reason::Unreadable(error) => format!("arquivo ilegível: {error}"),
            Reason::LineOutOfRange { lines } => {
                format!("linha não existe (arquivo tem {lines} linha(s))")
            }
            Reason::Changed { expected, actual } => format!(
                "arquivo mudou (blake3 citado `{expected}`, atual `{actual}`)"
            ),
        }
    }
}

/// Âncora morta: onde foi citada e por quê.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeadAnchor {
    /// Documento que contém a citação, relativo à raiz.
    pub doc: PathBuf,
    /// Linha do documento onde a citação aparece (1-based).
    pub doc_line: usize,
    pub anchor: Anchor,
    pub reason: Reason,
}

/// Resultado do check: quantas âncoras foram vistas e quais morreram.
#[derive(Debug, Default)]
pub struct Report {
    pub anchors: usize,
    pub dead: Vec<DeadAnchor>,
}

impl Report {
    pub fn is_clean(&self) -> bool {
        self.dead.is_empty()
    }

    /// Lista das âncoras mortas, uma por linha, com documento, citação e motivo.
    pub fn render(&self) -> String {
        let mut out = String::new();
        for dead in &self.dead {
            out.push_str(&format!(
                "{}:{} -> {}:{}: {}\n",
                dead.doc.display(),
                dead.doc_line,
                dead.anchor.path,
                dead.anchor.line,
                dead.reason.describe()
            ));
        }
        if self.dead.is_empty() {
            out.push_str(&format!(
                "âncoras vivas: {} (nenhuma morta)\n",
                self.anchors
            ));
        } else {
            out.push_str(&format!(
                "âncoras mortas: {} de {}\n",
                self.dead.len(),
                self.anchors
            ));
        }
        out
    }
}

/// Confere todas as âncoras de `docs/**/*.md` contra a raiz do projeto.
pub fn check(root: &Path) -> std::io::Result<Report> {
    let mut docs = Vec::new();
    collect_markdown(&root.join("docs"), Path::new("docs"), &mut docs)?;
    docs.sort();
    check_files(root, &docs)
}

/// Confere as âncoras dos documentos dados (caminhos relativos à raiz).
pub fn check_files(root: &Path, docs: &[PathBuf]) -> std::io::Result<Report> {
    let mut report = Report::default();
    for doc in docs {
        let text = match std::fs::read_to_string(root.join(doc)) {
            Ok(text) => text,
            Err(error) => {
                return Err(std::io::Error::new(
                    error.kind(),
                    format!("falha ao ler `{}`: {error}", doc.display()),
                ));
            }
        };
        for (index, line) in text.lines().enumerate() {
            for anchor in parse_anchors(line) {
                report.anchors += 1;
                if let Some(reason) = verify(root, &anchor) {
                    report.dead.push(DeadAnchor {
                        doc: doc.clone(),
                        doc_line: index + 1,
                        anchor,
                        reason,
                    });
                }
            }
        }
    }
    Ok(report)
}

/// Extrai as âncoras `caminho:linha` e `caminho:linha@hash` de um texto.
///
/// Um caminho é uma sequência de letras, dígitos, `_`, `.`, `-` e `/` que
/// começa em letra/`_`/`.` e contém `/` ou `.`; vem seguido de `:`, um número
/// de linha (aceitando `:12-34`) e, opcionalmente, `@` e um prefixo hex.
pub fn parse_anchors(text: &str) -> Vec<Anchor> {
    let bytes = text.as_bytes();
    let mut anchors = Vec::new();
    let mut index = 0;
    while index < bytes.len() {
        if !is_path_char(bytes[index]) {
            index += 1;
            continue;
        }
        let start = index;
        while index < bytes.len() && is_path_char(bytes[index]) {
            index += 1;
        }
        let path = &text[start..index];
        if !looks_like_path(path) {
            continue;
        }
        if bytes.get(index) != Some(&b':') {
            continue;
        }
        index += 1;
        let Some(line) = parse_number(bytes, &mut index) else {
            continue;
        };
        // `:12-34` cita a faixa; a âncora é a primeira linha.
        if bytes.get(index) == Some(&b'-') {
            let mut probe = index + 1;
            if parse_number(bytes, &mut probe).is_some() {
                index = probe;
            }
        }
        let mut hash = None;
        if bytes.get(index) == Some(&b'@') {
            let mut probe = index + 1;
            let hash_start = probe;
            while probe < bytes.len() && bytes[probe].is_ascii_hexdigit() {
                probe += 1;
            }
            if probe > hash_start {
                hash = Some(text[hash_start..probe].to_ascii_lowercase());
                index = probe;
            }
        }
        anchors.push(Anchor {
            path: path.to_string(),
            line,
            hash,
        });
    }
    anchors
}

/// Veredito de uma âncora: `None` quando está viva.
fn verify(root: &Path, anchor: &Anchor) -> Option<Reason> {
    let path = root.join(&anchor.path);
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Some(Reason::MissingFile);
        }
        Err(error) => return Some(Reason::Unreadable(error.to_string())),
    };
    let lines = String::from_utf8_lossy(&bytes).lines().count();
    if anchor.line == 0 || anchor.line > lines {
        return Some(Reason::LineOutOfRange { lines });
    }
    if let Some(expected) = &anchor.hash {
        let actual = blake3::hash(&bytes).to_hex().to_string();
        if !actual.starts_with(expected) {
            return Some(Reason::Changed {
                expected: expected.clone(),
                actual,
            });
        }
    }
    None
}

fn collect_markdown(
    dir: &Path,
    relative: &Path,
    found: &mut Vec<PathBuf>,
) -> std::io::Result<()> {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        let child = relative.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            collect_markdown(&path, &child, found)?;
        } else if path.extension().is_some_and(|extension| extension == "md") {
            found.push(child);
        }
    }
    Ok(())
}

fn is_path_char(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'.' | b'-' | b'/')
}

fn looks_like_path(path: &str) -> bool {
    let Some(first) = path.chars().next() else {
        return false;
    };
    if !(first.is_ascii_alphabetic() || first == '_' || first == '.') {
        return false;
    }
    path.contains('/') || path.contains('.')
}

fn parse_number(bytes: &[u8], index: &mut usize) -> Option<usize> {
    let start = *index;
    let mut value = 0usize;
    while let Some(byte) = bytes.get(*index) {
        if !byte.is_ascii_digit() {
            break;
        }
        value = value.saturating_mul(10).saturating_add((byte - b'0') as usize);
        *index += 1;
    }
    if *index == start { None } else { Some(value) }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "tupi-anchors-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(root.join("docs")).unwrap();
        root
    }

    #[test]
    fn a_bare_path_and_line_are_an_anchor_and_a_bare_word_is_not() {
        let anchors = parse_anchors("veja `crates/a.rs:12-14`, `docs/x.md:3` e `a::b` 10:30");
        assert_eq!(anchors.len(), 2);
        assert_eq!(anchors[0].path, "crates/a.rs");
        assert_eq!(anchors[0].line, 12);
        assert_eq!(anchors[0].hash, None);
        assert_eq!(anchors[1].path, "docs/x.md");
        assert_eq!(anchors[1].line, 3);
    }

    #[test]
    fn an_at_suffix_carries_the_declared_hash() {
        let anchors = parse_anchors("agora `crates/a.rs:9@ABCDEF`");
        assert_eq!(anchors.len(), 1);
        assert_eq!(anchors[0].hash.as_deref(), Some("abcdef"));
    }

    #[test]
    fn the_check_names_missing_file_dead_line_and_changed_file() {
        let root = temp_root("dead");
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("src/live.rs"), "fn main() {}\n").unwrap();
        std::fs::write(
            root.join("docs/guia.md"),
            "# Guia\n\n\
             viva `src/live.rs:1`\n\
             linha morta `src/live.rs:9`\n\
             arquivo sumido `src/ghost.rs:1`\n\
             hash velho `src/live.rs:1@deadbeef`\n",
        )
        .unwrap();

        let report = check(&root).unwrap();
        assert_eq!(report.anchors, 4);
        assert_eq!(report.dead.len(), 3);
        assert_eq!(report.dead[0].reason, Reason::LineOutOfRange { lines: 1 });
        assert_eq!(report.dead[1].reason, Reason::MissingFile);
        assert!(
            matches!(&report.dead[2].reason, Reason::Changed { expected, .. } if expected == "deadbeef")
        );
        let rendered = report.render();
        assert!(rendered.contains("docs/guia.md:4 -> src/live.rs:9: linha não existe"));
        assert!(rendered.contains("docs/guia.md:5 -> src/ghost.rs:1: arquivo não existe"));
        assert!(rendered.contains("docs/guia.md:6 -> src/live.rs:1: arquivo mudou"));
        assert!(rendered.contains("âncoras mortas: 3 de 4"));
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn a_matching_hash_keeps_the_anchor_alive() {
        let root = temp_root("alive");
        std::fs::create_dir_all(root.join("src")).unwrap();
        let bytes = b"fn main() {}\n";
        std::fs::write(root.join("src/live.rs"), bytes).unwrap();
        let hash = blake3::hash(bytes).to_hex().to_string();
        std::fs::write(
            root.join("docs/guia.md"),
            format!("viva `src/live.rs:1@{hash}`\n"),
        )
        .unwrap();

        let report = check(&root).unwrap();
        assert_eq!(report.anchors, 1);
        assert!(report.is_clean(), "{}", report.render());
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn this_repo_lista_as_ancoras_mortas_de_docs() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let report = check(&root).expect("docs/ do repositório é legível");
        println!("{}", report.render());
        assert!(
            report.anchors > 0,
            "nenhuma âncora `caminho:linha` encontrada em docs/"
        );
    }
}
