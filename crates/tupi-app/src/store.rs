use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tupi_domain::{AgentMode, Message, Role, Session, SessionId, ToolCall};

use crate::sessions::{
    SessionArchive, SessionLock, SessionMeta, SessionRecord, UNTITLED, new_session_id, now_unix,
    session_title,
};

pub struct SessionStore {
    inner: Mutex<Session>,
    archive: Mutex<Option<Arc<dyn SessionArchive>>>,
    current: Mutex<Option<SessionMeta>>,
    lock: Mutex<Option<SessionLock>>,
    pending_calls: Mutex<HashMap<String, ToolCall>>,
}

impl SessionStore {
    pub fn new(id: SessionId) -> Self {
        Self {
            inner: Mutex::new(Session::new(id)),
            archive: Mutex::new(None),
            current: Mutex::new(None),
            lock: Mutex::new(None),
            pending_calls: Mutex::new(HashMap::new()),
        }
    }

    pub fn with_archive(self, archive: Arc<dyn SessionArchive>) -> Self {
        self.attach_archive(archive);
        self
    }

    pub fn attach_archive(&self, archive: Arc<dyn SessionArchive>) {
        *self.archive.lock().unwrap() = Some(archive);
    }

    fn archive(&self) -> Option<Arc<dyn SessionArchive>> {
        self.archive.lock().unwrap().clone()
    }

    fn ensure_session(
        &self,
        text: &str,
        mode: AgentMode,
        provider: &str,
        model: &str,
    ) -> Result<(), String> {
        let Some(archive) = self.archive() else {
            return Ok(());
        };
        let mut current = self.current.lock().unwrap();
        if current.is_some() {
            return Ok(());
        }

        let now = now_unix();
        let meta = SessionMeta {
            id: new_session_id(),
            repo_key: archive.repo_key().to_string(),
            title: session_title(text),
            created_at: now,
            updated_at: now,
            message_count: 0,
            mode,
            provider: provider.to_string(),
            model: model.to_string(),
        };
        let lock = archive.create(&meta)?;
        *current = Some(meta);
        *self.lock.lock().unwrap() = Some(lock);
        Ok(())
    }

    fn ensure_any_session(&self) -> Result<(), String> {
        self.ensure_session(UNTITLED, AgentMode::Execution, "", "")
    }

    fn append_only(&self, record: SessionRecord) -> Result<(), String> {
        let Some(archive) = self.archive() else {
            return Ok(());
        };
        let Some(meta) = self.current.lock().unwrap().clone() else {
            return Ok(());
        };
        archive.append(&meta.id, &record)
    }

    fn append(&self, record: SessionRecord) -> Result<(), String> {
        self.append_only(record)?;
        let Some(archive) = self.archive() else {
            return Ok(());
        };
        let mut current = self.current.lock().unwrap();
        let Some(meta) = current.as_mut() else {
            return Ok(());
        };
        meta.message_count += 1;
        meta.updated_at = now_unix();
        archive.update_meta(meta)
    }

    pub fn record_user(
        &self,
        text: String,
        mode: AgentMode,
        provider: &str,
        model: &str,
    ) -> Result<(), String> {
        let ensured = self.ensure_session(&text, mode, provider, model);
        let title = session_title(&text);
        if let Some(meta) = self.current.lock().unwrap().as_mut()
            && meta.title == UNTITLED
        {
            meta.title = title;
        }

        let message = Message::user(text);
        self.inner.lock().unwrap().push_message(message.clone());
        let appended = self.append(SessionRecord::user(now_unix(), message));
        ensured.and(appended)
    }

    pub fn record_assistant(&self, message: Message) -> Result<(), String> {
        let ensured = self.ensure_any_session();
        {
            let mut pending = self.pending_calls.lock().unwrap();
            for call in &message.tool_calls {
                pending.insert(call.id.clone(), call.clone());
            }
        }
        self.inner.lock().unwrap().push_message(message.clone());
        let appended = self.append(SessionRecord::assistant(now_unix(), message));
        ensured.and(appended)
    }

