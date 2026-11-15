# BugHunter — `peek` com orçamento zero trava a paginação em loop

## 💡 O que

`PeekTool::peek` devolvia uma **página vazia** junto com `Próximo offset:` igual
ao offset que o chamador já estava usando quando `budget_tokens == 0`:

```
página: "\n\nRestante: 6 caracteres (~2 tokens). Próximo offset: 0."
```

Quem segue a instrução do envelope (`Próximo offset`) repete a mesma chamada
para sempre, queimando um request inteiro sem nunca receber conteúdo. O schema
da tool declara `budget_tokens` com `minimum: 1` (`peek/mod.rs:85-114`), mas o
runtime nunca impunha esse mínimo: `chars_for_budget(0) == 0`.

## 🎯 Por quê

A invariante implícita da paginação é *todo page não-final avança o offset*. Ela
não estava escrita em lugar nenhum nem coberta por teste:
`pagination_counts_characters_not_bytes_and_walks_to_the_end` e
`paginating_with_offset_covers_the_body_without_gaps_or_overlap` só exercitam
orçamentos sadios (>0), então o caso degenerado passava despercebido. Um
`default_budget_tokens` zerado por configuração produz o mesmo estado.

## 🛠️ Correção

```rust
fn chars_for_budget(budget_tokens: usize) -> usize {
    // Orçamento zero ainda entrega um caractere: uma página vazia que repete o
    // próprio `Próximo offset` trava a paginação em loop no chamador.
    ((budget_tokens as f64 * CHARS_PER_TOKEN) as usize).max(1)
}
```

Mínimo de um caractere, não erro: é a mesma convenção de
`support::budget::select`, onde a seção de maior prioridade entra mesmo quando
sozinha estoura o orçamento. O `min(remaining.chars().count())` que já vinha
depois continua garantindo `"Fim do corpo."` no fim do corpo.

## ✅ Verificação

Teste novo (Red comprovado antes do fix — falhou com `left: "" right: "a"`):

```
cargo test -p tupi-tools --lib -- peek          # 11 passed, 0 failed
cargo clippy -p tupi-tools                      # ok, sem warnings novos
```

`tools/peek/tests.rs`-style, dentro de `mod tests` em
`crates/tupi-tools/src/tools/peek/mod.rs`:

- `a_zero_budget_still_advances_the_offset`

## ⚠️ Achado fora do escopo (pré-existente)

`cargo test -p tupi-tools` ainda reprova **6 testes de `tools::workflow::tests`**
(`the_closest_layer_wins`, `tupi_beats_agents_in_the_same_scope`,
`an_agents_workflow_beats_the_native_one`, `list_shows_source_and_read_returns_the_body`,
`an_unknown_name_lists_what_exists`, `the_prompt_section_lists_names_without_bodies`).
A causa é sistemática, não relacionada a `peek`: a tabela `NATIVE` não contém mais
o workflow `review`, então `shadowed` vem sem `WorkflowSource::Native`
(`left: [GlobalAgents, GlobalTupi, LocalAgents]` vs
`right: [Native, GlobalAgents, GlobalTupi, LocalAgents]`). Ou os testes ficaram
presos a um nome nativo que mudou, ou o catálogo nativo perdeu entradas — precisa
de uma sessão própria, com a decisão de qual dos dois lados está errado.
