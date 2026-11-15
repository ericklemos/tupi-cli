//! Esqueleto automático de `docs/` no startup.
//!
//! Garante os 5 arquivos gerenciados de `<root>/docs` com um H1 de propósito e
//! H2 nomeados, sem corpo. O índice de docs chunka por seção de heading, e o
//! breadcrumb do H1/H2 é o identificador do pedaço recuperado: por isso o
//! esqueleto nasce com H2 nomeados, e não vazio de verdade.
//!
//! Política de escrita: documentação é artefato vivo. Só nasce arquivo ausente
//! ou arquivo cujo conteúdo ainda é exatamente um esqueleto gerado; arquivo com
//! qualquer linha além dos headings é autoral e fica intocado.

use std::path::Path;

use crate::lang::Lang;

/// Um arquivo gerenciado: caminho relativo ao root e esqueleto por língua.
struct Template {
    relative: &'static str,
    pt_br: &'static str,
    en: &'static str,
}

impl Template {
    fn for_lang(&self, lang: Lang) -> &'static str {
        match lang {
            Lang::PtBr => self.pt_br,
            Lang::En => self.en,
        }
    }
}

/// Os únicos caminhos que o scaffold escreve.
///
/// Nada de seção de tools: a lista de tools já é gravada no AGENTS.md e na
/// skill pelo `setup`, e duplicá-la aqui só criaria duas verdades.
const TEMPLATES: &[Template] = &[
    Template {
        relative: "docs/index.md",
        pt_br: "# Documentação do projeto\n\n## Arquitetura\n\n## Regras de negócio\n\n## Convenções\n",
        en: "# Project documentation\n\n## Architecture\n\n## Business rules\n\n## Conventions\n",
    },
    Template {
        relative: "docs/architecture.md",
        pt_br: "# Arquitetura\n\n## Camadas\n\n## Fronteiras\n\n## Componentes\n",
        en: "# Architecture\n\n## Layers\n\n## Boundaries\n\n## Components\n",
    },
    Template {
        relative: "docs/business.md",
        pt_br: "# Regras de negócio\n\n## Entidades\n\n## Invariantes\n\n## Fluxos\n",
        en: "# Business rules\n\n## Entities\n\n## Invariants\n\n## Flows\n",
    },
    Template {
        relative: "docs/conventions.md",
        pt_br: "# Convenções\n\n## Onde cada coisa mora\n\n## Testes\n\n## Commits\n",
        en: "# Conventions\n\n## Where things live\n\n## Tests\n\n## Commits\n",
    },
    Template {
        relative: "docs/decisions/0000-template.md",
        pt_br: "# ADR 0000: modelo de decisão\n\n## Contexto\n\n## Decisão\n\n## Alternativas rejeitadas\n\n## Consequências\n",
        en: "# ADR 0000: decision template\n\n## Context\n\n## Decision\n\n## Rejected alternatives\n\n## Consequences\n",
    },
];

/// Resultado do scaffold, para o log de startup.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Report {
    /// Arquivos ausentes que nasceram com o esqueleto.
    pub created: usize,
    /// Arquivos que ainda eram esqueleto e passaram para a língua atual.
    pub updated: usize,
    /// Arquivos com prosa autoral ou esqueleto já atual.
    pub preserved: usize,
    /// Arquivos que não puderam ser lidos nem escritos.
    pub failed: usize,
}

enum Sync {
    Created,
    Updated,
    Preserved,
}

/// Garante o esqueleto dos 5 caminhos gerenciados de `docs/`.
///
/// Falha de I/O não derruba o startup: vira `tracing::warn` e o arquivo é
/// contado em [`Report::failed`], como o fallback de `load_or_create`.
pub fn scaffold(root: &Path, lang: Lang) -> Report {
    let mut report = Report::default();
    for template in TEMPLATES {
        match sync(root, template, lang) {
            Ok(Sync::Created) => report.created += 1,
            Ok(Sync::Updated) => report.updated += 1,
            Ok(Sync::Preserved) => report.preserved += 1,
            Err(error) => {
                report.failed += 1;
                tracing::warn!(
                    %error,
                    path = %root.join(template.relative).display(),
                    "não foi possível escrever o esqueleto de documentação"
                );
            }
        }
    }
    report
}

