pub mod context;
pub mod memory;
pub mod sdd;
pub mod tokens;
mod usage;

pub use usage::Usage;

mod workflow;

pub use workflow::{WORKFLOWS_PREFIX, Workflow, WorkflowSource, parse_workflow_invocation};

pub type SessionId = u64;
pub type RequestId = u64;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Role {
    System,
    User,
    Assistant,
    Tool,
}

impl Role {
    pub fn as_str(self) -> &'static str {
        match self {
            Role::System => "system",
            Role::User => "user",
            Role::Assistant => "assistant",
            Role::Tool => "tool",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: String,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Message {
    pub role: Role,
    pub content: String,
    pub tool_calls: Vec<ToolCall>,
    pub tool_call_id: Option<String>,
    /// Chain-of-thought devolvido pelo provedor em thinking mode. O DeepSeek
    /// exige que ele volte no corpo da requisição quando há `tools`.
    pub reasoning_content: Option<String>,
}

impl Message {
    pub fn new(role: Role, content: impl Into<String>) -> Self {
        Self {
            role,
            content: content.into(),
            tool_calls: Vec::new(),
            tool_call_id: None,
            reasoning_content: None,
        }
    }

    pub fn user(content: impl Into<String>) -> Self {
        Self::new(Role::User, content)
    }

    pub fn assistant(content: impl Into<String>) -> Self {
        Self::new(Role::Assistant, content)
    }

    pub fn assistant_with_calls(content: impl Into<String>, tool_calls: Vec<ToolCall>) -> Self {
        Self {
            tool_calls,
            ..Self::assistant(content)
        }
    }

    pub fn tool(call_id: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            tool_call_id: Some(call_id.into()),
            ..Self::new(Role::Tool, content)
        }
    }

    /// Anexa o reasoning do provedor. Vazio é aceito mas não é enviado de volta.
    pub fn with_reasoning(mut self, reasoning: impl Into<String>) -> Self {
        self.reasoning_content = Some(reasoning.into());
        self
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Session {
    pub id: SessionId,
    pub messages: Vec<Message>,
}

impl Session {
    pub fn new(id: SessionId) -> Self {
        Self {
            id,
            messages: Vec::new(),
        }
    }

    pub fn push(&mut self, role: Role, content: impl Into<String>) {
        self.messages.push(Message::new(role, content));
    }

    pub fn push_message(&mut self, message: Message) {
        self.messages.push(message);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub enum AgentMode {
    #[default]
    Execution,
    Sdd,
    Docs,
}

impl AgentMode {
    pub fn as_u8(self) -> u8 {
        match self {
            Self::Execution => 0,
            Self::Sdd => 1,
            Self::Docs => 2,
        }
    }

    pub fn from_u8(raw: u8) -> Self {
        match raw {
            1 => Self::Sdd,
            2 => Self::Docs,
            _ => Self::Execution,
        }
    }
}

/// Teto de caracteres de `Event::ToolFinished::detail`.
///
/// O transcript da TUI é reconstruído a cada frame e rola por linhas:
/// despejar o output inteiro de uma tool custaria o frame. O número vive
/// aqui, junto do evento, para que quem publica e quem desenha não inventem
/// tetos diferentes.
pub const TOOL_DETAIL_MAX_CHARS: usize = 4_000;

/// Excerto de um texto para `Event::ToolFinished::detail`: cabeça + cauda
/// separadas por um marcador explícito de corte.
///
/// Texto que cabe no teto volta inteiro. O que não cabe volta com no máximo
/// [`TOOL_DETAIL_MAX_CHARS`] caracteres e com o marcador dizendo quantos
/// ficaram de fora — um detalhe cortado nunca se passa por completo, e um
/// texto não vazio nunca encolhe sem aviso. O corte segue o mesmo critério de
/// `crate::telemetry::clip`: nunca parte um `char` no meio, então o resultado
/// é sempre UTF-8 válido.
/// Consumidores externos dependem desta função pública (`Event::detail`).
pub fn tool_detail_excerpt(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    if chars.len() <= TOOL_DETAIL_MAX_CHARS {
        return text.to_string();
    }

    let marker = format!(
        "\n... ({} caracteres omitidos) ...\n",
        chars.len() - TOOL_DETAIL_MAX_CHARS
    );
    // A cabeça absorve o custo do marcador: o total nunca passa do teto.
    let tail = TOOL_DETAIL_MAX_CHARS / 4;
    let head = TOOL_DETAIL_MAX_CHARS.saturating_sub(tail + marker.chars().count());

    let mut out: String = chars[..head].iter().collect();
    out.push_str(&marker);
    out.extend(&chars[chars.len() - tail..]);
    out
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    Token {
        request_id: RequestId,
        text: String,
    },
    StreamFinished {
        request_id: RequestId,
        prompt_tokens: Option<u64>,
        cached_tokens: Option<u64>,
        completion_tokens: Option<u64>,
    },
    /// Ocupação atual da janela de contexto observada no último passo.
    ContextUpdated {
        request_id: RequestId,
        tokens: u64,
        budget: u64,
    },
    /// Novo teto de contexto depois de trocar de modelo. Não traz ocupação:
    /// só o budget muda, sem inventar tokens.
    ContextBudget {
        budget: u64,
    },
    StreamFailed {
        request_id: RequestId,
        message: String,
    },
    /// O histórico enviado ao modelo foi podado para caber no orçamento.
    ContextTrimmed {
        request_id: RequestId,
        estimate: usize,
        omitted_units: usize,
        shrunk_results: usize,
    },
    PromptMessages {
        request_id: RequestId,
        body: String,
    },
    ModelsLoaded {
        models: Vec<String>,
    },
    ModelsFailed {
        message: String,
    },
    Notice {
        message: String,
    },
    ToolStarted {
        request_id: RequestId,
        call_id: String,
        name: String,
        arguments: String,
        summary: String,
    },
    ToolFinished {
        request_id: RequestId,
        call_id: String,
        name: String,
        ok: bool,
        summary: String,
        /// Excerto do corpo do resultado, cortado por [`tool_detail_excerpt`]
        /// em no máximo [`TOOL_DETAIL_MAX_CHARS`] caracteres: cabeça + cauda
        /// com marcador explícito de corte.
        ///
        /// Chega inteiro (ou truncado) de uma vez: `ToolFinished` é evento
        /// único, não há streaming do output. Quem desenha não busca conteúdo
        /// por conta própria nem inventa o que falta.
        detail: String,
        /// Tempo de execução da tool em **microssegundos**: é a unidade que a
        /// telemetria da tool já usa, e micros inteiros evitam ter de
        /// serializar `std::time::Duration`. O consumidor escolhe a unidade
        /// final (`Duration::from_micros`). Só o executor da tool pode medir
        /// isso: derivar de `Instant` no consumidor daria errado, porque o
        /// evento atravessa o bus assíncrono e pode ser lido bem depois.
        duration_us: u64,
    },
    /// Linha discreta de memória: recall injetado ou fato gravado.
    Memory {
        text: String,
        /// true: entra no fim do turno; antes da resposta.
        after: bool,
    },
    ModeChanged {
        mode: AgentMode,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn message_constructors_keep_tool_metadata() {
        let call = ToolCall {
            id: "c1".into(),
            name: "read".into(),
            arguments: "{}".into(),
        };
        let assistant = Message::assistant_with_calls("", vec![call.clone()]);
        assert_eq!(assistant.role, Role::Assistant);
        assert_eq!(assistant.tool_calls, vec![call]);

        let result = Message::tool("c1", "conteudo");
        assert_eq!(result.role, Role::Tool);
        assert_eq!(result.tool_call_id.as_deref(), Some("c1"));
        assert!(result.tool_calls.is_empty());
    }

    #[test]
    fn tool_role_serializes_as_tool() {
        assert_eq!(Role::Tool.as_str(), "tool");
    }

    #[test]
    fn push_keeps_messages_in_order() {
        let mut session = Session::new(1);
        session.push(Role::User, "oi");
        session.push(Role::Assistant, "olá");

        assert_eq!(session.messages[0], Message::user("oi"));
        assert_eq!(session.messages[1].role, Role::Assistant);
    }

    #[test]
    fn docs_mode_round_trips_through_u8() {
        assert_eq!(AgentMode::Docs.as_u8(), 2);
        assert_eq!(AgentMode::from_u8(2), AgentMode::Docs);
        assert_eq!(AgentMode::from_u8(9), AgentMode::Execution);
    }
}
