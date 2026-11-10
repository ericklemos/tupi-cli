//! Memória, com escopo de sessão, do que o agente já recebeu.
//!
//! O processo do Tupi é uma sessão: "já visto" tem escopo sem ambiguidade. O
//! ledger guarda o conteúdo de cada símbolo entregue com o hash do arquivo no
//! momento da entrega e responde três perguntas na próxima chamada:
//!
//! - arquivo não mudou → o corpo é idêntico ao que o agente recebeu e pode ser
//!   apontado em vez de reenviado;
//! - arquivo mudou dentro do símbolo → só as linhas alteradas são novidade;
//! - a entrega anterior foi um recorte → um diff mentiria sobre o resto, então o
//!   corpo vem inteiro.
//!
//! Duas regras mantêm a honestidade: todo ponteiro carrega um handle **vivo**, e
//! um símbolo só é omitido um número limitado de vezes antes de o corpo voltar.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use blake3::Hash;

use crate::budget::estimate_tokens;

pub const DEFAULT_ENTRIES: usize = 512;
pub const DEFAULT_MAX_BYTES: usize = 8 * 1024 * 1024;

/// Depois destas omissões do mesmo símbolo, o corpo é servido de novo.
pub const MAX_OMISSIONS: usize = 2;

/// Acima destas linhas um diff deixa de ser mais barato que o corpo.
pub const DIFF_MAX_LINES: usize = 600;

