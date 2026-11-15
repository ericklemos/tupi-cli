//! Ordem do startup: configuração do projeto e esqueleto de `docs/`.
//!
//! O scaffold precisa rodar depois de `load_or_create` (a língua vem do campo
//! `lang` e o `docs_folders` default é `docs`) e antes de o índice de
//! documentação das tools ser montado, senão a pasta nasce fora do índice.

use std::path::Path;

use crate::docs_scaffold;
use crate::project_config::{self, ProjectConfig};

/// Carrega a configuração do projeto e garante o esqueleto de `docs/`.
///
/// A falha de escrita já é logada dentro do scaffold: o startup segue, porque
/// documentação ausente não é motivo para derrubar o binário.
pub fn load_project_with_docs(root: &Path) -> ProjectConfig {
    let project = project_config::load_or_create(root);
    let lang = project.lang();
    let report = docs_scaffold::scaffold(root, lang);
    tracing::debug!(
        lang = lang.as_str(),
        created = report.created,
        updated = report.updated,
        preserved = report.preserved,
        failed = report.failed,
        "esqueleto de documentação"
    );
    project
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn the_docs_skeleton_exists_before_the_index_is_built() {
        let dir = tempdir().unwrap();

        let project = load_project_with_docs(dir.path());

        // O índice de docs indexa `project.docs_folders(&root)`: a pasta
        // precisa existir com os 5 arquivos antes de a config ser devolvida.
        assert!(dir.path().join("docs/index.md").exists());
        assert_eq!(
            project.docs_folders(dir.path()),
            vec![dir.path().join("docs")]
        );

        // O `lang` do projeto é o que decide os headings do esqueleto.
        let index = std::fs::read_to_string(dir.path().join("docs/index.md")).unwrap();
        let expected_h1 = match project.lang() {
            crate::lang::Lang::PtBr => "# Documentação do projeto\n",
            crate::lang::Lang::En => "# Project documentation\n",
        };
        assert!(index.starts_with(expected_h1), "index: {index}");
    }
}
