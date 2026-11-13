//! Memória durável por repositório.
//!
//! Dois arquivos sob `.agents/tupi/`:
//!
//! - `facts.jsonl`: fatos apendados, endereçados por um id estável. Um
//!   tombstone (`deleted_at`) esquece o fato sem permitir que o extrator o
//!   reaprenda.
//! - `pending.jsonl`: snapshot atômico da fila de obrigações (lidas e
//!   gravadas pelo harness, não pelo modelo).
//!
//! Tudo aqui é puro disco: quem chama o modelo é `tupi-app`.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::io::Write;
use std::path::{Path, PathBuf};

pub const FACTS_FILE: &str = "facts.jsonl";
pub const PENDING_FILE: &str = "pending.jsonl";
pub const READ_ID: &str = "mem-read";
pub const FEEDBACK_ID: &str = "mem-feedback";
pub const READ_KIND: &str = "memory:read";
pub const FEEDBACK_KIND: &str = "memory:feedback";

pub fn dir(root: &Path) -> PathBuf {
    root.join(".agents").join("tupi")
}

pub fn facts_path(root: &Path) -> PathBuf {
    dir(root).join(FACTS_FILE)
}

pub fn pending_path(root: &Path) -> PathBuf {
    dir(root).join(PENDING_FILE)
}

pub fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or(0)
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Fact {
    #[serde(default)]
    pub id: String,
    pub text: String,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub at: u64,
    #[serde(default)]
    pub source: String,
    #[serde(default)]
    pub deleted_at: Option<u64>,
}

impl Fact {
    pub fn live(&self) -> bool {
        self.deleted_at.is_none()
    }
}

/// Normaliza para que o mesmo texto gere o mesmo id entre sessões.
pub fn normalize(text: &str) -> String {
    text.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

/// FNV-1a de 64 bits: estável, barato e suficiente para um fato por linha.
pub fn fact_id(text: &str) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in normalize(text).as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{hash:016x}")
}

fn parse_fact(line: &str) -> Option<Fact> {
    let mut fact = serde_json::from_str::<Fact>(line).ok()?;
    if fact.text.trim().is_empty() {
        return None;
    }
    if fact.id.is_empty() {
        fact.id = fact_id(&fact.text);
    }
    Some(fact)
}

/// Todas as linhas válidas, na ordem do arquivo (inclui tombstones).
pub fn records(root: &Path) -> Vec<Fact> {
    let Ok(body) = std::fs::read_to_string(facts_path(root)) else {
        return Vec::new();
    };
    body.lines().filter_map(parse_fact).collect()
}

/// Fatos vivos, sem duplicatas: a última linha com o mesmo id vence.
pub fn facts(root: &Path) -> Vec<Fact> {
    let mut folded: Vec<Fact> = Vec::new();
    let mut index = std::collections::HashMap::<String, usize>::new();
    for fact in records(root) {
        match index.get(&fact.id) {
            Some(&position) => folded[position] = fact,
            None => {
                index.insert(fact.id.clone(), folded.len());
                folded.push(fact);
            }
        }
    }
    folded.retain(Fact::live);
    folded
}

pub fn find(root: &Path, id: &str) -> Option<Fact> {
    records(root).into_iter().rev().find(|fact| fact.id == id)
}

fn append(root: &Path, fact: &Fact) -> std::io::Result<()> {
    let path = facts_path(root);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    writeln!(file, "{}", serde_json::to_string(fact).unwrap_or_default())
}

/// Grava um fato novo. Texto já conhecido (vivo ou esquecido) não é regravado.
pub fn record(
    root: &Path,
    text: &str,
    tags: Vec<String>,
    source: &str,
) -> std::io::Result<Option<Fact>> {
    let text = text.trim();
    if text.is_empty() {
        return Ok(None);
    }
    let id = fact_id(text);
    if find(root, &id).is_some() {
        return Ok(None);
    }
    let fact = Fact {
        id,
        text: text.to_string(),
        tags,
        at: now(),
        source: source.to_string(),
        deleted_at: None,
    };
    append(root, &fact)?;
    Ok(Some(fact))
}

