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
/// Arquivo JSONL onde o modelo registra o proprio atrito (log local de debug).
pub const FEEDBACK_FILE: &str = "feedback.jsonl";

/// Subdiretorio de debug sob `memory::dir(root)`, o mesmo da telemetria do shell.
pub const DEBUG_DIR: &str = "debug";

pub fn debug_dir(root: &Path) -> PathBuf {
    dir(root).join(DEBUG_DIR)
}

pub fn feedback_path(root: &Path) -> PathBuf {
    debug_dir(root).join(FEEDBACK_FILE)
}

/// Uma linha de `feedback.jsonl`: atrito que o modelo encontrou em campo.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Feedback {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub ts_ms: u64,
    #[serde(default)]
    pub session: String,
    #[serde(default)]
    pub mode: String,
    #[serde(default)]
    pub model: String,
    #[serde(default)]
    pub kind: String,
    #[serde(default)]
    pub tool: String,
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workaround: Option<String>,
}

pub fn fact_id(text: &str) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in normalize(text).as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{hash:016x}")
}

/// Teto do log de feedback. É log de debug local: o que interessa é o fim do
/// arquivo, então o corte joga fora o começo quando passa daqui.
pub const FEEDBACK_MAX_BYTES: u64 = 1024 * 1024;

/// Grava uma linha em `.agents/tupi/debug/feedback.jsonl`, criando o diretório
/// quando falta e cortando o começo do arquivo quando ele passou do teto.
///
/// Append síncrono, no mesmo molde do `trim` da telemetria: sem thread de
/// escrita, sem compactação por relógio e sem sair da máquina.
pub fn append_feedback(root: &Path, feedback: &Feedback) -> std::io::Result<()> {
    let path = feedback_path(root);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    // o corte vem antes do append: o registro novo nunca cai no recorte e o
    // arquivo volta ao teto sem reescrever a cada linha gravada
    trim_feedback(&path, FEEDBACK_MAX_BYTES)?;

    let mut record = feedback.clone();
    // o id é derivado do texto normalizado: o mesmo atrito escrito com outra
    // caixa ou outro espaçamento é o mesmo registro
    record.id = fact_id(&record.text);

    if record.ts_ms == 0 {
        record.ts_ms = now();
    }
    let mut line = serde_json::to_string(&record).unwrap_or_default();
    line.push('\n');
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)?;
    file.write_all(line.as_bytes())
}

/// Corta o começo de `path` até sobrar a metade de `max_bytes`, preservando as
/// linhas mais recentes inteiras.
fn trim_feedback(path: &Path, max_bytes: u64) -> std::io::Result<u64> {
    let len = match std::fs::metadata(path) {
        Ok(metadata) => metadata.len(),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(error) => return Err(error),
    };
    if len <= max_bytes {
        return Ok(len);
    }

    let bytes = std::fs::read(path)?;
    let keep = (max_bytes / 2) as usize;
    let tail = &bytes[bytes.len() - keep..];
    // o recorte pode cair no meio de uma linha: descarta até o primeiro `\n`
    let tail = match tail.iter().position(|byte| *byte == b'\n') {
        Some(at) => &tail[at + 1..],
        None => tail,
    };
    write_atomic(path, tail)?;
    Ok(tail.len() as u64)
}

fn parse_feedback(line: &str) -> Option<Feedback> {
    let mut feedback = serde_json::from_str::<Feedback>(line).ok()?;
    if feedback.text.trim().is_empty() {
        return None;
    }
    if feedback.id.is_empty() {
        feedback.id = fact_id(&feedback.text);
    }
    Some(feedback)
}

