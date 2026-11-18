#[derive(Debug, thiserror::Error)]
pub enum ProviderError {
    #[error("falha de rede: {0}")]
    Network(String),
    #[error("erro HTTP {status}: {body}")]
    Http { status: u16, body: String },
    #[error("resposta fora do formato esperado: {0}")]
    Protocol(String),
    #[error("limite de requisições (HTTP {status}): {message}")]
    RateLimited {
        status: u16,
        retry_after: Option<u64>,
        message: String,
        body: String,
    },
    #[error("quota esgotada (HTTP {status}): {message}")]
    QuotaExhausted {
        status: u16,
        reset: Option<u64>,
        message: String,
        body: String,
    },
    #[error("requisição inválida (HTTP {status}): {message}")]
    InvalidArgument {
        status: u16,
        message: String,
        body: String,
    },
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
