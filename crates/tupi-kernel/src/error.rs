#[derive(Debug, thiserror::Error)]
pub enum ProviderError {
    #[error("falha de rede: {0}")]
    Network(String),
    #[error("erro HTTP {status}: {body}")]
    Http { status: u16, body: String },
    #[error("resposta fora do formato esperado: {0}")]
    Protocol(String),
}

#[derive(Debug, thiserror::Error)]
pub enum CommandError {
    #[error("já existe um stream em andamento")]
    Busy,
    #[error("requisição desconhecida")]
    UnknownRequest,
    #[error(transparent)]
    Provider(#[from] ProviderError),
    #[error("erro interno: {0}")]
    Internal(String),
}

#[derive(Debug, thiserror::Error)]
pub enum QueryError {
    #[error(transparent)]
    Provider(#[from] ProviderError),
    #[error("erro interno: {0}")]
    Internal(String),
}
