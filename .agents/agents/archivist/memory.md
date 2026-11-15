# Memória — The Archivist 📚

Índice compacto das regras vigentes. Condensar quando crescer.

## Categorias cobertas (`docs/rules/`)

| Arquivo | Regra |
|---|---|
| `workflows.md` | Workflow nativo só existe em `configs/workflows/*.ron`; o `native_workflows.rs` em `OUT_DIR` é descartável. |
| `knowledge-placement.md` | Estado de agente em `.agents/agents/<slug>/` (+ `.agents/journals/`); regra e doc de projeto só em `docs_folders`. |
| `documentation.md` | Pasta de documentação precisa estar em `docs_folders` (`ai.project.json`), relativa, sem `..`. |

## Sessões

- **Sessão 1:** primeira sessão; `docs/` existia sem `docs/rules/`. Três regras criadas a partir de memória repetida (paths de agente em 4 workflows) e de restrições verificadas no código (`build.rs`, `ProjectConfig`, `DocsIndex`).
