---
description: Mudar a forma sem mudar o comportamento, com a rede de testes no lugar.
---

# Refactor

1. **Trave o comportamento.** Antes de tocar, rode os testes que cobrem a área
   (`exec` com `filter` ou `test_many`). Verde antes, verde depois.
2. **Meça a bagunça.** `map` no diretório e `context_graph` no símbolo dão o
   tamanho da teia. Refatoração sem lista de chamadores é reescrita.
3. **Mova em passos que compilam.** `move` para arquivo ou diretório; renomear
   símbolo é `edit` com a assinatura nova mais `search` pelos usos. Um passo
   por vez, `exec` com `kind=check` depois de cada um.
4. **Preserve as bordas.** Assinatura pública só muda com `impact_check`
   dizendo quem quebra.
5. **Rode os mesmos testes do passo 1.** Só o diff importa: `diff` mostra o que
   mudou, e o comportamento deve ter ficado exatamente igual.
6. **Clique no fim.** `clippy` no pacote tocado, e `cargo fmt` via `shell`.

Saída: o commit que move sem mudar, e a prova de que nada mudou.
