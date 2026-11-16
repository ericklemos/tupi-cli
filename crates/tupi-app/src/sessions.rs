use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use tupi_domain::{AgentMode, Message};
use tupi_kernel::TranscriptEntry;

/// Versão do formato de `meta.json` e de cada linha do log.
pub const SESSION_SCHEMA: u64 = 1;
/// Título usado enquanto nenhuma mensagem do usuário chegou.
pub const UNTITLED: &str = "sessão sem título";

const LOCK_STALE_SECS: u64 = 600;

#[derive(Debug, Clone)]
pub struct SessionMeta {
    pub id: String,
    pub repo_key: String,
    pub title: String,
    pub created_at: u64,
    pub updated_at: u64,
    pub message_count: usize,
    pub mode: AgentMode,
    pub provider: String,
    pub model: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SessionSummary {
    pub id: String,
    pub title: String,
    pub updated_at: u64,
    pub message_count: usize,
}

#[derive(Debug, Clone)]
pub struct StoredSession {
    pub meta: SessionMeta,
    pub messages: Vec<Message>,
    pub transcript: Vec<TranscriptEntry>,
}

#[derive(Debug, Clone)]
pub enum SessionRecord {
    User {
        at: u64,
        message: Message,
    },
    Assistant {
        at: u64,
        message: Message,
    },
    Tool {
        at: u64,
        message: Message,
        call_id: String,
        name: String,
        summary: String,
        ok: bool,
        duration_us: u64,
    },
    Synthetic {
        at: u64,
        message: Message,
    },
    System {
        at: u64,
        message: Message,
    },
    Checkpoint {
        at: u64,
        messages: Vec<Message>,
    },
}

impl SessionRecord {
    pub fn user(at: u64, message: Message) -> Self {
        Self::User { at, message }
    }

    pub fn assistant(at: u64, message: Message) -> Self {
        Self::Assistant { at, message }
    }

    pub fn synthetic(at: u64, message: Message) -> Self {
        Self::Synthetic { at, message }
    }

    pub fn system(at: u64, message: Message) -> Self {
        Self::System { at, message }
    }

    pub fn checkpoint(at: u64, messages: Vec<Message>) -> Self {
        Self::Checkpoint { at, messages }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn tool(
        at: u64,
        message: Message,
        call_id: &str,
        name: &str,
        summary: &str,
        ok: bool,
        duration_us: u64,
    ) -> Self {
        Self::Tool {
            at,
            message,
            call_id: call_id.to_string(),
            name: name.to_string(),
            summary: summary.to_string(),
            ok,
            duration_us,
        }
    }
}

pub trait SessionArchive: Send + Sync {
    fn repo_key(&self) -> &str;
    fn create(&self, meta: &SessionMeta) -> Result<SessionLock, String>;
    fn append(&self, id: &str, record: &SessionRecord) -> Result<(), String>;
    fn load(&self, id: &str) -> Result<Option<StoredSession>, String>;
    fn list(&self) -> Result<Vec<SessionSummary>, String>;
    fn update_meta(&self, meta: &SessionMeta) -> Result<(), String>;
    fn delete(&self, id: &str) -> Result<bool, String>;
    fn lock(&self, id: &str) -> Result<SessionLock, String>;
}

pub struct SessionLock {
    path: Option<PathBuf>,
}

impl SessionLock {
    fn none() -> Self {
        Self { path: None }
    }
}

impl Drop for SessionLock {
    fn drop(&mut self) {
        if let Some(path) = &self.path {
            let _ = fs::remove_file(path);
        }
    }
}

pub struct FsSessionArchive {
    sessions: PathBuf,
    repo_key: String,
}

impl FsSessionArchive {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        let root = root.into();
        let repo_key = tupi_tools::repo_key(&root);
        Self {
            sessions: root.join(".tupi").join("sessions"),
            repo_key,
        }
    }

    fn dir(&self, id: &str) -> PathBuf {
        self.sessions.join(id)
    }

    fn meta_path(&self, id: &str) -> PathBuf {
        self.dir(id).join("meta.json")
    }

    fn log_path(&self, id: &str) -> PathBuf {
        self.dir(id).join("log.jsonl")
    }

    fn lock_path(&self, id: &str) -> PathBuf {
        self.dir(id).join(".lock")
    }

