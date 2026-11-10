pub const TUPI_SYSTEM: &str = r#"# Tupi

Você é o Tupi, agente de código dentro do repositório do usuário.

## Regras
- Tool é a fonte de verdade. O índice do repositório é um mapa do início da sessão: use para decidir onde olhar, nunca como se fosse o conteúdo do arquivo.
- Nunca afirme o conteúdo de um arquivo que você não leu nesta sessão.
- Quando uma tool disser que o arquivo mudou, a versão atual é a da tool. Não tente reconciliar de memória.
- Edite por tool com referência e hash. Nunca reescreva um arquivo inteiro para mudar um trecho.
- Cite caminho e linha; não repita conteúdo já entregue.
- Responda no idioma do usuário.
"#;

pub fn compose(index_block: &str) -> String {
    if index_block.trim().is_empty() {
        return TUPI_SYSTEM.to_string();
    }

    format!("{TUPI_SYSTEM}\n## Índice do repositório\n\n{index_block}\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compose_puts_the_index_after_the_system_prompt() {
        let out = compose("root: .\nsrc/lib.rs :: alpha");
        assert!(out.starts_with(TUPI_SYSTEM));
        assert!(out.find("root: .").unwrap() > TUPI_SYSTEM.len());
    }

    #[test]
    fn compose_without_index_is_only_the_system_prompt() {
        assert_eq!(compose(""), TUPI_SYSTEM);
        assert_eq!(compose("  \n"), TUPI_SYSTEM);
    }

    #[test]
    fn compose_is_pure() {
        assert_eq!(compose("x"), compose("x"));
    }
}
