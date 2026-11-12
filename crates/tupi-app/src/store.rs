use std::sync::Mutex;

use tupi_domain::{Message, Role, Session, SessionId};

pub struct SessionStore {
    inner: Mutex<Session>,
}

impl SessionStore {
    pub fn new(id: SessionId) -> Self {
        Self {
            inner: Mutex::new(Session::new(id)),
        }
    }

    pub fn push(&self, role: Role, content: impl Into<String>) {
        self.inner.lock().unwrap().push(role, content);
    }

    pub fn push_message(&self, message: Message) {
        self.inner.lock().unwrap().messages.push(message);
    }

    pub fn push_assistant_once(&self, content: &str) {
        let mut session = self.inner.lock().unwrap();
        let duplicate = session
            .messages
            .last()
            .map(|message| message.role == Role::Assistant && message.content == content)
            .unwrap_or(false);

        if !duplicate {
            session.push(Role::Assistant, content);
        }
    }

    pub fn messages(&self) -> Vec<Message> {
        self.inner.lock().unwrap().messages.clone()
    }

    pub fn replace_messages(&self, messages: Vec<Message>) {
        self.inner.lock().unwrap().messages = messages;
    }

    pub fn snapshot(&self) -> Session {
        self.inner.lock().unwrap().clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn push_assistant_once_ignores_duplicate() {
        let store = SessionStore::new(1);
        store.push(Role::User, "oi");
        store.push_assistant_once("parcial");
        store.push_assistant_once("parcial");

        assert_eq!(store.messages().len(), 2);
    }

    #[test]
    fn replace_messages_overwrites_the_session_history() {
        let store = SessionStore::new(1);
        store.push(Role::User, "um");
        store.push(Role::Assistant, "dois");

        store.replace_messages(vec![Message::user("compactado")]);

        let messages = store.messages();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].content, "compactado");
    }
}
