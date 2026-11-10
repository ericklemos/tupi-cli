pub mod context;
pub mod tokens;
mod usage;

pub use usage::Usage;

mod workflow;

pub use workflow::{WORKFLOWS_PREFIX, Workflow, WorkflowSource, parse_workflow_invocation};

pub type SessionId = u64;
pub type RequestId = u64;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Message {
    pub role: Role,
    pub content: String,
    pub tool_calls: Vec<ToolCall>,
    pub tool_call_id: Option<String>,
}

impl Message {
    pub fn new(role: Role, content: impl Into<String>) -> Self {
        Self {
            role,
            content: content.into(),
            tool_calls: Vec::new(),
            tool_call_id: None,
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
}
