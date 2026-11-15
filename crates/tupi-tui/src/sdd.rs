//! `/sdd <slug>` e a lista de planos retomáveis na TUI.
//!
//! O comando não abre turno: o app reativa o plano, entra no modo SDD e injeta
//! o brief no histórico. Aqui ficam só o parser e a consulta que alimenta o
//! autocomplete.

use tupi_kernel::{Query, QueryBus, QueryResult};

pub const SDD_COMMAND: &str = "/sdd";

/// Separa `/sdd` e `/sdd <slug>` do resto do texto.
///
/// `Some(None)` é o comando sem slug (mostra o uso), `Some(Some(slug))` é a
/// retomada. Devolver `Some` para `/sdd auth` garante que o despacho não caia
/// no "comando desconhecido".
pub fn parse_sdd_invocation(input: &str) -> Option<Option<&str>> {
    let input = input.trim();
    let rest = input.strip_prefix(SDD_COMMAND)?;
    if rest.is_empty() {
        return Some(None);
    }
    let rest = rest.strip_prefix(' ')?.trim();
    // O Tab completa com espaço final; se o usuário digitar mais texto na
    // mesma linha, o slug continua sendo o primeiro token.
    Some(rest.split_whitespace().next())
}

/// Slugs dos planos que aceitam retomada. Falha ou app sem store degradam para
/// lista vazia: o autocomplete fica sem nomes, mas a sessão continua.
pub(crate) async fn sdd_spec_names(queries: &QueryBus) -> Vec<String> {
    match queries.dispatch(Query::ListSddSpecs).await {
        Ok(QueryResult::SddSpecs(specs)) => specs.into_iter().map(|spec| spec.slug).collect(),
        _ => Vec::new(),
    }
}
