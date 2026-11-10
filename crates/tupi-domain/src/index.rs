use serde::{Deserialize, Serialize};

/// Instantâneo do repositório no início da sessão.
///
/// O índice é um mapa: caminho, hash e símbolos. Nunca conteúdo.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Index {
    pub version: u32,
    pub scanned_at_unix: u64,
    pub files: Vec<IndexFile>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IndexFile {
    pub path: String,
    pub hash: String,
    pub language: String,
    pub symbols: Vec<IndexSymbol>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IndexSymbol {
    pub name: String,
    pub qualified: String,
    pub signature: String,
    pub start_line: usize,
    pub end_line: usize,
}