    pub fn record_message(&self, message: Message) -> Result<(), String> {
        match message.role {
            Role::Assistant => self.record_assistant(message),
            Role::System => self.record_system(message),
            Role::User => {
                let text = message.content.clone();
                self.record_user(text, AgentMode::Execution, "", "")
            }
            Role::Tool => {
                let call = message
                    .tool_call_id
                    .as_ref()
                    .and_then(|id| self.pending_calls.lock().unwrap().get(id).cloned())
                    .unwrap_or_else(|| ToolCall {
                        id: message.tool_call_id.clone().unwrap_or_default(),
                        name: "tool".to_string(),
                        arguments: String::new(),
                    });
                let summary = crate::app::summarize(&message.content);
                self.record_tool(message, &call, &summary, true, Duration::ZERO)
            }
        }
    }
    pub fn record_assistant_once(&self, content: &str) -> Result<(), String> {
        {
            let session = self.inner.lock().unwrap();
            let duplicate = session
                .messages
                .last()
                .map(|message| message.role == Role::Assistant && message.content == content)
                .unwrap_or(false);
            if duplicate {
                return Ok(());
            }
        }
        self.record_assistant(Message::assistant(content))
    }

    pub fn record_system(&self, message: Message) -> Result<(), String> {
        let ensured = self.ensure_any_session();
        self.inner.lock().unwrap().push_message(message.clone());
        let appended = self.append(SessionRecord::system(now_unix(), message));
        ensured.and(appended)
    }

    pub fn record_synthetic(&self, message: Message) -> Result<(), String> {
        let ensured = self.ensure_any_session();
        self.inner.lock().unwrap().push_message(message.clone());
        let appended = self.append(SessionRecord::synthetic(now_unix(), message));
        ensured.and(appended)
    }

    pub fn record_tool(
        &self,
        message: Message,
        call: &ToolCall,
        summary: &str,
        ok: bool,
        duration: Duration,
    ) -> Result<(), String> {
        let ensured = self.ensure_any_session();
        self.inner.lock().unwrap().push_message(message.clone());
        let appended = self.append(SessionRecord::tool(
            now_unix(),
            message,
            &call.id,
            &call.name,
            summary,
            ok,
            duration.as_micros() as u64,
        ));
        ensured.and(appended)
    }

    pub fn record_checkpoint(&self, messages: Vec<Message>) -> Result<(), String> {
        self.replace_messages(messages.clone());
        self.append_only(SessionRecord::checkpoint(now_unix(), messages))
    }

    pub fn touch(&self, mode: AgentMode) -> Result<(), String> {
        let Some(archive) = self.archive() else {
            return Ok(());
        };
        let mut current = self.current.lock().unwrap();
        let Some(meta) = current.as_mut() else {
            return Ok(());
        };
        meta.updated_at = now_unix();
        meta.mode = mode;
        archive.update_meta(meta)
    }

    pub fn resume(
        &self,
        _id: &str,
        messages: Vec<Message>,
        meta: SessionMeta,
        lock: SessionLock,
    ) -> Result<(), String> {
        let Some(archive) = self.archive() else {
            return Ok(());
        };
        let mut meta = meta;
        meta.updated_at = now_unix();
        archive.update_meta(&meta)?;
        self.replace_messages(messages);
        *self.current.lock().unwrap() = Some(meta);
        *self.lock.lock().unwrap() = Some(lock);
        Ok(())
    }

    pub fn finish_session(&self, mode: AgentMode) {
        let _ = self.touch(mode);
        *self.lock.lock().unwrap() = None;
        *self.current.lock().unwrap() = None;
    }

    pub fn current_id(&self) -> Option<String> {
        self.current
            .lock()
            .unwrap()
            .as_ref()
            .map(|meta| meta.id.clone())
    }

    pub fn current_meta(&self) -> Option<SessionMeta> {
        self.current.lock().unwrap().clone()
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
    use crate::sessions::InMemorySessionArchive;
    use tupi_domain::Message;

    #[test]
    fn record_user_creates_a_session_and_increments_the_count() {
        let archive = Arc::new(InMemorySessionArchive::new("repo"));
        let store = SessionStore::new(1).with_archive(archive.clone());

        store
            .record_user(
                "primeira linha\ndetalhe".into(),
                AgentMode::Sdd,
                "deepseek",
                "m",
            )
            .unwrap();

        let sessions = archive.list().unwrap();
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].title, "primeira linha");
        assert_eq!(sessions[0].message_count, 1);