    fn try_lock(&self, id: &str) -> Result<SessionLock, String> {
        let path = self.lock_path(id);
        if let Ok(mut file) = open_private(&path, true) {
            let _ = writeln!(file, "{}", std::process::id());
            return Ok(SessionLock { path: Some(path) });
        }
        if lock_is_stale(&path) {
            let _ = fs::remove_file(&path);
            if let Ok(mut file) = open_private(&path, true) {
                let _ = writeln!(file, "{}", std::process::id());
                return Ok(SessionLock { path: Some(path) });
            }
        }
        Err(format!("sessão `{id}` está aberta em outro processo"))
    }
}

impl SessionArchive for FsSessionArchive {
    fn repo_key(&self) -> &str {
        &self.repo_key
    }

    fn create(&self, meta: &SessionMeta) -> Result<SessionLock, String> {
        let dir = self.dir(&meta.id);
        fs::create_dir_all(&dir)
            .map_err(|error| format!("falha ao criar {}: {error}", dir.display()))?;
        if self.meta_path(&meta.id).exists() {
            return Err(format!("sessão `{}` já existe", meta.id));
        }
        let lock = self.try_lock(&meta.id)?;
        self.update_meta(meta)?;
        Ok(lock)
    }

    fn append(&self, id: &str, record: &SessionRecord) -> Result<(), String> {
        let line = LogLine::from_record(record);
        let mut text = serde_json::to_string(&line)
            .map_err(|error| format!("falha ao serializar sessão: {error}"))?;
        text.push('\n');

        let path = self.log_path(id);
        let mut file = open_private(&path, false)
            .map_err(|error| format!("falha ao abrir {}: {error}", path.display()))?;
        file.write_all(text.as_bytes())
            .map_err(|error| format!("falha ao gravar {}: {error}", path.display()))?;
        Ok(())
    }

    fn load(&self, id: &str) -> Result<Option<StoredSession>, String> {
        let meta_path = self.meta_path(id);
        let meta_text = match fs::read_to_string(&meta_path) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => {
                return Err(format!("falha ao ler {}: {error}", meta_path.display()));
            }
        };
        let meta_file: MetaFile = serde_json::from_str(&meta_text)
            .map_err(|error| format!("JSON inválido em {}: {error}", meta_path.display()))?;
        if meta_file.schema != SESSION_SCHEMA {
            return Err(format!(
                "sessão `{id}` usa schema {} e este Tupi entende {}",
                meta_file.schema, SESSION_SCHEMA
            ));
        }

        let mut records = Vec::new();
        if let Ok(text) = fs::read_to_string(self.log_path(id)) {
            for line in text.lines() {
                if line.trim().is_empty() {
                    continue;
                }
                let Ok(line) = serde_json::from_str::<LogLine>(line) else {
                    continue;
                };
                if line.schema() != SESSION_SCHEMA {
                    continue;
                }
                records.push(line.into_record());
            }
        }

        Ok(Some(build_stored(meta_file.into(), &records)))
    }

    fn list(&self) -> Result<Vec<SessionSummary>, String> {
        let mut out = Vec::new();
        let entries = match fs::read_dir(&self.sessions) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(out),
            Err(error) => return Err(format!("falha ao ler {}: {error}", self.sessions.display())),
        };
        for entry in entries.flatten() {
            let path = entry.path().join("meta.json");
            let Ok(text) = fs::read_to_string(&path) else {
                continue;
            };
            let Ok(meta) = serde_json::from_str::<MetaFile>(&text) else {
                continue;
            };
            if meta.schema != SESSION_SCHEMA {
                continue;
            }
            out.push(SessionSummary {
                id: meta.id,
                title: meta.title,
                updated_at: meta.updated_at,
                message_count: meta.message_count,
            });
        }
        out.sort_by(|a, b| {
            b.updated_at
                .cmp(&a.updated_at)
                .then_with(|| b.id.cmp(&a.id))
        });
        Ok(out)
    }

    fn update_meta(&self, meta: &SessionMeta) -> Result<(), String> {
        let path = self.meta_path(&meta.id);
        let parent = path.parent().unwrap_or(&self.sessions);
        fs::create_dir_all(parent)
            .map_err(|error| format!("falha ao criar {}: {error}", parent.display()))?;
        let tmp = path.with_extension("json.tmp");
        let text = serde_json::to_string_pretty(&MetaFile::from(meta))
            .map_err(|error| format!("falha ao serializar meta: {error}"))?;
        fs::write(&tmp, text.as_bytes())
            .map_err(|error| format!("falha ao gravar {}: {error}", tmp.display()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(&tmp, fs::Permissions::from_mode(0o600));
        }
        fs::rename(&tmp, &path)
            .map_err(|error| format!("falha ao substituir {}: {error}", path.display()))?;
        Ok(())
    }

    fn delete(&self, id: &str) -> Result<bool, String> {
        let dir = self.dir(id);
        if !dir.exists() {
            return Ok(false);
        }
        fs::remove_dir_all(&dir)
            .map_err(|error| format!("falha ao apagar {}: {error}", dir.display()))?;
        Ok(true)
    }

    fn lock(&self, id: &str) -> Result<SessionLock, String> {
        self.try_lock(id)
    }
}