/// Linhas de contexto inalterado ao redor de cada trecho alterado.
pub const DIFF_CONTEXT: usize = 1;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    Fresh,
    Seen(Seen),
    Changed(Changed),
    Full {
        reason: FullReason,
        previous_tool: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Seen {
    pub text: String,
    pub handle: Option<String>,
    pub tool: String,
    pub hash: String,
    pub lines: usize,
    pub tokens: usize,
    pub file_changed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Changed {
    pub diff: String,
    pub from: String,
    pub to: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FullReason {
    TooManyRepeats,
    PartialDelivery,
    PartialBefore,
    TooLargeToDiff,
}

#[derive(Debug, Clone)]
pub struct Delivery<'a> {
    pub tool: &'a str,
    pub path: &'a Path,
    pub id: &'a str,
    pub file_hash: Hash,
    pub text: &'a str,
    pub complete: bool,
    pub handle: Option<String>,
}

struct Entry {
    tool: String,
    path: PathBuf,
    id: String,
    file_hash: Hash,
    text: String,
    complete: bool,
    handle: Option<String>,
    delivered_at: u64,
    omissions: usize,
    bytes: usize,
}

struct State {
    entries: Vec<Entry>,
    bytes: usize,
    clock: u64,
}

/// Limitado, em memória, por processo. Perder uma entrada só custa um corpo
/// repetido, nunca uma resposta errada.
pub struct DeliveryLedger {
    max_entries: usize,
    max_bytes: usize,
    state: Mutex<State>,
}

impl DeliveryLedger {
    pub fn new(max_entries: usize, max_bytes: usize) -> Self {
        Self {
            max_entries: max_entries.max(1),
            max_bytes: max_bytes.max(1),
            state: Mutex::new(State {
                entries: Vec::new(),
                bytes: 0,
                clock: 0,
            }),
        }
    }

    pub fn check(
        &self,
        path: &Path,
        id: &str,
        file_hash: &Hash,
        current_text: &str,
        start_line: usize,
    ) -> Verdict {
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        state.clock += 1;
        let clock = state.clock;

        let Some(entry) = state
            .entries
            .iter_mut()
            .find(|entry| entry.path == path && entry.id == id)
        else {
            return Verdict::Fresh;
        };
        entry.delivered_at = clock;

        if entry.omissions >= MAX_OMISSIONS {
            entry.omissions = 0;
            return Verdict::Full {
                reason: FullReason::TooManyRepeats,
                previous_tool: entry.tool.clone(),
            };
        }

        if entry.file_hash == *file_hash {
            if !entry.complete {
                return Verdict::Full {
                    reason: FullReason::PartialDelivery,
                    previous_tool: entry.tool.clone(),
                };
            }
            return Verdict::Seen(seen_of(entry, false));
        }

        if !entry.complete {
            return Verdict::Full {
                reason: FullReason::PartialBefore,
                previous_tool: entry.tool.clone(),
            };
        }

        if entry.text.trim_end() == current_text.trim_end() {
            return Verdict::Seen(seen_of(entry, true));
        }

        match diff_lines(&entry.text, current_text, start_line, DIFF_CONTEXT) {
            Some(diff) if !diff.is_empty() => Verdict::Changed(Changed {
                diff,
                from: short_hash(&entry.file_hash),
                to: short_hash(file_hash),
            }),
            Some(_) => Verdict::Seen(seen_of(entry, true)),
            None => Verdict::Full {
                reason: FullReason::TooLargeToDiff,
                previous_tool: entry.tool.clone(),
            },
        }
    }

    pub fn remember_handle(&self, path: &Path, id: &str, handle: &str) {
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        if let Some(entry) = state
            .entries
            .iter_mut()
            .find(|entry| entry.path == path && entry.id == id)
        {
            entry.handle = Some(handle.to_string());
        }
    }

    pub fn note_omitted(&self, path: &Path, id: &str) {
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        if let Some(entry) = state
            .entries
            .iter_mut()
            .find(|entry| entry.path == path && entry.id == id)
        {
            entry.omissions += 1;
        }
    }

    pub fn record(&self, delivery: Delivery<'_>) {
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        state.clock += 1;
        let clock = state.clock;

        let bytes = delivery.text.len() + delivery.path.to_string_lossy().len() + 64;
        if let Some(previous) = state
            .entries
            .iter()
            .position(|entry| entry.path == delivery.path && entry.id == delivery.id)
        {
            state.bytes = state.bytes.saturating_sub(state.entries[previous].bytes);
            state.entries.remove(previous);
        }

        state.entries.push(Entry {
            tool: delivery.tool.to_string(),
            path: delivery.path.to_path_buf(),
            id: delivery.id.to_string(),
            file_hash: delivery.file_hash,
            text: delivery.text.to_string(),
            complete: delivery.complete,
            handle: delivery.handle,
            delivered_at: clock,
            omissions: 0,
            bytes,
        });
        state.bytes += bytes;

        while state.entries.len() > self.max_entries || state.bytes > self.max_bytes {
            if state.entries.len() == 1 {
                break;
            }
            let Some(oldest) = state
                .entries
                .iter()
                .enumerate()
                .min_by_key(|(_, entry)| entry.delivered_at)
                .map(|(index, _)| index)
            else {
                break;
            };
            let entry = state.entries.remove(oldest);
            state.bytes = state.bytes.saturating_sub(entry.bytes);
        }
    }

    pub fn len(&self) -> usize {
        self.state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .entries
            .len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

fn seen_of(entry: &Entry, file_changed: bool) -> Seen {
    Seen {
        text: entry.text.clone(),
        handle: entry.handle.clone(),
        tool: entry.tool.clone(),
        hash: short_hash(&entry.file_hash),
        lines: entry.text.lines().count(),
        tokens: estimate_tokens(&entry.text),
        file_changed,
    }
}

/// O corpo de um ponteiro: diz exatamente o que foi omitido, quanto custa e onde
/// recuperar. Ponteiro sem handle seria beco sem saída.
pub fn pointer_body(seen: &Seen, handle: &str) -> String {
    let mut out = format!(
        "> **Já entregue nesta sessão** ({} · blake3 `{}` · {} linhas) — corpo em `{}` `{handle}`.\n",
        seen.tool,
        seen.hash,
        seen.lines,
        crate::budget::PEEK_TOOL_NAME
    );
    if seen.file_changed {
        out.push_str("> O arquivo mudou fora deste símbolo; o corpo é o mesmo.\n");
    }
    out
}

/// A nota do diff mais os trechos. Se o diff passar do orçamento, o envelope de
/// truncamento e o handle impedem que o corte passe por "nada mais mudou".
pub fn changed_body(changed: &Changed) -> String {
    format!(
        "> **Mudou desde a entrega** (blake3 `{}` → `{}`): só as linhas alteradas abaixo; o resto do corpo é o que você já tem.\n\n```diff\n{}```\n",
        changed.from, changed.to, changed.diff
    )
}

pub fn full_note(reason: FullReason) -> &'static str {
    match reason {
        FullReason::TooManyRepeats => {
            "> Terceira repetição deste símbolo: entregando o corpo em vez de omitir de novo."
        }
        FullReason::PartialDelivery => {
            "> O trecho entregue antes era um recorte do símbolo; segue o corpo inteiro para você não trabalhar com metade."
        }
        FullReason::PartialBefore => {
            "> O trecho entregue antes era um recorte; o arquivo mudou, então segue o corpo atual inteiro."
        }
        FullReason::TooLargeToDiff => {
            "> Símbolo grande demais para diff; segue o corpo atual inteiro."
        }
    }
}

pub fn short_hash(hash: &Hash) -> String {
    hash.to_hex()[..12].to_string()
}

pub fn content_hash(content: &str) -> Hash {
    blake3::hash(content.as_bytes())
}

/// Intervalo de bytes das linhas 1-based `start_line..=end_line`.
#[allow(dead_code)]
pub fn line_span(
    content: &str,
    start_line: usize,
    end_line: usize,
) -> Option<std::ops::Range<usize>> {
    if start_line == 0 || end_line < start_line {
        return None;
    }
    let mut start = None;
    let mut end = None;
    let mut offset = 0usize;
    for (index, line) in content.split_inclusive('\n').enumerate() {
        let number = index + 1;
        if number == start_line {
            start = Some(offset);
        }
        offset += line.len();
        if number >= end_line {
            end = Some(offset);
            break;
        }
    }
    let end = end.or_else(|| start.map(|_| content.len()));
    Some(start?..end?)
}

/// Diff de linhas com `context` linhas inalteradas ao redor de cada trecho, ou
/// `None` quando as entradas são grandes demais para diferenciar barato.
pub fn diff_lines(old: &str, new: &str, old_start_line: usize, context: usize) -> Option<String> {
    let old_lines: Vec<&str> = old.lines().collect();
    let new_lines: Vec<&str> = new.lines().collect();
    if old_lines.len() > DIFF_MAX_LINES || new_lines.len() > DIFF_MAX_LINES {
        return None;
    }

    #[derive(Clone, Copy)]
    enum Op {
        Equal(usize),
        Delete(usize),
        Insert(usize),
    }

    let (n, m) = (old_lines.len(), new_lines.len());
    let mut lcs = vec![vec![0u32; m + 1]; n + 1];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            lcs[i][j] = if old_lines[i] == new_lines[j] {
                lcs[i + 1][j + 1] + 1
            } else {
                lcs[i + 1][j].max(lcs[i][j + 1])
            };
        }
    }

    let mut ops = Vec::with_capacity(n + m);
    let (mut i, mut j) = (0usize, 0usize);
    while i < n && j < m {
        if old_lines[i] == new_lines[j] {
            ops.push(Op::Equal(j));
            i += 1;
            j += 1;
        } else if lcs[i + 1][j] >= lcs[i][j + 1] {
            ops.push(Op::Delete(i));
            i += 1;
        } else {
            ops.push(Op::Insert(j));
            j += 1;
        }
    }
    while i < n {
        ops.push(Op::Delete(i));
        i += 1;
    }
    while j < m {
        ops.push(Op::Insert(j));
        j += 1;
    }

    if !ops.iter().any(|op| !matches!(op, Op::Equal(..))) {
        return Some(String::new());
    }

    let old_index_at = |upto: usize| -> usize {
        ops[..upto]
            .iter()
            .filter(|op| matches!(op, Op::Equal(..) | Op::Delete(..)))
            .count()
    };

    let mut out = String::new();
    let mut idx = 0usize;
    let mut emitted_until = 0usize;
    while idx < ops.len() {
        if matches!(ops[idx], Op::Equal(..)) {
            idx += 1;
            continue;
        }

        let run_start = idx;
        let mut run_end = idx;
        while run_end < ops.len() && !matches!(ops[run_end], Op::Equal(..)) {
            run_end += 1;
        }

        let from = run_start.saturating_sub(context).max(emitted_until);
        let to = (run_end + context).min(ops.len());
        out.push_str(&format!("@@ {} @@\n", old_start_line + old_index_at(from)));
        for op in &ops[from..to] {
            match *op {
                Op::Equal(new_line) => out.push_str(&format!(" {}\n", new_lines[new_line])),
                Op::Delete(old_line) => out.push_str(&format!("-{}\n", old_lines[old_line])),
                Op::Insert(new_line) => out.push_str(&format!("+{}\n", new_lines[new_line])),
            }
        }
        emitted_until = to;
        idx = to;
    }

    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use blake3::Hash;
    use std::path::Path;

    fn hash(text: &str) -> Hash {
        blake3::hash(text.as_bytes())
    }

    fn ledger() -> DeliveryLedger {
        DeliveryLedger::new(DEFAULT_ENTRIES, DEFAULT_MAX_BYTES)
    }

    fn record(ledger: &DeliveryLedger, id: &str, text: &str) {
        ledger.record(Delivery {
            tool: "read",
            path: Path::new("a.rs"),
            id,
            file_hash: hash(text),
            text,
            complete: true,
            handle: Some("h_first".to_string()),
        });
    }

    #[test]
    fn an_unseen_symbol_is_fresh() {
        let ledger = ledger();
        let verdict = ledger.check(Path::new("a.rs"), "f", &hash("x"), "x", 1);
        assert_eq!(verdict, Verdict::Fresh);
    }

    #[test]
    fn the_same_symbol_with_the_same_hash_is_seen() {
        let ledger = ledger();
        let body = "fn a() {}\n";
        record(&ledger, "a", body);

        match ledger.check(Path::new("a.rs"), "a", &hash(body), body, 1) {
            Verdict::Seen(seen) => {
                assert_eq!(seen.text, body);
                assert_eq!(seen.handle.as_deref(), Some("h_first"));
                assert_eq!(seen.tool, "read");
                assert!(!seen.file_changed);
            }
            other => panic!("expected Seen, got {other:?}"),
        }
    }

    #[test]
    fn a_changed_file_yields_only_the_changed_lines() {
        let ledger = ledger();
        let old = "fn a() {\n    let x = 1;\n    let y = 2;\n}\n";
        let new = "fn a() {\n    let x = 1;\n    let y = 3;\n}\n";
        record(&ledger, "a", old);

        match ledger.check(Path::new("a.rs"), "a", &hash(new), new, 10) {
            Verdict::Changed(changed) => {
                assert!(changed.diff.contains("-    let y = 2;"), "{}", changed.diff);
                assert!(changed.diff.contains("+    let y = 3;"), "{}", changed.diff);
                assert!(!changed.diff.contains("-fn a"));
                assert!(changed.diff.contains("@@ 11 @@"), "{}", changed.diff);
            }
            other => panic!("expected Changed, got {other:?}"),
        }
    }

    #[test]
    fn an_edited_symbol_keeps_its_identity_when_lines_move() {
        let ledger = ledger();
        let body = "fn a() {\n    1\n}\n";
        record(&ledger, "a", body);
        let new = "fn a() {\n    1 + 1\n}\n";
        match ledger.check(Path::new("a.rs"), "a", &hash(new), new, 40) {
            Verdict::Changed(changed) => assert!(changed.diff.contains("+    1 + 1")),
            other => panic!("expected Changed, got {other:?}"),
        }
    }

    #[test]
    fn a_change_outside_the_symbol_is_still_a_pointer() {
        let ledger = ledger();
        let body = "fn a() {}\n";
        record(&ledger, "a", body);

        match ledger.check(Path::new("a.rs"), "a", &hash("other"), body, 1) {
            Verdict::Seen(seen) => assert!(seen.file_changed),
            other => panic!("expected Seen, got {other:?}"),
        }
    }

    #[test]
    fn a_partial_delivery_is_served_whole_even_when_unchanged() {
        let ledger = ledger();
        ledger.record(Delivery {
            tool: "read",
            path: Path::new("a.rs"),
            id: "long_fn",
            file_hash: hash("v1"),
            text: "recorte de 10 linhas",
            complete: false,
            handle: None,
        });

        match ledger.check(Path::new("a.rs"), "long_fn", &hash("v1"), "v1 inteiro", 1) {
            Verdict::Full { reason, .. } => assert_eq!(reason, FullReason::PartialDelivery),
            other => panic!("expected Full, got {other:?}"),
        }
    }

    #[test]
    fn a_partial_delivery_is_never_diffed() {
        let ledger = ledger();
        ledger.record(Delivery {
            tool: "read",
            path: Path::new("a.rs"),
            id: "long_fn",
            file_hash: hash("v1"),
            text: "primeiras 10 linhas",
            complete: false,
            handle: None,
        });

        match ledger.check(Path::new("a.rs"), "long_fn", &hash("v2"), "v2 inteiro", 1) {
            Verdict::Full { reason, .. } => assert_eq!(reason, FullReason::PartialBefore),
            other => panic!("expected Full, got {other:?}"),
        }
    }

    #[test]
    fn a_symbol_is_not_omitted_forever() {
        let ledger = ledger();
        let body = "fn a() {}\n";
        record(&ledger, "a", body);

        for _ in 0..MAX_OMISSIONS {
            assert!(matches!(
                ledger.check(Path::new("a.rs"), "a", &hash(body), body, 1),
                Verdict::Seen(_)
            ));
            ledger.note_omitted(Path::new("a.rs"), "a");
        }

        match ledger.check(Path::new("a.rs"), "a", &hash(body), body, 1) {
            Verdict::Full { reason, .. } => assert_eq!(reason, FullReason::TooManyRepeats),
            other => panic!("expected Full, got {other:?}"),
        }
        assert!(matches!(
            ledger.check(Path::new("a.rs"), "a", &hash(body), body, 1),
            Verdict::Seen(_)
        ));
    }

    #[test]
    fn recording_the_same_symbol_replaces_the_previous_entry() {
        let ledger = ledger();
        record(&ledger, "a", "v1");
        record(&ledger, "a", "v2");
        assert_eq!(ledger.len(), 1);

        match ledger.check(Path::new("a.rs"), "a", &hash("v2"), "v2", 1) {
            Verdict::Seen(seen) => assert_eq!(seen.text, "v2"),
            other => panic!("expected Seen, got {other:?}"),
        }
    }

    #[test]
    fn the_oldest_entry_is_evicted_at_the_ceiling() {
        let ledger = DeliveryLedger::new(2, DEFAULT_MAX_BYTES);
        record(&ledger, "a", "aaa");
        record(&ledger, "b", "bbb");
        record(&ledger, "c", "ccc");

        assert_eq!(ledger.len(), 2);
        assert!(matches!(
            ledger.check(Path::new("a.rs"), "a", &hash("aaa"), "aaa", 1),
            Verdict::Fresh
        ));
    }

    #[test]
    fn a_single_entry_survives_a_tiny_byte_ceiling() {
        let ledger = DeliveryLedger::new(DEFAULT_ENTRIES, 1);
        record(&ledger, "a", "some body");
        assert_eq!(ledger.len(), 1);
    }

    #[test]
    fn a_remembered_handle_is_returned() {
        let ledger = ledger();
        record(&ledger, "a", "body");
        ledger.remember_handle(Path::new("a.rs"), "a", "h_new");

        match ledger.check(Path::new("a.rs"), "a", &hash("body"), "body", 1) {
            Verdict::Seen(seen) => assert_eq!(seen.handle.as_deref(), Some("h_new")),
            other => panic!("expected Seen, got {other:?}"),
        }
    }

    #[test]
    fn a_line_span_covers_whole_lines() {
        let content = "a\nbb\nccc\n";
        assert_eq!(&content[line_span(content, 2, 3).unwrap()], "bb\nccc\n");
        assert_eq!(&content[line_span(content, 1, 1).unwrap()], "a\n");
        assert_eq!(&content[line_span(content, 3, 9).unwrap()], "ccc\n");
        assert!(line_span(content, 9, 10).is_none());
    }

    #[test]
    fn diff_shows_context_and_never_splits_a_hunk() {
        let old = "a\nb\nc\nd\ne\n";
        let new = "a\nb\nC\nd\ne\n";
        let diff = diff_lines(old, new, 1, 1).unwrap();
        assert!(diff.contains("@@ 2 @@"), "{diff}");
        assert!(diff.contains(" b\n-c\n+C\n d\n"), "{diff}");
    }

    #[test]
    fn identical_texts_produce_an_empty_diff() {
        assert_eq!(diff_lines("a\nb\n", "a\nb\n", 1, 1).unwrap(), "");
    }

    #[test]
    fn huge_spans_are_not_diffed() {
        let old = "a\n".repeat(DIFF_MAX_LINES + 1);
        let new = "b\n".repeat(DIFF_MAX_LINES + 1);
        assert!(diff_lines(&old, &new, 1, 1).is_none());
    }

    #[test]
    fn qualified_ids_do_not_collide_in_the_same_file() {
        let ledger = ledger();
        record(&ledger, "A::new", "v1");
        record(&ledger, "B::new", "v2");

        assert!(matches!(
            ledger.check(Path::new("a.rs"), "A::new", &hash("v1"), "v1", 1),
            Verdict::Seen(_)
        ));
        assert!(matches!(
            ledger.check(Path::new("a.rs"), "B::new", &hash("v2"), "v2", 1),
            Verdict::Seen(_)
        ));
    }
}
