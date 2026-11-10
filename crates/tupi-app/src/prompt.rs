pub const TUPI_SYSTEM: &str = r#"Agente de código Tupi.

1. Descoberta: o prompt não contém o repositório. Use `map(depth="dirs")` -> `map(depth="files", path)` -> `read`/`search`. Não presuma caminhos fora do `map`.
2. Verdade: tool vence memória. Não afirme conteúdo não lido na sessão; se o arquivo mudou, vale a tool.
3. Edição: `edit` com referência e hash; nunca reescreva arquivo inteiro para mudar um trecho.
4. Saída: idioma do usuário, direto, citando `caminho:linha` sem repetir conteúdo já entregue.
"#;

pub fn compose(system: &str) -> String {
    system.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compose_returns_the_system_prompt_verbatim() {
        let out = compose(TUPI_SYSTEM);
        assert_eq!(out, TUPI_SYSTEM);
        assert!(TUPI_SYSTEM.contains("`map(depth=\"dirs\")`"));
        assert!(!out.contains("root: ."));
    }

    #[test]
    fn compose_uses_the_given_system_prompt() {
        let out = compose("PERSONA");
        assert_eq!(out, "PERSONA");
    }

    #[test]
    fn the_system_prompt_no_longer_points_at_a_repo_index() {
        assert!(!TUPI_SYSTEM.contains("índice do repositório"));
        assert!(!TUPI_SYSTEM.contains("mapa do início da sessão"));
    }

    #[test]
    fn compose_is_pure() {
        assert_eq!(compose(TUPI_SYSTEM), compose(TUPI_SYSTEM));
    }
}
