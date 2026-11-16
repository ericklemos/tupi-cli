//! Contrato de saída densa do `lookup`: uma única chamada a `dense::dense`
//! (um orçamento, um envelope de `peek`, dedupe pelo `DeliveryLedger`), com as
//! linhas de contexto no topo — quantos itens foram recusados e quantos
//! símbolos da cadeia continuam só como fronteira.

use crate::support::budget::estimate_tokens;
use crate::support::dense::{self, Item};
use crate::support::ledger::DeliveryLedger;
use crate::support::store::ResponseStore;

/// Monta a saída: o cabeçalho (contado no orçamento, nunca cortado) e o corpo
/// denso com o resto. Item com erro já entra no corpo com prioridade de topo.
pub(crate) fn output(
    items: Vec<Item>,
    budget_tokens: usize,
    refusals: usize,
    frontier: usize,
    ledger: &DeliveryLedger,
    store: &ResponseStore,
) -> String {
    let header = header_lines(refusals, frontier);
    let remaining = budget_tokens.saturating_sub(estimate_tokens(&header));
    let body = dense::dense(items, remaining, ledger, store);
    if body.is_empty() {
        return header.trim_end().to_string();
    }
    format!("{header}{body}")
}

/// Duas linhas, cada uma só quando há o que dizer: erro por item e fronteira.
fn header_lines(refusals: usize, frontier: usize) -> String {
    let mut out = String::new();
    if refusals > 0 {
        out.push_str(&format!(
            "> {refusals} item(ns) recusado(s) no topo; o que deu certo segue abaixo.\n\n"
        ));
    }
    if frontier > 0 {
        out.push_str(&format!(
            "> fronteira: {frontier} símbolo(s) da cadeia ainda por abrir — \
             `{{\"kind\":\"symbol\",\"path\":\"…\",\"symbol\":\"…\"}}` ou \
             `{{\"kind\":\"skeleton\",\"path\":\"…\"}}` devolve o corpo.\n\n"
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::support::budget::Section;
    use crate::support::ledger::{DEFAULT_ENTRIES, DEFAULT_MAX_BYTES};

    fn store() -> ResponseStore {
        ResponseStore::new(std::time::Duration::from_secs(600), 8 * 1024 * 1024)
    }

    fn ledger() -> DeliveryLedger {
        DeliveryLedger::new(DEFAULT_ENTRIES, DEFAULT_MAX_BYTES)
    }

    fn plain(heading: &str, body: &str, priority: u8) -> Item {
        Item {
            section: Section {
                heading: heading.to_string(),
                body: body.to_string(),
                priority,
            },
            track: None,
        }
    }

    #[test]
    fn the_frontier_line_says_how_many_symbols_are_still_closed() {
        let out = output(
            vec![plain("#### read: a", "x", 100)],
            2_000,
            0,
            3,
            &ledger(),
            &store(),
        );
        assert!(out.contains("fronteira: 3 símbolo(s)"), "{out}");
        assert!(out.contains("devolve o corpo"), "{out}");
    }

    #[test]
    fn no_frontier_and_no_refusal_means_no_header_line() {
        let out = output(
            vec![plain("#### read: a", "x", 100)],
            2_000,
            0,
            0,
            &ledger(),
            &store(),
        );
        assert!(!out.contains("fronteira:"), "{out}");
        assert!(!out.contains("recusado(s)"), "{out}");
    }

    #[test]
    fn a_refused_item_goes_first_and_the_rest_survives() {
        let out = output(
            vec![
                plain("#### lookup op 0 (symbol) recusada", "erro", 255),
                plain("#### read: ok", "corpo", 100),
            ],
            2_000,
            1,
            0,
            &ledger(),
            &store(),
        );
        let refused = out.find("recusada").expect("recusa");
        let ok = out.find("read: ok").expect("item que deu certo");
        assert!(refused < ok, "{out}");
        assert!(out.contains("1 item(ns) recusado(s)"), "{out}");
    }
}
