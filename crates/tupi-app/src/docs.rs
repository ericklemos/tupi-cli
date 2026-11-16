//! Postura do modo docs: investigar o acervo sem editar nada.

/// Instruções de sistema do modo docs.
///
/// O modo promete cobertura: nenhuma ausência pode ser afirmada sem que o
/// acervo esteja indexado. O texto abaixo é o contrato que o modelo recebe.
pub const DOCS_AGENT_PREFIX: &str = "## Modo docs

Você investiga um acervo persistente de arquivos (texto, imagem, vídeo). Regras:

1. Nunca edite nem crie arquivos dentro das raízes do acervo. Você só lê.
2. Toda afirmação sobre o acervo precisa vir com localizador: fonte e linha,
   página ou timestamp.
3. Nunca afirme que algo não existe sem antes consultar a cobertura do índice.
   Se a cobertura estiver incompleta, diga exatamente o que falta.
4. A varredura é determinística: quem decide onde procurar é o índice, não você.
   Seu trabalho é julgar cada candidato e recusar os que não são a mesma pessoa
   ou o mesmo assunto.
5. Prefira respostas em formato estruturado. Prosa livre só no relatório final.";

/// Compõe o prompt do projeto com a postura do modo docs.
pub fn compose_docs_prefix(base: &str) -> String {
    if base.trim().is_empty() {
        DOCS_AGENT_PREFIX.to_string()
    } else {
        format!("{base}\n\n{DOCS_AGENT_PREFIX}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compose_docs_prefix_keeps_the_project_prompt() {
        let composed = compose_docs_prefix("PROJETO");
        assert!(composed.starts_with("PROJETO"));
        assert!(composed.contains(DOCS_AGENT_PREFIX));
    }

    #[test]
    fn compose_docs_prefix_falls_back_to_the_posture_alone() {
        assert_eq!(compose_docs_prefix("   "), DOCS_AGENT_PREFIX);
    }
}
