//! Interface única de endereço do `edit`: quem responde "como endereçar este
//! arquivo" é o dado, nunca a extensão espalhada em `if`.
//!
//! Âncora literal (`old`+`new`) e faixa de linhas valem para qualquer arquivo de
//! texto; a rota `symbol` só existe onde há coletor de símbolos, e essa é a
//! tabela [`SYMBOL_PARSERS`] — acrescentar linguagem é acrescentar linha nela,
//! não um ramo novo no caminho de escrita. Sem parser a recusa ensina as rotas
//! textuais, que continuam valendo.

use std::path::Path;

use tupi_kernel::ToolError;

/// Extensões com coletor de símbolos, com o nome que a recusa usa.
const SYMBOL_PARSERS: &[(&str, &str)] = &[("rs", "Rust")];

/// Parser de símbolos que serve um arquivo — a linguagem como dado, não como
/// ramo de código.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SymbolParser {
    name: &'static str,
}

impl SymbolParser {
    /// Nome da linguagem, para a recusa e para o relatório.
    pub fn name(self) -> &'static str {
        self.name
    }
}

/// Consulta a tabela: `Some` só quando o arquivo tem parser de símbolos. É a
/// única pergunta de linguagem do `edit`.
pub fn symbol_parser(path: &Path) -> Option<SymbolParser> {
    let extension = path.extension().and_then(|extension| extension.to_str())?;
    SYMBOL_PARSERS
        .iter()
        .find(|(known, _)| *known == extension)
        .map(|(_, name)| SymbolParser { name })
}

/// Recusa que ensina: arquivo sem parser não é arquivo sem escrita cirúrgica.
pub fn no_symbol_parser(path: &str) -> ToolError {
    let known = SYMBOL_PARSERS
        .iter()
        .map(|(extension, name)| format!("{}: {extension}", name.to_lowercase()))
        .collect::<Vec<_>>()
        .join(", ");
    ToolError::Other(format!(
        "`symbol` exige parser de símbolos e `{path}` não tem ({known}). O endereço textual vale para qualquer arquivo de texto: `old`+`new` (âncora literal única) ou `start`+`end`+`expect`+`new` (faixa de linhas). Nada foi escrito."
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    /// A linguagem é dado: responde a extensão, e o `edit` não ramifica.
    #[test]
    fn the_parser_is_selected_by_extension_not_by_a_branch() {
        assert_eq!(
            symbol_parser(Path::new("src/lib.rs")).map(SymbolParser::name),
            Some("Rust")
        );
        assert!(symbol_parser(Path::new("app.py")).is_none());
        assert!(symbol_parser(Path::new("Makefile")).is_none());
    }

    /// A recusa nomeia o que existe e o caminho que continua aberto.
    #[test]
    fn a_file_without_parser_is_refused_with_the_textual_routes() {
        let message = no_symbol_parser("docs/index.md").to_string();
        assert!(message.contains("docs/index.md"), "{message}");
        assert!(message.contains("rust: rs"), "{message}");
        assert!(message.contains("`old`+`new`"), "{message}");
        assert!(message.contains("`start`+`end`"), "{message}");
    }
}
