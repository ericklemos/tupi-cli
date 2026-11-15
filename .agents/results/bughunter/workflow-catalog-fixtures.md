# Suíte vermelha em `tools::workflow`: fixtures apontavam para workflows que o binário não traz

## Reprodução

`cargo test -p tupi-tools` → 6 falhas, todas em
`crates/tupi-tools/src/tools/workflow/mod.rs`:

- `tests::the_closest_layer_wins`
- `tests::tupi_beats_agents_in_the_same_scope`
- `tests::an_agents_workflow_beats_the_native_one`
- `tests::list_shows_source_and_read_returns_the_body`
- `tests::an_unknown_name_lists_what_exists`
- `tests::the_prompt_section_lists_names_without_bodies`

## Causa

`NATIVE` é gerado em tempo de compilação por `crates/tupi-tools/build.rs`
(`include!(concat!(env!("OUT_DIR"), "/native_workflows.rs"))`) a partir de
`configs/workflows/*.ron`. O catálogo que viaja hoje no binário é exatamente o
conjunto de agentes:

```
alchemist, archivist, artisan, bolt, bughunter, disco, gatekeeper, janitor,
observer, optimizer, palette, scribe, sentinel, skeptic, solver, spark
```

Os seis testes fixavam nomes que não estão nesse catálogo — `feature`,
`fix-bug` e `review`. Consequências observadas:

- `merge` não encontrava nativo para sombrear: `shadowed` saía vazio
  (`an_agents_workflow_beats_the_native_one` recebia `[]` em vez de `[Native]`).
- `prompt_section` e `action=list` não listavam o nome esperado.

O código de `merge`, `catalog_with` e `parse` está correto: com um nome que
existe, a precedência nativo < global(.agents) < global(.tupi) <
local(.agents) < local(.tupi) funciona como os testes descrevem.

## Correção

Só testes, nenhuma asserção enfraquecida — cada fixture passou a usar um nome do
catálogo real:

| teste | antes | depois |
|---|---|---|
| `the_closest_layer_wins` | `review` | `sentinel` |
| `tupi_beats_agents_in_the_same_scope` | `feature` | `skeptic` |
| `an_agents_workflow_beats_the_native_one` | `fix-bug` | `bughunter` |
| `list_shows_source_and_read_returns_the_body` | `` `feature` (nativo) `` | `` `bolt` (nativo) `` |
| `an_unknown_name_lists_what_exists` | `` `feature` `` | `` `spark` `` |
| `the_prompt_section_lists_names_without_bodies` | `feature` | `alchemist` |

Teste novo (fecha a lacuna que permitiu a falha):
`tests::the_shipped_catalog_is_exactly_the_agent_workflows` fixa a lista de
`NATIVE` na ordem de `build.rs`. Um `.ron` removido ou renomeado em
`configs/workflows/` passa a falhar com a lista na mão, em vez de desaparecer em
silêncio do prompt do sistema, do autocomplete da TUI e dos testes de precedência.

## Verificação

```bash
cargo test -p tupi-tools          # 331 passam, 0 falham
cargo test -p tupi-tools -- tools::workflow
```

## Pendência para decisão humana

Se `feature`, `fix-bug` e `review` deveriam continuar a viajar no binário, o
conserto certo é outro: recriar os `.ron` em `configs/workflows/`. O conteúdo
original não existe na árvore (só os 16 agentes estão lá) e não foi
reconstruído aqui — inventar corpo de prompt seria pior que a falha. Nesse caso,
`the_shipped_catalog_is_exactly_the_agent_workflows` reprova com a lista
esperada e aponta o arquivo faltante.
