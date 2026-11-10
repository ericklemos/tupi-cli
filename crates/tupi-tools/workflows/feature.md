---
description: Levar uma feature nova do desenho ao teste que prova o comportamento.
---

# Feature

1. **Enquadre.** Escreva em uma frase o comportamento novo e o comando que
   prova que ele existe. Sem esse comando a feature não tem falsificador.
2. **Descubra.** `map` com `depth=files` no diretório provável e `search` pelo
   nome do símbolo que a feature toca. Leia só o que for tocar: `read` com
   símbolo devolve o corpo, `read` do arquivo devolve o esqueleto.
3. **Ache a costura.** `impact_check` no símbolo cuja assinatura muda e
   `context_graph` para ver quem consome. Cada camada tem seu lugar: tipo puro
   em `tupi-domain`, contrato em `tupi-kernel`, implementação na crate dona.
4. **Escreva o teste primeiro.** Um teste que falha pelo motivo certo vale mais
   que dez que passam. Rode com `test_many` filtrando pelo nome.
5. **Implemente o mínimo.** `edit` troca o corpo de um símbolo; `write` cria
   arquivo. Não reescreva o arquivo inteiro para mudar um trecho.
6. **Verifique.** `exec` com `kind=check`, depois `test_many` nos testes
   afetados e `clippy` no fim. `diff` mostra o que mudou e quem pode quebrar.
7. **Documente o que ficou.** Contrato ou comando novo muda o README na mesma
   mudança; a decisão durável vai para o `ledger`.

Saída: a frase do passo 1 e o teste que a prova.
