# Prefixo do turno — memória e manifesto, nunca dentro do resultado da tool

Duas coisas que o modelo precisa saber a cada passo entram no **prefixo do
turno**: o bloco de memória do projeto e o manifesto do que o turno já entregou.
Nenhuma das duas entra no corpo do resultado de uma tool.

## Por que fora do resultado

O `shell` mostrou o custo de misturar: com a memória anexada ao resultado, o
modelo lia um fato lembrado como se fosse saída do comando (telemetria do run
`1794840347760-112433`, seq 12, 20, 22 e 24). Daí a regra: memória de projeto e
manifesto são **prefixo**, e o resultado da tool é só o que a tool devolveu. No
`shell` a supressão continua em `crates/tupi-app/src/memory.rs` e não pode
regredir ([tool `shell`](shell-tool.md)).

## Memória: um julgamento por turno (T4.1)

`begin_turn` paga **um** julgamento de memória e anexa o bloco ao prefixo
(`crates/tupi-app/src/memory.rs`). Com o bloco já no prefixo, `recall_for_tool`
não dispara requisição: julgar de novo por tool call compraria uma ida extra ao
provedor para reencontrar o que o modelo já recebeu. A marca `turn_attached` é
zerada no começo de cada turno, então o turno seguinte volta a julgar.

- Contrato: no máximo **um** julgamento por turno quando o bloco foi anexado;
  `recall_for_tool` devolve `None` sem chamar o provedor.
- Prova: `a_turn_that_already_attached_memory_does_not_judge_again_per_tool`
  (`crates/tupi-app/tests/shell_memory.rs`) confere `calls == 1` depois de uma
  tool call.
- O cache por `{tool}+arg` e o timeout de julgamento seguem valendo para os turnos
  em que nada foi anexado.

## Manifesto: o que já veio, sem corpos (T6.1)

Fora da janela de `recent_results` a poda troca o resultado antigo por um
ponteiro (`shrink_old_results`, `crates/tupi-domain/src/context.rs`). Sem aviso,
o modelo relê para recuperar detalhe e descobre que o corpo estava a um `peek` de
distância — um passo gasto para nada. O manifesto fecha esse buraco:

- `turn_manifest` (`crates/tupi-domain/src/context.rs:256-306`) monta uma linha
  por resultado já entregue: `path · símbolo` (`format_call_target`), `blake3` e
  `handle` de `peek`, extraídos do **cabeçalho** do resultado (`header_meta`).
- Cabeçalho: `MANIFEST_PREFIX` (`context.rs:228`) declara a regra — sem corpos,
  `peek` para o trecho omitido, `read` com `repeat="allow"` para o corpo. É
  lembrete, não pedido solto: o `peek` que o manifesto imprime é o mesmo handle
  que a tool aceita ([controle por tool](../rules/model-control.md)).
- Limites: `MANIFEST_MAX_ENTRIES = 24` entradas, últimas primeiro e depois
  invertidas, linha repetida deduplicada, e só os primeiros `MANIFEST_HEAD_CHARS`
  de cada resultado são lidos para achar hash e handle. Nada de corpo.
- Prefixo cacheado: o manifesto é texto curto e estável por passo; não cortar por
  corte que invalide o prefixo a cada passo (a poda amortizada depende disso,
  `configs/context.ron`).

## O que o número diz

O manifesto não é otimização de conforto: o baseline sem manifesto custa **2**
chamadas de leitura por resultado reaberto (`read` devolve ponteiro, `peek`
devolve o corpo); com manifesto, **1**. Medição e forma de comparar em
[latência do turno](../perf/turn-latency.md), campo `re-read` de
[`tupi telemetry`](tupi-telemetry.md).

## Invariantes

- Memória e manifesto no prefixo; resultado de tool só com o resultado.
- Um julgamento de memória por turno, quando o bloco foi anexado.
- Manifesto sem corpos, com teto de entradas e sem duplicata.
- A supressão da memória no `shell` não regride.
- Nenhum dos dois muda a ordem de história nem de eventos `ToolStarted`/
  `ToolFinished`.

## Limitações conhecidas

- O manifesto cobre o turno corrente: sessão resumida recomeça sem ele.
- Só entram resultados cujo cabeçalho traz hash ou handle; o resto fica fora do
  lembrete.
- O adiamento/localização do juiz de memória (T4.2) **não** foi aplicado: hoje o
  julgamento do `begin_turn` ainda acontece no caminho do turno, só deixou de se
  repetir por tool.
