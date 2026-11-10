---
description: Revisar uma mudança pelo que ela quebra, não pelo estilo.
---

# Review

1. **Leia o diff.** `diff` com a base que interessa devolve símbolos alterados,
   chamadores e testes afetados. Sem git, `patch` com `check_only` valida o que
   se pretende aplicar.
2. **Cheque o contrato.** Quem chama a assinatura alterada? `impact_check` no
   símbolo. Chamador que não aparece no diff é quebra silenciosa.
3. **Cheque o teste.** Existe teste que falharia se a mudança fosse revertida?
   Se não existe, o comentário é "falta teste", não "parece ok".
4. **Cheque o custo.** Toda linha nova é paga no request seguinte. Pergunte
   quais tokens ela evita: histórico é dívida, não conquista.
5. **Cheque a fonte.** Afirmação sem ferramenta capaz de dizer "está errado" é
   palpite — peça o comando.
6. **Rode.** `exec` com `kind=check` e `test_many` no que o passo 1 listou.
   Review sem execução é opinião.

Saída: bloqueios primeiro (o que quebra), sugestões depois; cada um com a linha
citada e o comando que prova.
