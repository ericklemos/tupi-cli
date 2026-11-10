pub type SessionId = u64;
pub type RequestId = u64;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Role {
    System,
    User,
    Assistant,
}

impl Role {
    pub fn as_str(self) -> &'static str {
        match self {
            Role::System => "system",
            Role::User => "user",
            Role::Assistant => "assistant",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Message {
    pub role: Role,
    pub content: String,
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
        self.messages.push(Message {
            role,
            content: content.into(),
        });
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
    },
    StreamFailed {
        request_id: RequestId,
        message: String,
    },
    ModelsLoaded {
        models: Vec<String>,
    },
    ModelsFailed {
        message: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn push_keeps_messages_in_order() {
        let mut session = Session::new(1);
        session.push(Role::User, "oi");
        session.push(Role::Assistant, "olá");

        assert_eq!(
            session.messages[0],
            Message {
                role: Role::User,
                content: "oi".into()
            }
        );
        assert_eq!(session.messages[1].role, Role::Assistant);
    }
}
