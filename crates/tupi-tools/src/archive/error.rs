//! Erros do acervo do modo docs.

use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum ArchiveError {
    #[error("acervo já aberto por outro processo: {0}")]
    Locked(PathBuf),
    #[error("banco do acervo: {0}")]
    Db(String),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("ferramenta externa ausente: {0}")]
    MissingTool(String),
    #[error("contrato de saída violado: {0}")]
    Contract(String),
    #[error("não encontrado: {0}")]
    NotFound(String),
}