/// Esquece por tombstone: o id continua bloqueado para o extrator.
pub fn forget(root: &Path, id: &str) -> std::io::Result<bool> {
    let Some(mut fact) = find(root, id) else {
        return Ok(false);
    };
    if fact.deleted_at.is_some() {
        return Ok(false);
    }
    fact.deleted_at = Some(now());
    append(root, &fact)?;
    Ok(true)
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Pending {
    pub id: String,
    pub kind: String,
    /// `open` ou `closed`.
    pub status: String,
    pub created_at: u64,
    pub updated_at: u64,
    #[serde(default)]
    pub session: String,
    #[serde(default)]
    pub payload: Value,
    #[serde(default)]
    pub closed_by: Option<String>,
    #[serde(default)]
    pub close_reason: Option<String>,
}

impl Pending {
    pub fn open(&self) -> bool {
        self.status == "open"
    }
}

pub fn pendings(root: &Path) -> Vec<Pending> {
    let Ok(body) = std::fs::read_to_string(pending_path(root)) else {
        return Vec::new();
    };
    let mut folded: Vec<Pending> = Vec::new();
    let mut index = std::collections::HashMap::<String, usize>::new();
    for pending in body
        .lines()
        .filter_map(|line| serde_json::from_str::<Pending>(line).ok())
    {
        match index.get(&pending.id) {
            Some(&position) => folded[position] = pending,
            None => {
                index.insert(pending.id.clone(), folded.len());
                folded.push(pending);
            }
        }
    }
    folded
}

pub fn save_pendings(root: &Path, pendings: &[Pending]) -> std::io::Result<()> {
    let path = pending_path(root);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut body = String::new();
    for pending in pendings {
        body.push_str(&serde_json::to_string(pending).unwrap_or_default());
        body.push('\n');
    }
    write_atomic(&path, body.as_bytes())
}

fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(tmp, path)
}

pub fn is_open(pendings: &[Pending], id: &str) -> bool {
    pendings
        .iter()
        .any(|pending| pending.id == id && pending.open())
}

/// Cria a pendência; idempotente enquanto ela estiver aberta.
pub fn open_pending(pendings: &mut Vec<Pending>, id: &str, kind: &str, session: &str) -> bool {
    if let Some(existing) = pendings.iter_mut().find(|pending| pending.id == id) {
        if existing.open() {
            return false;
        }
        let at = now();
        existing.kind = kind.to_string();
        existing.status = "open".to_string();
        existing.created_at = at;
        existing.updated_at = at;
        existing.session = session.to_string();
        existing.closed_by = None;
        existing.close_reason = None;
        return true;
    }
    let at = now();
    pendings.push(Pending {
        id: id.to_string(),
        kind: kind.to_string(),
        status: "open".to_string(),
        created_at: at,
        updated_at: at,
        session: session.to_string(),
        payload: Value::Null,
        closed_by: None,
        close_reason: None,
    });
    true
}

/// Fecha a pendência; devolve `false` se ela não estava aberta.
pub fn close_pending(pendings: &mut [Pending], id: &str, by: &str, reason: &str) -> bool {
    let Some(pending) = pendings
        .iter_mut()
        .find(|pending| pending.id == id && pending.open())
    else {
        return false;
    };
    pending.status = "closed".to_string();
    pending.updated_at = now();
    pending.closed_by = Some(by.to_string());
    pending.close_reason = Some(reason.to_string());
    true
}

const STOPWORDS: &[&str] = &[
    "a", "as", "ao", "aos", "aquela", "aquele", "as", "com", "como", "da", "das", "de", "do",
    "dos", "e", "em", "entre", "era", "essa", "esse", "esta", "este", "eu", "foi", "ha", "isso",
    "isto", "ja", "mais", "mas", "me", "mesmo", "meu", "na", "nao", "nas", "no", "nos", "nossa",
    "o", "os", "ou", "para", "pela", "pelo", "por", "que", "quem", "se", "sem", "ser", "seu", "so",
    "sua", "tem", "um", "uma", "voce", "and", "are", "as", "at", "be", "by", "for", "from", "how",
    "in", "is", "it", "not", "of", "on", "or", "that", "the", "this", "to", "was", "what", "when",
    "where", "which", "why", "with",
];