fn sync(root: &Path, template: &Template, lang: Lang) -> std::io::Result<Sync> {
    let path = root.join(template.relative);
    let expected = template.for_lang(lang);
    match std::fs::read(&path) {
        Ok(current) => {
            // Byte a byte, nas duas línguas: qualquer outra linha é prosa
            // autoral e não se toca, nem para "melhorar".
            if current != template.pt_br.as_bytes() && current != template.en.as_bytes() {
                return Ok(Sync::Preserved);
            }
            // Esqueleto já na língua atual: não reescreve, senão o `mtime`
            // invalidaria o índice de docs a cada startup.
            if current == expected.as_bytes() {
                return Ok(Sync::Preserved);
            }
            std::fs::write(&path, expected)?;
            Ok(Sync::Updated)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(&path, expected)?;
            Ok(Sync::Created)
        }
        Err(error) => Err(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    use tempfile::tempdir;

    fn templates() -> impl Iterator<Item = (&'static str, &'static str, &'static str)> {
        TEMPLATES
            .iter()
            .map(|template| (template.relative, template.pt_br, template.en))
    }

    fn h2(text: &str) -> Vec<&str> {
        text.lines()
            .filter(|line| line.starts_with("## "))
            .collect()
    }

    fn files_under(root: &Path) -> Vec<String> {
        fn walk(directory: &Path, root: &Path, found: &mut Vec<String>) {
            let Ok(entries) = std::fs::read_dir(directory) else {
                return;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    walk(&path, root, found);
                } else if let Ok(relative) = path.strip_prefix(root) {
                    found.push(relative.to_string_lossy().replace('\\', "/"));
                }
            }
        }
        let mut found = Vec::new();
        walk(root, root, &mut found);
        found.sort();
        found
    }

    #[test]
    fn the_skeleton_is_only_headings() {
        for (relative, pt_br, en) in templates() {
            for text in [pt_br, en] {
                let headings = text.lines().filter(|line| line.starts_with('#')).count();
                let body = text
                    .lines()
                    .filter(|line| !line.is_empty() && !line.starts_with('#'))
                    .count();
                assert_eq!(body, 0, "{relative} ganhou corpo: {text}");
                assert_eq!(
                    text.lines().filter(|line| line.starts_with("# ")).count(),
                    1
                );
                assert!(headings > 1, "{relative} sem H2: {text}");
            }
        }
    }

    #[test]
    fn the_sections_are_the_named_ones() {
        let index = TEMPLATES[0].pt_br;
        assert_eq!(
            h2(index),
            ["## Arquitetura", "## Regras de negócio", "## Convenções"]
        );

        let architecture = TEMPLATES[1].pt_br;
        assert_eq!(
            h2(architecture),
            ["## Camadas", "## Fronteiras", "## Componentes"]
        );

        let business = TEMPLATES[2].pt_br;
        assert_eq!(
            h2(business),
            ["## Entidades", "## Invariantes", "## Fluxos"]
        );

        let conventions = TEMPLATES[3].pt_br;
        assert_eq!(
            h2(conventions),
            ["## Onde cada coisa mora", "## Testes", "## Commits"]
        );

        let decisions = TEMPLATES[4].pt_br;
        assert_eq!(
            h2(decisions),
            [
                "## Contexto",
                "## Decisão",
                "## Alternativas rejeitadas",
                "## Consequências"
            ]
        );

        // A mesma estrutura em inglês, para o esqueleto não virar meio-traduzido.
        assert_eq!(
            h2(TEMPLATES[0].en),
            ["## Architecture", "## Business rules", "## Conventions"]
        );
        assert_eq!(
            h2(TEMPLATES[1].en),
            ["## Layers", "## Boundaries", "## Components"]
        );
        assert_eq!(
            h2(TEMPLATES[2].en),
            ["## Entities", "## Invariants", "## Flows"]
        );
        assert_eq!(
            h2(TEMPLATES[3].en),
            ["## Where things live", "## Tests", "## Commits"]
        );
        assert_eq!(
            h2(TEMPLATES[4].en),
            [
                "## Context",
                "## Decision",
                "## Rejected alternatives",
                "## Consequences"
            ]
        );
    }

    #[test]
    fn an_absent_folder_is_seeded_with_the_language_of_the_project() {
        let dir = tempdir().unwrap();
        let report = scaffold(dir.path(), Lang::PtBr);
        assert_eq!(
            report,
            Report {
                created: 5,
                updated: 0,
                preserved: 0,
                failed: 0
            }
        );
        assert_eq!(
            files_under(dir.path()),
            [
                "docs/architecture.md",
                "docs/business.md",
                "docs/conventions.md",
                "docs/decisions/0000-template.md",
                "docs/index.md",
            ]
        );
        assert_eq!(
            std::fs::read_to_string(dir.path().join("docs/index.md")).unwrap(),
            TEMPLATES[0].pt_br
        );
    }

    #[test]
    fn the_skeleton_is_not_rewritten_on_the_second_run() {
        let dir = tempdir().unwrap();
        scaffold(dir.path(), Lang::PtBr);
        let path = dir.path().join("docs/index.md");
        let modified = std::fs::metadata(&path).unwrap().modified().unwrap();

        std::thread::sleep(std::time::Duration::from_millis(10));
        let report = scaffold(dir.path(), Lang::PtBr);
        assert_eq!(
            report,
            Report {
                created: 0,
                updated: 0,
                preserved: 5,
                failed: 0
            }
        );
        assert_eq!(
            std::fs::metadata(&path).unwrap().modified().unwrap(),
            modified
        );
    }

    #[test]
    fn authored_content_is_never_touched() {
        let dir = tempdir().unwrap();
        let docs = dir.path().join("docs");
        std::fs::create_dir_all(&docs).unwrap();

        // Nota fora dos 5 caminhos e arquivo gerenciado já com prosa.
        std::fs::write(docs.join("notas.md"), "rascunho\n").unwrap();
        let authored = format!("{}Uma camada a mais.\n", TEMPLATES[1].pt_br);
        std::fs::write(docs.join("architecture.md"), &authored).unwrap();
        let notes = std::fs::read_to_string(docs.join("notas.md")).unwrap();

        let report = scaffold(dir.path(), Lang::PtBr);
        assert_eq!(
            report,
            Report {
                created: 4,
                updated: 0,
                preserved: 1,
                failed: 0
            }
        );
        assert_eq!(
            std::fs::read_to_string(docs.join("architecture.md")).unwrap(),
            authored
        );
        assert_eq!(
            std::fs::read_to_string(docs.join("notas.md")).unwrap(),
            notes
        );
        assert_eq!(
            files_under(dir.path()),
            [
                "docs/architecture.md",
                "docs/business.md",
                "docs/conventions.md",
                "docs/decisions/0000-template.md",
                "docs/index.md",
                "docs/notas.md",
            ]
        );
    }

    #[test]
    fn a_language_switch_updates_a_pure_skeleton() {
        let dir = tempdir().unwrap();
        scaffold(dir.path(), Lang::PtBr);

        let report = scaffold(dir.path(), Lang::En);
        assert_eq!(
            report,
            Report {
                created: 0,
                updated: 5,
                preserved: 0,
                failed: 0
            }
        );
        assert_eq!(
            std::fs::read_to_string(dir.path().join("docs/index.md")).unwrap(),
            TEMPLATES[0].en
        );
    }

    #[test]
    fn a_failing_write_does_not_stop_the_startup() {
        let dir = tempdir().unwrap();
        // `docs` como arquivo comum: nenhum dos 5 caminhos pode ser escrito.
        std::fs::write(dir.path().join("docs"), "arquivo qualquer").unwrap();

        let report = scaffold(dir.path(), Lang::En);
        assert_eq!(
            report,
            Report {
                created: 0,
                updated: 0,
                preserved: 0,
                failed: 5
            }
        );
    }
}
