# Pasta de documentação precisa estar declarada em `docs_folders`

**Context:** `ProjectConfig::docs_folders` (`crates/tupi/src/project_config.rs:47-52`) resolve a lista lida de `ai.project.json`; `DocsIndex::new` e `refresh` (`crates/tupi-tools/src/tools/docs/index.rs:58-69`) percorrem exatamente essas pastas e nada além delas. O campo é `#[serde(default)]` (`project_config.rs:21-40`): ausente no arquivo, vira lista vazia — e a documentação passa a existir no git sem existir para o agente. Entradas vazias, absolutas ou com `..` são recusadas na carga.
**Rule:** Toda pasta que recebe documentação indexável DEVE ser declarada em `docs_folders` de `ai.project.json` antes de receber conteúdo, como caminho relativo ao root, sem `..` e sem caminho absoluto. A primeira entrada é a pasta de docs de projeto (`./docs`), destino de `scribe` e `archivist`.
**Example (Do):** `"docs_folders": ["./docs", "./specs"]`, e só então escrever em `specs/`.
**Example (Don't):** criar `specs/` e escrever documentos sem tocar em `ai.project.json` — a pasta fica invisível para `docs query`, `docs map` e `docs status`.