#[derive(Default)]
pub struct InMemorySessionArchive {
    repo_key: String,
    inner: Mutex<HashMap<String, (SessionMeta, Vec<SessionRecord>)>>,
}

impl InMemorySessionArchive {
    pub fn new(repo_key: impl Into<String>) -> Self {
        Self {
            repo_key: repo_key.into(),
            inner: Mutex::new(HashMap::new()),
        }
    }
}

impl SessionArchive for InMemorySessionArchive {
    fn repo_key(&self) -> &str {
        &self.repo_key
    }

    fn create(&self, meta: &SessionMeta) -> Result<SessionLock, String> {
        let mut inner = self.inner.lock().unwrap();
        if inner.contains_key(&meta.id) {
            return Err(format!("sessão `{}` já existe", meta.id));
        }
        inner.insert(meta.id.clone(), (meta.clone(), Vec::new()));
        Ok(SessionLock::none())
    }

    fn append(&self, id: &str, record: &SessionRecord) -> Result<(), String> {
        let mut inner = self.inner.lock().unwrap();
        let Some((_, records)) = inner.get_mut(id) else {
            return Err(format!("sessão `{id}` não existe"));
        };
        records.push(record.clone());
        Ok(())
    }

    fn load(&self, id: &str) -> Result<Option<StoredSession>, String> {
        let inner = self.inner.lock().unwrap();
        Ok(inner
            .get(id)
            .map(|(meta, records)| build_stored(meta.clone(), records)))
    }

    fn list(&self) -> Result<Vec<SessionSummary>, String> {
        let inner = self.inner.lock().unwrap();
        let mut out: Vec<SessionSummary> = inner
            .values()
            .map(|(meta, _)| SessionSummary {
                id: meta.id.clone(),
                title: meta.title.clone(),
                updated_at: meta.updated_at,
                message_count: meta.message_count,
            })
            .collect();
        out.sort_by(|a, b| {
            b.updated_at
                .cmp(&a.updated_at)
                .then_with(|| b.id.cmp(&a.id))
        });
        Ok(out)
    }

    fn update_meta(&self, meta: &SessionMeta) -> Result<(), String> {
        let mut inner = self.inner.lock().unwrap();
        let Some((slot, _)) = inner.get_mut(&meta.id) else {
            return Err(format!("sessão `{}` não existe", meta.id));
        };
        *slot = meta.clone();
        Ok(())
    }

    fn delete(&self, id: &str) -> Result<bool, String> {
        Ok(self.inner.lock().unwrap().remove(id).is_some())
    }

