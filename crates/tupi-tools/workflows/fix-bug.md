---
description: Sair de um sintoma reproduzível para a causa e o conserto com teste.
---

# Fix bug

1. **Reproduza.** Rode o comando que falha (`exec` com `filter`, ou `test_many`)
   e guarde a mensagem literal. Sintoma sem reprodutor é palpite.
2. **Leia o erro inteiro.** Arquivo, linha, símbolo. `read` com o símbolo do
   erro devolve só o corpo dele, não o arquivo todo.
3. **Suba pelo grafo.** `context_graph` e `trace` mostram quem chama o símbolo
   que falhou; a causa costuma estar no chamador, não no corpo.
4. **Enuncie a hipótese.** "Se X está errado, então mudar Y faz o erro
   desaparecer porque Z." Uma frase, testável.
5. **Falsifique.** Faça a mudança mínima e rode o mesmo comando do passo 1. Se
   o erro mudar de lugar, a hipótese estava incompleta: volte ao passo 3.
6. **Prove com teste.** O caso que falhava vira teste permanente; rode com
   `test_many` no nome do teste novo.
7. **Procure o irmão.** `search` pelo padrão que estava errado: o mesmo defeito
   costuma existir em mais de um lugar.

Saída: causa, conserto e teste, mais a linha do tempo que liga os três.