pub fn feedback_records(root: &Path) -> Vec<Feedback> {
    let Ok(body) = std::fs::read_to_string(feedback_path(root)) else {
        return Vec::new();
    };
    body.lines().filter_map(parse_feedback).collect()
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
    // Teto do log antes de compactar. O caminho normal é só append: cada linha
    // é um retrato completo de um `Pending` e o leitor fica com a última
    // ocorrência do id, então duas abas fechando dívidas ao mesmo tempo não se
    // apagam e não existe read-modify-write do arquivo inteiro.
    const COMPACT_LINES: usize = 512;

    let path = pending_path(root);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut body = String::new();
    for pending in pendings {
        body.push_str(&serde_json::to_string(pending).unwrap_or_default());
        body.push('\n');
    }
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)?;
    file.write_all(body.as_bytes())?;
    drop(file);

    // Compactação: reconcilia relendo o próprio log, para não descartar linhas
    // que outra sessão anexou depois da nossa escrita.
    let log = std::fs::read_to_string(&path)?;
    if log.lines().count() <= COMPACT_LINES {
        return Ok(());
    }
    let mut snapshot = String::new();
    for pending in super::memory::pendings(root) {
        snapshot.push_str(&serde_json::to_string(&pending).unwrap_or_default());
        snapshot.push('\n');
    }
    write_atomic(&path, snapshot.as_bytes())
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

    #[test]
    fn feedback_path_is_the_local_debug_sink() {
        let root = tempfile::tempdir().unwrap();
        let path = feedback_path(root.path());
        assert!(path.ends_with(".agents/tupi/debug/feedback.jsonl"));
        assert_eq!(path, debug_dir(root.path()).join(FEEDBACK_FILE));
    }

    #[test]
    fn feedback_round_trips_with_absent_optional_fields() {
        let bare: Feedback = serde_json::from_str(r#"{"text":"sem grep"}"#).unwrap();
        assert!(bare.id.is_empty());
        assert_eq!(bare.ts_ms, 0);
        assert!(bare.workaround.is_none());
        assert_eq!(bare.text, "sem grep");

        let full = Feedback {
            id: fact_id("sem grep"),
            text: "sem grep".into(),
            workaround: Some("use search".into()),
            ..Feedback::default()
        };
        let line = serde_json::to_string(&full).unwrap();
        assert_eq!(serde_json::from_str::<Feedback>(&line).unwrap(), full);
        assert!(!serde_json::to_string(&bare).unwrap().contains("workaround"));
    }

    #[test]
    fn appending_feedback_creates_the_debug_dir_and_stamps_the_record() {
        let root = tempfile::tempdir().unwrap();
        let path = feedback_path(root.path());
        assert!(!path.exists(), "o diretório ainda não existe");

        append_feedback(
            root.path(),
            &Feedback {
                text: "sem grep".into(),
                ..Feedback::default()
            },
        )
        .unwrap();

        assert_eq!(
            debug_dir(root.path()),
            root.path().join(".agents").join("tupi").join("debug")
        );
        let body = std::fs::read_to_string(&path).unwrap();
        let record: Feedback = serde_json::from_str(body.lines().next().unwrap()).unwrap();
        assert_eq!(record.text, "sem grep");
        assert_eq!(record.id, fact_id("sem grep"));
        assert!(record.ts_ms > 0);
    }

    #[test]
    fn appending_past_the_ceiling_keeps_the_newest_lines() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(debug_dir(root.path())).unwrap();
        let path = feedback_path(root.path());
        let mut body = String::new();
        let mut n = 0usize;
        while body.len() <= FEEDBACK_MAX_BYTES as usize + 4096 {
            let record = Feedback {
                id: format!("{n:016x}"),
                text: format!("atrito {}", n),
                ..Feedback::default()
            };
            body.push_str(&serde_json::to_string(&record).unwrap());
            body.push('\n');
            n += 1;
        }
        std::fs::write(&path, &body).unwrap();

        append_feedback(
            root.path(),
            &Feedback {
                text: "ultimo atrito".into(),
                ..Feedback::default()
            },
        )
        .unwrap();

        let kept = std::fs::read_to_string(&path).unwrap();
        assert!(
            (kept.len() as u64) <= FEEDBACK_MAX_BYTES / 2 + 512,
            "cortou até a metade do teto: {} bytes",
            kept.len()
        );
        for line in kept.lines() {
            serde_json::from_str::<Feedback>(line).expect("linhas sobrevivem inteiras");
        }
        let last = kept.lines().last().unwrap();
        let last: Feedback = serde_json::from_str(last).unwrap();
        assert_eq!(last.text, "ultimo atrito");
        // o fim do corpo antigo sobrevive, sem linha partida no começo
        let oldest = kept.lines().next().unwrap();
        assert!(body.contains(oldest), "o começo é que foi descartado");
        assert!(body.len() > kept.len());
        assert!(!root.path().join(".agents/tupi/debug/feedback.tmp").exists());
    }

    #[test]
    fn feedback_ids_are_stable_and_records_keep_the_writing_order() {
        let root = tempfile::tempdir().unwrap();
        append_feedback(
            root.path(),
            &Feedback {
                text: "sem grep".into(),
                ..Feedback::default()
            },
        )
        .unwrap();
        append_feedback(
            root.path(),
            &Feedback {
                text: "prompt grande".into(),
                ..Feedback::default()
            },
        )
        .unwrap();

        let records = feedback_records(root.path());
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].text, "sem grep");
        assert_eq!(records[1].text, "prompt grande");
        // o id vem do texto normalizado: caixa e espacos nao geram registro novo
        assert_eq!(records[0].id, fact_id("  SEM    grep "));
        assert_eq!(records[0].id, fact_id("sem grep"));
        assert_ne!(records[0].id, records[1].id);
        assert!(feedback_records(&root.path().join("sem")).is_empty());
    }

    #[test]
    fn a_supplied_id_is_replaced_by_the_normalized_one() {
        let root = tempfile::tempdir().unwrap();
        append_feedback(
            root.path(),
            &Feedback {
                id: "id-escolhido-pelo-chamador".into(),
                text: "  Sem    GREP ".into(),
                ..Feedback::default()
            },
        )
        .unwrap();

        let records = feedback_records(root.path());
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].id, fact_id("sem grep"));
    }

    #[test]
    fn the_same_atrito_written_twice_shares_one_id() {
        let root = tempfile::tempdir().unwrap();
        append_feedback(
            root.path(),
            &Feedback {
                text: "sem grep".into(),
                ..Feedback::default()
            },
        )
        .unwrap();
        append_feedback(
            root.path(),
            &Feedback {
                text: "  SEM   grep ".into(),
                session: "s1".into(),
                ..Feedback::default()
            },
        )
        .unwrap();

        let records = feedback_records(root.path());
        // o log guarda as duas linhas (e historico), mas o id se repete: quem le
        // agrupa por id e nao conta o mesmo atrito duas vezes
        assert_eq!(records.len(), 2);
        let ids: std::collections::BTreeSet<&str> =
            records.iter().map(|record| record.id.as_str()).collect();
        assert_eq!(ids.len(), 1, "caixa e espaco nao geram id novo");
        assert_eq!(records[0].id, fact_id("sem grep"));
        assert_eq!(records[1].session, "s1");
        assert!(records[0].ts_ms > 0 && records[1].ts_ms > 0);
    }
}
