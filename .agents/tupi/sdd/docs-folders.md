<!-- Gerado automaticamente pelo tupi (somente leitura; fonte de verdade no banco local). -->

# Campo `docs_folders` no ai.project.json (declarativo) (`docs-folders`)

**Status:** closed

## Objetivo

O projeto passa a saber onde ficam os arquivos de documentação: novo campo opcional `docs_folders` (lista de caminhos relativos ao root) em `<root>/ai.project.json`, lido por `ProjectConfig` em `crates/tupi/src/project_config.rs`, validado na carga e resolvido para caminhos absolutos por um acessor. Nenhum consumidor além do log de inicialização nesta etapa — injeção no prompt, indexação ou tools ficam para depois.

## Restrições

- Escopo travado: só campo + validação + acessor + log de debug. Não injetar seção no prompt de sistema, não indexar arquivos, não alterar `ToolsOptions`.
- Caminhos relativos ao root apenas; absolutos e `..` são rejeitados na carga (viram erro de config, não silêncio).
- Retrocompatível: arquivo sem `docs_folders` continua válido e a chave não aparece no arquivo criado (`skip_serializing_if`).
- Zero token novo no prefixo de sistema nesta etapa.
- `load_or_create` mantém o comportamento atual: arquivo existente nunca é sobrescrito.

## Nuances

- `ProjectConfig` hoje tem `name` + `models` (crates/tupi/src/project_config.rs:21-28); `models` já usa `#[serde(default, skip_serializing_if = "BTreeMap::is_empty")]` como precedente de campo opcional.
- `parse_project` (:65-75) é o único ponto de validação semântica hoje: ele checa `name` não vazio e devolve `ProjectConfigError`; validação de `docs_folders` entra aqui.
- `fallback` (:110-115) constrói `ProjectConfig` literal — precisa incluir o campo novo ou não compila.
- `load_or_create` (:86-108) cai no fallback com warning em qualquer erro, inclusive o erro novo de docs; teste `an_invalid_file_falls_back_without_overwriting` continua valendo.
- `main` é o único chamador (`crates/tupi/src/main.rs:17`); o log existente `tracing::debug!(name, models, path, "configuração do projeto")` é o modelo para o novo log.
- Não há pasta `docs/` neste repositório nem documentação local do `ai.project.json`; nada a atualizar em markdown.
- Ordem da lista preservada e deduplicada na resolução, para log estável entre execuções.

## Tasks

- [ ] **t1** Adicionar `docs_folders: Vec<String>` a `ProjectConfig`
  - Em `crates/tupi/src/project_config.rs:21-28`, adicionar o campo com doc comment, `#[serde(default, skip_serializing_if = "Vec::is_empty")]`. Atualizar `fallback` (:110-115) para `docs_folders: Vec::new()`.
  - [ ] **t1.1** Campo e default no struct — `ProjectConfig` tem `docs_folders`; `cargo check -p tupi` passa sem warning novo.
  - [ ] **t1.2** Arquivo criado sem a chave — Teste em `mod tests` (:139): após `load_or_create` em tempdir vazio, o conteúdo de `ai.project.json` não contém `docs_folders`.
- [ ] **t2** Validar `docs_folders` na carga
  - Nova variante em `ProjectConfigError` (:32-59) recusando entrada não relativa. Em `parse_project` (:65-75), normalizar cada entrada com `trim()`, remover `./` repetido, recusar vazio, `is_absolute`, `Component::Prefix` e `Component::ParentDir`; deduplicar preservando ordem.
  - Nuance: `Path::components` em caminho relativo nunca produz `RootDir`, mas `C:` produz `Prefix` no Windows — checar `is_absolute` + `Prefix` + `ParentDir` cobre os dois SOs.
  - [ ] **t2.1** Variante de erro + validador — Função privada `normalize_docs_folders(path, entries) -> Result<Vec<String>, ProjectConfigError>` usada por `parse_project`; sem `unwrap` em caminho de produção.
  - [ ] **t2.2** Testes de recusa — Testes cobrindo: `/abs/docs` recusado; `../fora` recusado; `"  "` recusado; `["docs","./docs"]` aceito e deduplicado para `["docs"]`.
  - [ ] **t2.3** Fallback não quebra — `load_or_create` com `{"name":"x","docs_folders":["/etc"]}` usa o fallback da pasta e NÃO sobrescreve o arquivo.
- [ ] **t3** Acessor que resolve `docs_folders` para caminhos absolutos
  - `pub fn docs_folders(&self, root: &Path) -> Vec<PathBuf>` devolvendo `root.join(entry)` para entradas que existem e são diretório; inexistentes geram `tracing::warn!` e são omitidas; ordem preservada.
  - [ ] **t3.1** Implementar o acessor — Retorna `Vec<PathBuf>` absoluto para diretórios existentes; lista vazia quando o campo está vazio.
  - [ ] **t3.2** Testes de resolução — Teste com `docs/` criado e `specs/` ausente: resultado contém apenas `docs/`, prefixado pelo root do tempdir.
- [ ] **t4** Expor no log de inicialização
  - Em `crates/tupi/src/main.rs`, após o `tracing::debug!` de "configuração do projeto", logar os diretórios resolvidos a partir da variável `project` já existente. Nenhuma alteração em `compose_startup_prefix`.
  - [ ] **t4.1** Log de debug — Com `TUPI_LOG=debug`, os diretórios resolvidos aparecem no log; `prompt_chars` da telemetria não muda quando `docs_folders` é preenchido.