        let id = store.current_id().unwrap();
        let stored = archive.load(&id).unwrap().unwrap();
        assert_eq!(
            stored.messages,
            vec![Message::user("primeira linha\ndetalhe")]
        );
        assert_eq!(stored.meta.mode, AgentMode::Sdd);
    }

    #[test]
    fn finish_then_next_user_creates_a_second_session() {
        let archive = Arc::new(InMemorySessionArchive::new("repo"));
        let store = SessionStore::new(1).with_archive(archive.clone());

        store
            .record_user("um".into(), AgentMode::Execution, "p", "m")
            .unwrap();
        let first = store.current_id().unwrap();
        store.finish_session(AgentMode::Execution);
        store
            .record_user("dois".into(), AgentMode::Execution, "p", "m")
            .unwrap();
        let second = store.current_id().unwrap();

        assert_ne!(first, second);
        assert_eq!(archive.list().unwrap().len(), 2);
        assert_eq!(archive.load(&first).unwrap().unwrap().messages.len(), 1);
    }

    #[test]
    fn resume_does_not_reappend_loaded_messages() {
        let archive = Arc::new(InMemorySessionArchive::new("repo"));
        let store = SessionStore::new(1).with_archive(archive.clone());

        store
            .record_user("antiga".into(), AgentMode::Sdd, "p", "m")
            .unwrap();
        let id = store.current_id().unwrap();
        let stored = archive.load(&id).unwrap().unwrap();
        store.finish_session(AgentMode::Sdd);

        let lock = archive.lock(&id).unwrap();
        store
            .resume(&id, stored.messages, stored.meta, lock)
            .unwrap();
        store
            .record_user("nova".into(), AgentMode::Sdd, "p", "m")
            .unwrap();

        let reloaded = archive.load(&id).unwrap().unwrap();
        assert_eq!(
            reloaded
                .messages
                .iter()
                .map(|message| message.content.as_str())
                .collect::<Vec<_>>(),
            vec!["antiga", "nova"]
        );
        assert_eq!(reloaded.meta.message_count, 2);
    }

    #[test]
    fn assistant_once_is_not_recorded_twice() {
        let archive = Arc::new(InMemorySessionArchive::new("repo"));
        let store = SessionStore::new(1).with_archive(archive.clone());
        store
            .record_user("oi".into(), AgentMode::Execution, "p", "m")
            .unwrap();

        store.record_assistant_once("parcial").unwrap();
        store.record_assistant_once("parcial").unwrap();

        let id = store.current_id().unwrap();
        let stored = archive.load(&id).unwrap().unwrap();
        let assistants = stored
            .messages
            .iter()
            .filter(|message| message.role == Role::Assistant)
            .count();
        assert_eq!(assistants, 1);
    }

    #[test]
    fn checkpoint_resets_the_agent_context() {
        let archive = Arc::new(InMemorySessionArchive::new("repo"));
        let store = SessionStore::new(1).with_archive(archive.clone());
        store
            .record_user("oi".into(), AgentMode::Execution, "p", "m")
            .unwrap();
        store
            .record_checkpoint(vec![Message::user("handoff")])
            .unwrap();
        store
            .record_user("segue".into(), AgentMode::Execution, "p", "m")
            .unwrap();

        let id = store.current_id().unwrap();
        let stored = archive.load(&id).unwrap().unwrap();
        assert_eq!(
            stored
                .messages
                .iter()
                .map(|message| message.content.as_str())
                .collect::<Vec<_>>(),
            vec!["handoff", "segue"]
        );
    }

    #[test]
    fn tool_and_synthetic_increment_the_count() {
        let archive = Arc::new(InMemorySessionArchive::new("repo"));
        let store = SessionStore::new(1).with_archive(archive.clone());
        store
            .record_user("oi".into(), AgentMode::Execution, "p", "m")
            .unwrap();
        store
            .record_tool(
                Message::tool("c1", "ok"),
                &ToolCall {
                    id: "c1".into(),
                    name: "read".into(),
                    arguments: "{}".into(),
                },
                "src/lib.rs",
                true,
                Duration::from_millis(4),
            )
            .unwrap();
        store.record_synthetic(Message::user("brief")).unwrap();

        let id = store.current_id().unwrap();
        assert_eq!(archive.load(&id).unwrap().unwrap().meta.message_count, 3);
    }

    #[test]
    fn without_archive_keeps_the_old_behavior() {
        let store = SessionStore::new(1);
        store
            .record_user("oi".into(), AgentMode::Execution, "p", "m")
            .unwrap();

        assert_eq!(store.messages(), vec![Message::user("oi")]);
        assert!(store.current_id().is_none());
    }
}
