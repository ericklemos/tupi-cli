//! Contrato denso canônico das tools de leitura.
//!
//! Um lugar só decide o que sai: ordena as seções por prioridade, corta pelo
//! orçamento, guarda o excedente no [`ResponseStore`] (recuperável por `peek`)
//! e registra no [`DeliveryLedger`] o que foi entregue — seção já entregue vira
//! ponteiro, seção cortada vira omissão anotada. A economia vem do cap por
//! seção e da honestidade do corte, não do número de chamadas.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use super::budget::{self, Section};
use super::ledger::{self, Delivery, DeliveryLedger, Seen, Verdict};
use super::store::ResponseStore;

/// Uma seção pronta para sair, com a identidade que o ledger precisa.
pub(crate) struct Item {
    pub section: Section,
    /// `None` para o que não é entrega de corpo (aviso, fronteira, erro de item).
    pub track: Option<Track>,
}

/// Identidade de um corpo entregável: o que o [`DeliveryLedger`] usa para saber
/// que a mesma seção já saiu nesta sessão.
pub(crate) struct Track {
    pub tool: &'static str,
    pub path: PathBuf,
    pub id: String,
    pub file_hash: blake3::Hash,
    pub text: String,
    pub complete: bool,
    pub start_line: usize,
    /// `true` reexibe o corpo: nem ponteiro, nem consulta ao ledger.
    pub repeat_allowed: bool,
}

/// Caminho único de saída: prioridade → orçamento → handle → ledger.
pub(crate) fn dense(
    items: Vec<Item>,
    budget_tokens: usize,
    ledger: &DeliveryLedger,
    store: &ResponseStore,
) -> String {
    let mut items = items;
    let mut pointed: HashSet<usize> = HashSet::new();
    apply_ledger(&mut items, ledger, store, &mut pointed);

    // Prioridade manda na ordem e no corte; empate mantém a ordem do chamador.
    let mut order: Vec<usize> = (0..items.len()).collect();
    order.sort_by(|&a, &b| items[b].section.priority.cmp(&items[a].section.priority));

    let sections: Vec<Section> = order
        .iter()
        .map(|&idx| items[idx].section.clone())
        .collect();
    let selection = budget::select(&sections, budget_tokens);
    let handle = if selection.omitted.is_empty() {
        None
    } else {
        Some(store.insert(selection.omitted_body()))
    };

    settle(&items, &selection, ledger, &pointed);

    budget::render(&selection, handle.as_deref())
}

fn apply_ledger(
    items: &mut [Item],
    ledger: &DeliveryLedger,
    store: &ResponseStore,
    pointed: &mut HashSet<usize>,
) {
    for (idx, item) in items.iter_mut().enumerate() {
        let Some(track) = &item.track else {
            continue;
        };
        if track.repeat_allowed {
            continue;
        }
        match ledger.check(
            &track.path,
            &track.id,
            &track.file_hash,
            &track.text,
            track.start_line,
        ) {
            Verdict::Seen(seen) => {
                let handle = live_handle(&seen, &track.path, &track.id, ledger, store);
                item.section.body = ledger::pointer_body(&seen, &handle);
                pointed.insert(idx);
            }
            Verdict::Changed(changed) => {
                item.section.body = ledger::changed_body(&changed);
            }
            Verdict::Full { reason, .. } => {
                item.section.body = format!("{}\n{}", ledger::full_note(reason), item.section.body);
            }
            Verdict::Fresh => {}
        }
    }
}

fn settle(
    items: &[Item],
    selection: &budget::Selection,
    ledger: &DeliveryLedger,
    pointed: &HashSet<usize>,
) {
    let included: HashSet<&str> = selection
        .included
        .iter()
        .map(|section| section.heading.as_str())
        .collect();

    for (idx, item) in items.iter().enumerate() {
        if pointed.contains(&idx) {
            continue;
        }
        let Some(track) = &item.track else {
            continue;
        };
        if !included.contains(item.section.heading.as_str()) {
            // Cortada pelo orçamento ainda é entrega parcial: a próxima leitura
            // devolve o corpo inteiro com handle em vez de repetir o truncado.
            ledger.note_omitted(&track.path, &track.id);
            continue;
        }
        ledger.record(Delivery {
            tool: track.tool,
            path: &track.path,
            id: &track.id,
            file_hash: track.file_hash,
            text: &track.text,
            complete: track.complete,
            handle: None,
        });
    }
}

