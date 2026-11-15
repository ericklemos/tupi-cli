# Janitor — diário de limpeza

Somente aprendizados críticos. Nada de rotina.

## 2026-02-14 - `impact_check` mente sobre zero chamadores

**Aprendizado:** `impact_check` (e `assert used_outside`) reportaram 0 chamadores para
`MetaFile::from_plan` e `TasksFile::from_plan` (crates/tupi-tools/src/tools/sdd/meta.rs),
inclusive com `include_tests=true`. Falso negativo: `FsSddStore::save`
(crates/tupi-tools/src/tools/sdd/store.rs) chama os dois, e o teste
`meta_projection_keeps_activities_and_tasks_stay_separate` também. Grafo não indexa
chamadas dentro de impls de trait nem de helpers de teste em todos os casos.

**Ação:** nunca deletar por "0 chamadores" do grafo. Confirmar com leitura textual do
arquivo que consome (buscar o nome no `store.rs`, nos `mod tests`) antes de apagar.
Um "0" do grafo é hipótese, não prova.

## 2026-02-14 - Placeholders do SDD ficaram para trás em dois crates

**Aprendizado:** a migração do SDD para `.tupi/sdd` deixou arquivos-âncora de uma linha
("// Removido: o SDD agora vive em .tupi/sdd") em `crates/tupi-store/src/sdd.rs` (com
`mod sdd;` em lib.rs) e em `crates/tupi/src/sdd_store.rs` (sem `mod` nenhum — nunca
compilado). Arquivos sem símbolos não aparecem no `map`, então o órfão de `crates/tupi`
só foi descoberto comparando a lista de arquivos do diretório com os `mod` declarados
em `main.rs`.

**Ação:** depois de qualquer migração, cruzar `map(depth="files")` com as declarações
`mod` do arquivo raiz. Arquivo presente no disco e ausente do mapa é órfão até prova em
contrário.

## 2026-11-15 - `patch` é inutilizável para deletar (último hunk corrompido)

**Aprendizado:** a tool `patch` recusa todo diff cujo último hunk termina em linha de
contexto ou de remoção longa com `error: corrupt patch at line N`, sempre apontando a
última linha do patch. Só passaram hunks minúsculos (5 linhas, uma remoção de
`Cargo.toml`). Testado com 1 hunk de 4 a 18 linhas, 4 hunks, com e sem `
` final:
todas as variantes grandes falharam — enquanto o mesmo conteúdo com header de contagem
"errada" foi aceito na fase de parse (logo o problema é o parser do lado da tool, não o
diff).

**Ação:** planejar a deleção em hunks de ~5 linhas ou menos por arquivo; para remoções
maiores, fatiar em vários patches pequenos. Nunca começar a sessão contando com um
patch grande — e registrar no relatório quando a tool bloquear a limpeza.

## 2026-11-15 - O grafo também não vê chamadas de crate em `mod tests` do próprio arquivo

**Aprendizado:** `assert used_outside` deu NÃO para `with_save_path`
(crates/tupi-providers/src/antigravity.rs:590) e NÃO para `SddSpecSummary`. O segundo é
usado em `crates/tupi-app/tests/sdd_specs.rs:71` (teste de integração de outro crate) —
o grafo não indexa `crates/*/tests/`. Para dependências de Cargo o método confiável é
remover a linha e rodar `cargo check --all-targets`: o E0433 lista exatamente os usos.

**Ação:** para dependências, provar por compilação, não por busca. Para símbolos,
procurar o nome nos diretórios `tests/` de todos os crates antes de apagar.
