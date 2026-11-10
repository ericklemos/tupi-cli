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
}