fn live_handle(
    seen: &Seen,
    path: &Path,
    id: &str,
    ledger: &DeliveryLedger,
    store: &ResponseStore,
) -> String {
    if let Some(handle) = &seen.handle
        && store.get(handle).is_some()
    {
        return handle.clone();
    }
    let handle = store.insert(seen.text.clone());
    ledger.remember_handle(path, id, &handle);
    handle
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::budget::{Section, DEFAULT_BUDGET_TOKENS};

    fn store() -> ResponseStore {
        ResponseStore::new(Duration::from_secs(600), 8 * 1024 * 1024)
    }

    fn ledger() -> DeliveryLedger {
        DeliveryLedger::new(ledger::DEFAULT_ENTRIES, ledger::DEFAULT_MAX_BYTES)
    }

    fn section(heading: &str, body: &str, priority: u8) -> Section {
        Section {
            heading: heading.to_string(),
            body: body.to_string(),
            priority,
        }
    }

    fn plain(heading: &str, body: &str, priority: u8) -> Item {
        Item {
            section: section(heading, body, priority),
            track: None,
        }
    }

    fn tracked(text: &str, id: &str, body: &str) -> Item {
        Item {
            section: section("### trecho", body, 255),
            track: Some(Track {
                tool: "read",
                path: Path::new("a.rs").to_path_buf(),
                id: id.to_string(),
                file_hash: ledger::content_hash(text),
                text: text.to_string(),
                complete: true,
                start_line: 1,
                repeat_allowed: false,
            }),
        }
    }

    /// Seções na ordem de leitura do output, sem o envelope.
    fn headings(out: &str) -> Vec<&str> {
        out.lines()
            .filter(|line| line.starts_with("###"))
            .collect()
    }

    #[test]
    fn the_highest_priority_section_comes_first() {
        let store = store();
        let ledger = ledger();
        let out = dense(
            vec![
                plain("### baixa", "conteudo de baixa prioridade", 10),
                plain("### alta", "conteudo de alta prioridade", 250),
                plain("### media", "conteudo de media prioridade", 100),
            ],
            DEFAULT_BUDGET_TOKENS,
            &ledger,
            &store,
        );
        assert_eq!(
            headings(&out),
            vec!["### alta", "### media", "### baixa"],
            "{out}"
        );
    }

    #[test]
    fn the_highest_priority_survives_a_budget_that_fits_one_section() {
        let store = store();
        let ledger = ledger();
        let out = dense(
            vec![
                plain("### baixa", "conteudo de baixa prioridade", 10),
                plain("### alta", "conteudo de alta prioridade", 250),
            ],
            1,
            &ledger,
            &store,
        );
        assert_eq!(headings(&out), vec!["### alta"], "{out}");
        assert!(!out.contains("conteudo de baixa prioridade"), "{out}");
    }

    #[test]
    fn what_does_not_fit_becomes_a_handle_for_peek() {
        let store = store();
        let ledger = ledger();
        let out = dense(
            vec![
                plain("### cabe", "curto", 255),
                plain("### sobra", "corpo grande da sobra", 10),
            ],
            4,
            &ledger,
            &store,
        );

        assert!(out.contains("`peek`"), "{out}");
        assert!(!out.contains("corpo grande da sobra"), "{out}");

        let handle = out
            .split("handle: `")
            .nth(1)
            .and_then(|rest| rest.split('`').next())
            .unwrap_or_else(|| panic!("handle ausente no envelope: {out}"));
        let body = store.get(handle).expect("handle recuperável pelo peek");
        assert!(body.contains("### sobra"), "{body}");
        assert!(body.contains("corpo grande da sobra"), "{body}");
    }

    #[test]
    fn a_section_already_delivered_becomes_a_pointer() {
        let store = store();
        let ledger = ledger();
        let text = "fn foo() {}\n";
        let path = Path::new("a.rs");
        let file_hash = ledger::content_hash(text);
        ledger.record(Delivery {
            tool: "read",
            path,
            id: "foo",
            file_hash,
            text,
            complete: true,
            handle: None,
        });

        let out = dense(
            vec![Item {
                section: section("### `foo`", "corpo que nao deveria sair", 255),
                track: Some(Track {
                    tool: "read",
                    path: path.to_path_buf(),
                    id: "foo".to_string(),
                    file_hash,
                    text: text.to_string(),
                    complete: true,
                    start_line: 1,
                    repeat_allowed: false,
                }),
            }],
            DEFAULT_BUDGET_TOKENS,
            &ledger,
            &store,
        );

        assert!(out.contains("Já entregue nesta sessão"), "{out}");
        assert!(!out.contains("corpo que nao deveria sair"), "{out}");

        let handle = out
            .split("`peek` `")
            .nth(1)
            .and_then(|rest| rest.split('`').next())
            .unwrap_or_else(|| panic!("handle ausente no ponteiro: {out}"));
        assert_eq!(store.get(handle).as_deref(), Some(text));
    }

    #[test]
    fn an_omitted_delivery_is_remembered_as_partial() {
        let store = store();
        let ledger = ledger();
        let text = "fn bar() {}\n";
        let out = dense(
            vec![
                plain("### cabe", "curto", 255),
                tracked(text, "bar", "corpo grande que nao cabe"),
            ],
            4,
            &ledger,
            &store,
        );
        assert!(!out.contains("corpo grande que nao cabe"), "{out}");
        // Cortar não inventa entrega: item nunca entregue e omitido não entra
        // no ledger como corpo completo (só conta omissão de entrega anterior).
        assert!(ledger.is_empty());
    }

    #[test]
    fn a_delivered_section_is_registered_once() {
        let store = store();
        let ledger = ledger();
        let text = "fn baz() {}\n";
        let _ = dense(
            vec![tracked(text, "baz", "```rust\nfn baz() {}\n```")],
            DEFAULT_BUDGET_TOKENS,
            &ledger,
            &store,
        );
        assert_eq!(ledger.len(), 1);

        let again = dense(
            vec![tracked(text, "baz", "```rust\nfn baz() {}\n```")],
            DEFAULT_BUDGET_TOKENS,
            &ledger,
            &store,
        );
        assert!(again.contains("Já entregue nesta sessão"), "{again}");
    }
}