/// Termos contentuais da consulta, sem stopwords e sem repetição.
pub fn terms(query: &str) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    query
        .to_lowercase()
        .split(|character: char| !character.is_alphanumeric())
        .filter(|term| term.len() >= 3 && !STOPWORDS.contains(term))
        .filter(|term| seen.insert(term.to_string()))
        .map(str::to_string)
        .collect()
}

#[derive(Clone, Debug)]
pub struct Scored {
    pub fact: Fact,
    pub score: usize,
}

/// Ranking lexical barato: o modelo só vê o que já passou por aqui.
pub fn score(root: &Path, query: &str, limit: usize) -> Vec<Scored> {
    let terms = terms(query);
    if terms.is_empty() || limit == 0 {
        return Vec::new();
    }
    let mut scored: Vec<Scored> = facts(root)
        .into_iter()
        .filter_map(|fact| {
            let text = fact.text.to_lowercase();
            let tags = fact
                .tags
                .iter()
                .map(|tag| tag.to_lowercase())
                .collect::<Vec<_>>();
            let mut score = 0usize;
            for term in &terms {
                if tags.iter().any(|tag| tag == term) {
                    score += 3;
                }
                score += text.matches(term.as_str()).count().min(3);
            }
            (score > 0).then_some(Scored { fact, score })
        })
        .collect();
    scored.sort_by(|left, right| {
        right
            .score
            .cmp(&left.score)
            .then(right.fact.at.cmp(&left.fact.at))
    });
    scored.truncate(limit);
    scored
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_recorded_fact_is_listed_once_and_not_relearned() {
        let root = tempfile::tempdir().unwrap();
        let fact = record(
            root.path(),
            "  o teto é 3.3 chars/token ",
            vec!["token".into()],
            "test",
        )
        .unwrap()
        .expect("primeiro registro");
        assert_eq!(fact.id, fact_id("o teto é 3.3 chars/token"));
        assert!(
            record(root.path(), "o teto é 3.3 chars/token", vec![], "test")
                .unwrap()
                .is_none()
        );
        assert_eq!(facts(root.path()).len(), 1);
    }

    #[test]
    fn forgetting_is_a_tombstone_that_blocks_relearning() {
        let root = tempfile::tempdir().unwrap();
        let fact = record(root.path(), "o watcher usa notify", vec![], "test")
            .unwrap()
            .expect("registro");
        assert!(forget(root.path(), &fact.id).unwrap());
        assert!(facts(root.path()).is_empty());
        assert!(find(root.path(), &fact.id).unwrap().deleted_at.is_some());
        assert!(
            record(root.path(), "o watcher usa notify", vec![], "test")
                .unwrap()
                .is_none(),
            "tombstone impede reaprender"
        );
    }

    #[test]
    fn pending_open_is_idempotent_and_close_is_one_shot() {
        let root = tempfile::tempdir().unwrap();
        let mut queue = Vec::new();
        assert!(open_pending(&mut queue, READ_ID, READ_KIND, "s1"));
        assert!(!open_pending(&mut queue, READ_ID, READ_KIND, "s1"));
        save_pendings(root.path(), &queue).unwrap();

        let mut loaded = pendings(root.path());
        assert!(is_open(&loaded, READ_ID));
        assert!(close_pending(&mut loaded, READ_ID, "recall", "consulted"));
        assert!(!close_pending(&mut loaded, READ_ID, "recall", "consulted"));
        save_pendings(root.path(), &loaded).unwrap();

        let loaded = pendings(root.path());
        assert!(!is_open(&loaded, READ_ID));
        assert_eq!(loaded[0].close_reason.as_deref(), Some("consulted"));
    }

    #[test]
    fn score_ignores_stopwords_and_prefers_tags() {
        let root = tempfile::tempdir().unwrap();
        record(root.path(), "o login usa PKCE", vec!["auth".into()], "test").unwrap();
        record(
            root.path(),
            "o grafo é reconstruído por watcher",
            vec!["graph".into()],
            "test",
        )
        .unwrap();

        let auth = score(root.path(), "como funciona o auth do login?", 5);
        assert_eq!(auth.len(), 1);
        assert_eq!(auth[0].fact.tags, vec!["auth".to_string()]);

        assert!(score(root.path(), "bom dia", 5).is_empty());
    }
}
