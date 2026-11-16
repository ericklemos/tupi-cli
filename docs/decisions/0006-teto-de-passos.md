# ADR 0006 — teto de 30 passos, e dois tetos com dois números

## Contexto

O teto de passos é multiplicador direto do pior caso de um turno: cada passo é
uma requisição com histórico + schema de tool, e o custo dominante é a ida e
volta ao provedor, não a tool local. `configs/tools.ron` trazia
`max_tool_steps: 200`, 6,7× o default de código
(`DEFAULT_MAX_TOOL_STEPS = 30`, `crates/tupi/src/tool_config.rs`), enquanto a
telemetria mostrava turno interativo de 12 passos a ~5,4 s por passo, 48% no
provedor e 46% nas tools (`tupi telemetry`,
[crates/tupi/tests/turn_baseline.rs], [latência do turno](../perf/turn-latency.md)).

O freio só avisa a partir de 60% dos passos e a última chamada, sem tools, só sai
no último passo; com teto de 200 o aviso aparecia no passo ~120 — tarde demais
para servir de aviso. Além disso o mesmo número governava o turno interativo e o
subagente do executor SDD, que tem fôlego diferente por natureza (um subtarefa
curta, um brief e uma saída).

## Decisão

1. `configs/tools.ron` usa **`max_tool_steps: 30`**, o default de código: 2,5× de
   folga sobre os 12 passos medidos, aviso em 18 (60%) e síntese forçada sem tools
   no passo 30. Subir de novo exige **pior caso medido**, não margem imaginada.
2. **Dois tetos, dois números**: o turno principal lê `max_tool_steps` de
   `configs/tools.ron` (`App::with_max_tool_steps`); o subagente lê
   `agent_max_steps` do próprio request (`SddExecRequest::agent_max_steps`).
   Nenhum dos dois deriva do outro, e a falha do subagente nomeia o teto do seu
   request. `the_agent_ceiling_is_per_request_not_the_turn_constant`
   (`crates/tupi-app/src/sdd_run.rs`) prova a independência com 2 e com 5.

O número está em `configs/` porque é limite que o código aplica — é o que faz a
regra valer ([controle por tool](../rules/model-control.md)). Baixar o teto não
cria outro mecanismo de saída: a síntese forçada no último passo continua sendo
a porta.

## Alternativas rejeitadas

- **Manter 200 e confiar em "seja econômico"**: instrução sem limite não é regra;
  o pior caso continuaria crescendo linearmente com o teto.
- **Um teto só, compartilhado com o SDD**: obriga o turno a herdar o fôlego do
  subagente (ou o contrário) e faz o teste do subagente depender da constante do
  turno.
- **Teto derivado do número medido (12)**: sem folga nenhuma; o primeiro turno com
  uma leitura a mais falharia por desenho.

## Consequências

- O pior caso de um turno interativo cai de 200 para 30 passos — 6,7× menos
  round trips possíveis, com a mesma síntese forçada no fim.
- Turnos legítimos longos passam a falhar mais cedo e **nomeando o teto**; é o
  comportamento desejado (falha explícita em vez de turno interminável).
- O comentário de `configs/tools.ron` guarda de onde veio o número; a validação do
  arquivo (`the_checked_in_file_is_valid`, `crates/tupi/src/tool_config.rs`) segue
  verde.
- **Revisar quando** a telemetria mostrar turnos batendo no teto de forma
  recorrente (`steps` ≈ 30 com `ok: false`): aí o número medido mudou, não a
  opinião.