    fn lock(&self, id: &str) -> Result<SessionLock, String> {
        if self.inner.lock().unwrap().contains_key(id) {
            Ok(SessionLock::none())
        } else {
            Err(format!("sessão `{id}` não existe"))
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct MetaFile {
    schema: u64,
    id: String,
    repo_key: String,
    title: String,
    created_at: u64,
    updated_at: u64,
    message_count: usize,
    mode: String,
    provider: String,
    model: String,
}

impl From<&SessionMeta> for MetaFile {
    fn from(meta: &SessionMeta) -> Self {
        Self {
            schema: SESSION_SCHEMA,
            id: meta.id.clone(),
            repo_key: meta.repo_key.clone(),
            title: meta.title.clone(),
            created_at: meta.created_at,
            updated_at: meta.updated_at,
            message_count: meta.message_count,
            mode: mode_str(meta.mode).to_string(),
            provider: meta.provider.clone(),
            model: meta.model.clone(),
        }
    }
}

impl From<MetaFile> for SessionMeta {
    fn from(meta: MetaFile) -> Self {
        Self {
            id: meta.id,
            repo_key: meta.repo_key,
            title: meta.title,
            created_at: meta.created_at,
            updated_at: meta.updated_at,
            message_count: meta.message_count,
            mode: parse_mode(&meta.mode),
            provider: meta.provider,
            model: meta.model,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum LogLine {
    User {
        schema: u64,
        at: u64,
        message: Message,
    },
    Assistant {
        schema: u64,
        at: u64,
        message: Message,
    },
    Tool {
        schema: u64,
        at: u64,
        message: Message,
        tool: ToolLine,
    },
    Synthetic {
        schema: u64,
        at: u64,
        message: Message,
    },
    System {
        schema: u64,
        at: u64,
        message: Message,
    },
    Checkpoint {
        schema: u64,
        at: u64,
        messages: Vec<Message>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ToolLine {
    call_id: String,
    name: String,
    summary: String,
    ok: bool,
    duration_us: u64,
}

impl LogLine {
    fn schema(&self) -> u64 {
        match self {
            Self::User { schema, .. }
            | Self::Assistant { schema, .. }
            | Self::Tool { schema, .. }
            | Self::Synthetic { schema, .. }
            | Self::System { schema, .. }
            | Self::Checkpoint { schema, .. } => *schema,
        }
    }

    fn from_record(record: &SessionRecord) -> Self {
        match record {
            SessionRecord::User { at, message } => Self::User {
                schema: SESSION_SCHEMA,
                at: *at,
                message: message.clone(),
            },
            SessionRecord::Assistant { at, message } => Self::Assistant {
                schema: SESSION_SCHEMA,
                at: *at,
                message: message.clone(),
            },
            SessionRecord::Tool {
                at,
                message,
                call_id,
                name,
                summary,
                ok,
                duration_us,
            } => Self::Tool {
                schema: SESSION_SCHEMA,
                at: *at,
                message: message.clone(),
                tool: ToolLine {
                    call_id: call_id.clone(),
                    name: name.clone(),
                    summary: summary.clone(),
                    ok: *ok,
                    duration_us: *duration_us,
                },
            },
            SessionRecord::Synthetic { at, message } => Self::Synthetic {
                schema: SESSION_SCHEMA,
                at: *at,
                message: message.clone(),
            },
            SessionRecord::System { at, message } => Self::System {
                schema: SESSION_SCHEMA,
                at: *at,
                message: message.clone(),
            },
            SessionRecord::Checkpoint { at, messages } => Self::Checkpoint {
                schema: SESSION_SCHEMA,
                at: *at,
                messages: messages.clone(),
            },
        }
    }

    fn into_record(self) -> SessionRecord {
        match self {
            Self::User { at, message, .. } => SessionRecord::User { at, message },
            Self::Assistant { at, message, .. } => SessionRecord::Assistant { at, message },
            Self::Tool {
                at, message, tool, ..
            } => SessionRecord::Tool {
                at,
                message,
                call_id: tool.call_id,
                name: tool.name,
                summary: tool.summary,
                ok: tool.ok,
                duration_us: tool.duration_us,
            },
            Self::Synthetic { at, message, .. } => SessionRecord::Synthetic { at, message },
            Self::System { at, message, .. } => SessionRecord::System { at, message },
            Self::Checkpoint { at, messages, .. } => SessionRecord::Checkpoint { at, messages },
        }
    }
}

fn build_stored(meta: SessionMeta, records: &[SessionRecord]) -> StoredSession {
    let mut messages = Vec::new();
    let mut transcript = Vec::new();

    for record in records {
        match record {
            SessionRecord::User { message, .. } => {
                messages.push(message.clone());
                transcript.push(TranscriptEntry::User {
                    text: message.content.clone(),
                });
            }
            SessionRecord::Assistant { message, .. } => {
                messages.push(message.clone());
                if !message.content.is_empty() {
                    transcript.push(TranscriptEntry::Assistant {
                        text: message.content.clone(),
                    });
                }
            }
            SessionRecord::Tool {
                message,
                call_id,
                name,
                summary,
                ok,
                duration_us,
                ..
            } => {
                messages.push(message.clone());
                transcript.push(TranscriptEntry::Tool {
                    call_id: call_id.clone(),
                    name: name.clone(),
                    summary: summary.clone(),
                    ok: *ok,
                    duration_us: *duration_us,
                });
            }
            SessionRecord::Synthetic { message, .. } => {
                messages.push(message.clone());
                transcript.push(TranscriptEntry::Notice {
                    text: message.content.clone(),
                });
            }
            SessionRecord::System { message, .. } => {
                messages.push(message.clone());
            }
            SessionRecord::Checkpoint {
                messages: reset, ..
            } => {
                messages = reset.clone();
            }
        }
    }

    StoredSession {
        meta,
        messages,
        transcript,
    }
}

fn open_private(path: &Path, exclusive: bool) -> std::io::Result<File> {
    let mut options = OpenOptions::new();
    if exclusive {
        options.write(true).create_new(true);
    } else {
        options.create(true).append(true);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)
}

fn lock_is_stale(path: &Path) -> bool {
    fs::metadata(path)
        .and_then(|meta| meta.modified())
        .ok()
        .and_then(|modified| SystemTime::now().duration_since(modified).ok())
        .map(|elapsed| elapsed > Duration::from_secs(LOCK_STALE_SECS))
        .unwrap_or(false)
}

pub fn session_title(text: &str) -> String {
    let line = text.lines().next().unwrap_or("").trim();
    if line.is_empty() {
        return UNTITLED.to_string();
    }
    line.chars()
        .take(60)
        .collect::<String>()
        .trim_end()
        .to_string()
}

pub fn relative_time(now: u64, at: u64) -> String {
    let delta = now.saturating_sub(at);
    if delta < 60 {
        "agora".to_string()
    } else if delta < 3_600 {
        format!("{} min", delta / 60)
    } else if delta < 86_400 {
        format!("{} h", delta / 3_600)
    } else if delta < 7 * 86_400 {
        format!("{} d", delta / 86_400)
    } else {
        format!("{} sem", delta / (7 * 86_400))
    }
}

pub fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or(0)
}

pub fn new_session_id() -> String {
    let elapsed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    format!("{}-{}", elapsed.as_secs(), elapsed.subsec_nanos())
}

fn mode_str(mode: AgentMode) -> &'static str {
    match mode {
        AgentMode::Execution => "execution",
        AgentMode::Sdd => "sdd",
        AgentMode::Docs => "docs",
    }
}

fn parse_mode(raw: &str) -> AgentMode {
    match raw {
        "sdd" => AgentMode::Sdd,
        "docs" => AgentMode::Docs,
        _ => AgentMode::Execution,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tupi_domain::Message;

    fn meta(archive: &FsSessionArchive, id: &str, title: &str, updated_at: u64) -> SessionMeta {
        SessionMeta {
            id: id.into(),
            repo_key: archive.repo_key().to_string(),
            title: title.into(),
            created_at: updated_at,
            updated_at,
            message_count: 0,
            mode: AgentMode::Execution,
            provider: "deepseek".into(),
            model: "deepseek-chat".into(),
        }
    }

    #[test]
    fn round_trip_meta_and_log() {
        let dir = tempfile::tempdir().unwrap();
        let archive = FsSessionArchive::new(dir.path());
        let lock = archive
            .create(&meta(&archive, "100-1", "Primeira", 100))
            .unwrap();

        archive
            .append("100-1", &SessionRecord::user(100, Message::user("oi")))
            .unwrap();
        archive
            .append(
                "100-1",
                &SessionRecord::assistant(101, Message::assistant("olá")),
            )
            .unwrap();
        archive
            .append(
                "100-1",
                &SessionRecord::tool(
                    102,
                    Message::tool("c1", "ok"),
                    "c1",
                    "read",
                    "src/lib.rs",
                    true,
                    42,
                ),
            )
            .unwrap();

        let stored = archive.load("100-1").unwrap().unwrap();
        assert_eq!(stored.meta.title, "Primeira");
        assert_eq!(stored.messages.len(), 3);
        assert_eq!(
            stored.transcript,
            vec![
                TranscriptEntry::User { text: "oi".into() },
                TranscriptEntry::Assistant {
                    text: "olá".into()
                },
                TranscriptEntry::Tool {
                    call_id: "c1".into(),
                    name: "read".into(),
                    summary: "src/lib.rs".into(),
                    ok: true,
                    duration_us: 42
                }
            ]
        );
        drop(lock);
    }

    #[test]
    fn list_is_newest_first_and_skips_missing_meta() {
        let dir = tempfile::tempdir().unwrap();
        let archive = FsSessionArchive::new(dir.path());
        let _a = archive
            .create(&meta(&archive, "100-1", "Antiga", 100))
            .unwrap();
        let _b = archive
            .create(&meta(&archive, "200-2", "Nova", 200))
            .unwrap();
        fs::create_dir_all(archive.sessions.join("vazio")).unwrap();

        let list = archive.list().unwrap();
        assert_eq!(
            list.iter().map(|item| item.id.as_str()).collect::<Vec<_>>(),
            vec!["200-2", "100-1"]
        );
    }

    #[test]
    fn partial_last_line_is_ignored() {
        let dir = tempfile::tempdir().unwrap();
        let archive = FsSessionArchive::new(dir.path());
        let _lock = archive.create(&meta(&archive, "100-1", "A", 100)).unwrap();
        archive
            .append("100-1", &SessionRecord::user(100, Message::user("oi")))
            .unwrap();

        let mut file = OpenOptions::new()
            .append(true)
            .open(archive.log_path("100-1"))
            .unwrap();
        file.write_all(br#"{"schema":1,"kind":"user""#).unwrap();
        drop(file);

        assert_eq!(archive.load("100-1").unwrap().unwrap().messages.len(), 1);
    }

    #[test]
    fn future_schema_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let archive = FsSessionArchive::new(dir.path());
        let _lock = archive.create(&meta(&archive, "100-1", "A", 100)).unwrap();
        let path = archive.meta_path("100-1");
        let text = fs::read_to_string(&path).unwrap();
        fs::write(&path, text.replace(r#""schema": 1"#, r#""schema": 2"#)).unwrap();

        assert!(archive.load("100-1").is_err());
    }

    #[test]
    fn delete_removes_the_session() {
        let dir = tempfile::tempdir().unwrap();
        let archive = FsSessionArchive::new(dir.path());
        let lock = archive.create(&meta(&archive, "100-1", "A", 100)).unwrap();
        drop(lock);

        assert!(archive.delete("100-1").unwrap());
        assert!(archive.load("100-1").unwrap().is_none());
        assert!(!archive.delete("100-1").unwrap());
    }

    #[test]
    fn checkpoint_replaces_the_agent_context_but_keeps_the_transcript() {
        let dir = tempfile::tempdir().unwrap();
        let archive = FsSessionArchive::new(dir.path());
        let _lock = archive.create(&meta(&archive, "100-1", "A", 100)).unwrap();
        archive
            .append("100-1", &SessionRecord::user(100, Message::user("oi")))
            .unwrap();
        archive
            .append(
                "100-1",
                &SessionRecord::assistant(101, Message::assistant("olá")),
            )
            .unwrap();
        archive
            .append(
                "100-1",
                &SessionRecord::checkpoint(102, vec![Message::user("handoff")]),
            )
            .unwrap();
        archive
            .append("100-1", &SessionRecord::user(103, Message::user("segue")))
            .unwrap();

        let stored = archive.load("100-1").unwrap().unwrap();
        assert_eq!(
            stored
                .messages
                .iter()
                .map(|message| message.content.as_str())
                .collect::<Vec<_>>(),
            vec!["handoff", "segue"]
        );
        assert_eq!(
            stored.transcript,
            vec![
                TranscriptEntry::User { text: "oi".into() },
                TranscriptEntry::Assistant {
                    text: "olá".into()
                },
                TranscriptEntry::User {
                    text: "segue".into()
                }
            ]
        );
    }

    #[test]
    fn synthetic_messages_are_not_user_bubbles() {
        let dir = tempfile::tempdir().unwrap();
        let archive = FsSessionArchive::new(dir.path());
        let _lock = archive.create(&meta(&archive, "100-1", "A", 100)).unwrap();
        archive
            .append(
                "100-1",
                &SessionRecord::synthetic(100, Message::user("brief do SDD")),
            )
            .unwrap();

        let stored = archive.load("100-1").unwrap().unwrap();
        assert_eq!(stored.messages.len(), 1);
        assert_eq!(
            stored.transcript,
            vec![TranscriptEntry::Notice {
                text: "brief do SDD".into()
            }]
        );
    }

    #[test]
    fn title_is_the_first_user_line_capped() {
        assert_eq!(session_title("  linha um\nresto"), "linha um");
        assert_eq!(session_title("   "), UNTITLED);
        let long = "x".repeat(80);
        assert_eq!(session_title(&long).chars().count(), 60);
    }

    #[test]
    fn relative_time_reads_naturally() {
        assert_eq!(relative_time(1000, 1000), "agora");
        assert_eq!(relative_time(1120, 1000), "2 min");
        assert_eq!(relative_time(7300, 1000), "1 h");
        assert_eq!(relative_time(1000 + 3 * 86_400, 1000), "3 d");
    }

    #[test]
    fn old_lock_is_reclaimed() {
        let dir = tempfile::tempdir().unwrap();
        let archive = FsSessionArchive::new(dir.path());
        let lock = archive.create(&meta(&archive, "100-1", "A", 100)).unwrap();
        drop(lock);

        let path = archive.lock_path("100-1");
        fs::write(&path, b"stale").unwrap();
        let old = SystemTime::now() - Duration::from_secs(LOCK_STALE_SECS + 60);
        let file = OpenOptions::new().write(true).open(&path).unwrap();
        file.set_times(fs::FileTimes::new().set_modified(old))
            .unwrap();
        drop(file);

        assert!(archive.lock("100-1").is_ok());
    }

    #[test]
    fn fresh_lock_blocks_a_second_open() {
        let dir = tempfile::tempdir().unwrap();
        let archive = FsSessionArchive::new(dir.path());
        let _lock = archive.create(&meta(&archive, "100-1", "A", 100)).unwrap();
        assert!(archive.lock("100-1").is_err());
    }

    #[test]
    fn huge_tool_result_is_one_line() {
        let dir = tempfile::tempdir().unwrap();
        let archive = FsSessionArchive::new(dir.path());
        let _lock = archive.create(&meta(&archive, "100-1", "A", 100)).unwrap();
        let big = "x".repeat(1_000_000);
        archive
            .append(
                "100-1",
                &SessionRecord::tool(
                    100,
                    Message::tool("c1", big.clone()),
                    "c1",
                    "read",
                    "grande",
                    true,
                    1,
                ),
            )
            .unwrap();

        let stored = archive.load("100-1").unwrap().unwrap();
        assert_eq!(stored.messages[0].content.len(), big.len());
        let log = fs::read_to_string(archive.log_path("100-1")).unwrap();
        assert_eq!(log.lines().count(), 1);
    }

    #[test]
    fn control_chars_stay_escaped() {
        let dir = tempfile::tempdir().unwrap();
        let archive = FsSessionArchive::new(dir.path());
        let _lock = archive.create(&meta(&archive, "100-1", "A", 100)).unwrap();
        let text = "a\nb\"c\u{1}";
        archive
            .append("100-1", &SessionRecord::user(100, Message::user(text)))
            .unwrap();

        let stored = archive.load("100-1").unwrap().unwrap();
        assert_eq!(stored.messages[0].content, text);
        let log = fs::read_to_string(archive.log_path("100-1")).unwrap();
        assert_eq!(log.lines().count(), 1);
    }

    #[cfg(unix)]
    #[test]
    fn files_are_private() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let archive = FsSessionArchive::new(dir.path());
        let _lock = archive.create(&meta(&archive, "100-1", "A", 100)).unwrap();
        archive
            .append("100-1", &SessionRecord::user(100, Message::user("oi")))
            .unwrap();

        for path in [archive.meta_path("100-1"), archive.log_path("100-1")] {
            let mode = fs::metadata(path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
    }
}
