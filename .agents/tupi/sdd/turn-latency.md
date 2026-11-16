<!-- Gerado automaticamente pelo tupi (somente leitura; fonte de verdade no banco local). -->

# Menos round trips por turno: medir o split provider×tool, cortar o teto de passos e tirar custo do caminho quente (`turn-latency`)

**Status:** completed

## Objetivo

Reduzir o tempo de parede de um turno interativo, que hoje é dominado pela quantidade de round trips ao provedor (cada passo = uma requisição com histórico + schemas de tool). Escopo: (a) produzir o número que separa tempo de modelo de tempo de tool a partir da telemetria existente; (b) cortar o teto de passos configurado; (c) tirar requisições auxiliares e execuções seriais do caminho quente; (d) reduzir re-leituras causadas pela poda de contexto. Fora de escopo: trocar provedor/modelo, mexer no orçamento de contexto (64k/32k está folgado, configs/context.ron), reescrever a poda amortizada.

Fontes: configs/tools.ron:1-9 (max_tool_steps: 200, expose_all: true) · crates/tupi/src/tool_config.rs:11-48 (DEFAULT_MAX_TOOL_STEPS = 30, ToolConfig::default, exposure) · crates/tupi/src/tool_config.rs:51-53 (tools_path = configs/tools.ron) · crates/tupi-app/src/app.rs:18 (MAX_TOOL_STEPS = 30) · crates/tupi-app/src/sdd_run.rs:71-96 (freio a partir de 60% e última chamada sem tools; "mesmo freio do turno principal") · crates/tupi-app/src/app.rs:142-168 (plan_context: tool_tokens são subtraídos do orçamento de mensagens) · crates/tupi-app/src/app.rs:20-57 (run_tool: spawn_blocking por call) · crates/tupi-app/src/pipeline.rs:9 e 196-232 (único buffer_unordered(MAX_PARALLEL); serial quando a tool muta) · crates/tupi-app/src/telemetry.rs:409-424 (TurnFinished: duration_us, steps, tool_calls, tokens) · crates/tupi-app/src/telemetry.rs:450-480 (ProviderResponse: duration_us, first_token_us, prompt/cached/completion) · crates/tupi-app/src/telemetry.rs:65-92 (Stats/StatsSnapshot contam eventos, não tempo) · crates/tupi-app/src/telemetry.rs:113-151 (ToolFinished com duration_us) · crates/tupi-app/src/memory.rs:246-281 (recall_for_tool: cache por {tool}+arg clipado 400+fingerprint) · crates/tupi-app/src/memory.rs:301-336 e 400-433 (recall → complete: requisição extra ao mesmo provider) · crates/tupi-app/src/memory.rs:23 (JUDGE_TIMEOUT = 30s) · crates/tupi-domain/src/context.rs:24 (RECENT_RESULTS_KEPT = 3) · crates/tupi-domain/src/context.rs:284-338 (shrink_old_results: resultado antigo vira ponteiro) · configs/context.ron:1-12 (budget 64000, target 32000, recent_results 6).

## Restrições

- Sem número, não fica: nenhuma mudança de comportamento (T2+) é justificada antes de T1.1/T1.3 entregarem o split provider×tool de um run real (docs/philosophy.md, P5).
- Instrução ao modelo só acompanhada de tool ou limite que a verifique ou recuse; pedido em prompt não conta como regra (docs/rules/model-control.md).
- Não gastar API no que é mecânico: julgamento de memória é scoring, não raciocínio; se sair do caminho quente pode ir para o modelo local (docs/rules/local-first.md).
- Memória de projeto não é removida, só sai do caminho quente; a supressão do shell (crates/tupi-app/src/memory.rs:236-243) não pode regredir.
- A poda amortizada preserva o prefixo cacheado do provedor (configs/context.ron:1-4); não trocar por corte que invalide prefixo a cada passo.
- Alteração de contrato observável (saída de CLI, relatório, linha da TUI) atualiza docs/ e README no mesmo commit (docs/conventions.md).
- Edição cirúrgica com hash; nunca reescrever arquivo inteiro para mudar um trecho (AGENTS.md §3).

## Nuances

- O teto configurado é 200 passos (configs/tools.ron:7), 6,7× o default de código 30 (crates/tupi/src/tool_config.rs:11); o freio só avisa a partir de 60% dos passos (~120) e a síntese sem tools só no último, então o pior caso cresce linearmente com o teto.
- expose_all: true (configs/tools.ron:8) coloca o schema de todas as tools em toda requisição; esses tokens são contados como tool_tokens e subtraídos do orçamento de mensagens em plan_context (crates/tupi-app/src/app.rs:142-168), então schema grande encurta o histórico útil antes do previsto.
- Cada tool call paga uma requisição extra de julgamento de memória: recall_for_tool → recall → complete usa o mesmo provider/model (crates/tupi-app/src/memory.rs:246-336, 400-433) com timeout de 30s (memory.rs:23). O cache só acerta com argumentos idênticos nos primeiros 400 chars, logo ler arquivos diferentes = misses distintos = uma requisição extra por leitura.
- begin_turn também roda recall (crates/tupi-app/src/memory.rs:183): o turno pode pagar dois julgamentos (abertura + por tool) do mesmo tema.
- O único paralelismo do caminho de tools é o de pipeline, com buffer_unordered(MAX_PARALLEL) e serialização apenas quando tool.mutates() (crates/tupi-app/src/pipeline.rs:9, 196-232); no turno principal cada call é awaited isoladamente, então N leituras na mesma resposta custam N execuções seriais.
- A poda troca resultado antigo de tool por ponteiro/resumo fora da janela de recent_results (crates/tupi-domain/src/context.rs:24, 284-338); o modelo relê para recuperar detalhe, e uma releitura de algo já entregue na sessão devolve ponteiro para peek — ou seja, gasta um passo para descobrir que não precisava dele.
- A telemetria já tem o dado bruto para o split: ProviderResponse.duration_us/first_token_us (telemetry.rs:450-480), ToolFinished.duration_us (telemetry.rs:113-151) e TurnFinished.steps/tool_calls/duration_us (telemetry.rs:409-424). O que falta é agregação por turno; StatsSnapshot (telemetry.rs:80-92) só conta eventos.
- Tool exec roda em spawn_blocking com cancel cooperativo (crates/tupi-app/src/app.rs:20-57); o custo dominante por passo é a ida e volta ao provedor, não a tool local — por isso cortar passos vence micro-otimizar tool.
- Um passo tem no máximo os tokens de max_tokens de saída; resposta cortada em length descarta as calls e gasta o passo (crates/tupi-app/src/app.rs:4120-4154 mostra o contrato: última chamada sem tools).

## Tasks

- [x] **T1** Medir o split provider×tool por turno a partir da telemetria existente
  - O dado bruto já existe: ProviderResponse.duration_us/first_token_us (crates/tupi-app/src/telemetry.rs:450-480), ToolFinished.duration_us (telemetry.rs:113-151), TurnFinished.duration_us/steps/tool_calls (telemetry.rs:409-424). O que falta é agregação por turno: Stats/StatsSnapshot (telemetry.rs:65-92) só contam eventos.
  - Nuance: Agregar no ponto de record do TelemetryEvent::absorb (crates/tupi-app/src/telemetry.rs:247-265), sem tocar nos emissores.
  - Nuance: A agregação é aritmética em cima do evento já emitido, não pode bloquear o turno.
  - Nuance: StatsSnapshot é struct pública consumida por TUI/web: só adicionar campos.
  - [x] **T1.1** Agregar tempos em Stats/StatsSnapshot — Stats ganha provider_us, first_token_us acumulado, tool_us e steps; StatsSnapshot expõe os mesmos campos; teste unitário em telemetry.rs alimenta múltiplos eventos e confere a soma.
  - [x] **T1.2** Relatório legível por turno — Existe saída legível (subcomando de manutenção despachado no main antes de env/provider, conforme docs/conventions.md, ou o dashboard de crates/tupi-web) que imprime por turno steps, Σprovider_us, Σtool_us e prompt/cached/completion.
  - [x] **T1.3** Baseline gravado em docs/perf — docs/perf/turn-latency.md registra o run de baseline com provedor/modelo, configs/tools.ron, configs/context.ron e o split medido, no formato de docs/perf/build-baseline.md.
- [x] **T2** Cortar o teto de passos do turno interativo
  - configs/tools.ron:7 define max_tool_steps: 200 contra o default de código 30 (crates/tupi/src/tool_config.rs:11). O freio só avisa em 60% dos passos e a síntese sem tools só no último (crates/tupi-app/src/sdd_run.rs:71-96), então o teto é multiplicador direto do pior caso.
  - Nuance: O número novo vem do baseline de T1.3, não de palpite (P5 em docs/philosophy.md).
  - Nuance: Se o executor SDD precisar de fôlego diferente do turno interativo, o teto tem de ser dois números; hoje é um só.
  - Nuance: A síntese forçada no último passo continua sendo o mecanismo de saída; não criar outro.
  - [x] **T2.1** Baixar max_tool_steps com o número justificado — configs/tools.ron tem o novo valor, o comentário do arquivo explica de onde veio o número, e the_checked_in_file_is_valid (crates/tupi/src/tool_config.rs:97-101) segue verde.
  - [x] **T2.2** Desacoplar teto do SDD do teto do turno, se o baseline exigir — Teste prova que o teto do turno principal e o do executor SDD são independentes quando configurados com valores distintos.
- [x] **T3** Decidir expose_all com número de tokens de schema por passo
  - expose_all: true (configs/tools.ron:8) envia o schema de todas as tools em toda requisição; esses tokens entram como tool_tokens e encurtam o orçamento de mensagens em plan_context (crates/tupi-app/src/app.rs:142-168). Aplicação em register_exposed_with/Exposure::admits (crates/tupi-tools/src/lib.rs:465-500).
  - Nuance: Medir antes de cortar: comparar estimate_tokens de PromptSent/ProviderResponse (crates/tupi-app/src/telemetry.rs:450-480) com e sem expose_all.
  - Nuance: Cortar exposição exige recusa acionável para tool não exposta, no padrão de docs/features/shell-tool.md.
  - Nuance: Se o schema for irrelevante frente ao histórico, fechar a task sem mudança e registrar o número.
  - [x] **T3.1** Medir tool_tokens por passo — docs/perf/turn-latency.md tem a linha de tokens de schema, com a diferença entre estimate_tokens e prompt_tokens explicada.
  - [x] **T3.2** Aplicar ou descartar o corte com registro — Aplicado: expose por lista no config, teste em crates/tupi-tools/tests/exposure.rs e recusa que ensina a alternativa. Descartado: decisão em docs/decisions/ com o número que a sustenta.
- [x] **T4** Tirar o julgamento de memória do caminho quente
  - Cada tool call dispara recall_for_tool → recall → complete, uma requisição extra ao mesmo provider (crates/tupi-app/src/memory.rs:246-336, 400-433) com timeout de 30s (memory.rs:23); o cache só acerta com argumentos idênticos nos primeiros 400 chars. begin_turn também paga um julgamento (memory.rs:183).
  - Nuance: A supressão do shell (memory.rs:236-243) e o contrato de memória no prefixo do turno, fora do bloco de resultado, precisam sobreviver.
  - Nuance: Scoring de memória é mecânico: pode ir para o modelo local (docs/rules/local-first.md).
  - Nuance: Se o juiz for adiado, o resultado da tool é publicado antes; o turno não espera.
  - [x] **T4.1** Não julgar de novo o que o turno já recebeu — Com bloco já anexado por begin_turn, recall_for_tool não dispara requisição; teste em crates/tupi-app/tests/shell_memory.rs (ou vizinho) prova uma requisição a menos no turno.
  - [x] **T4.2** Adiar ou localizar o juiz com prova de não-bloqueio — Teste com provider que demora mais que o turno mostra que ToolFinished e o passo seguinte não esperam o julgamento.
- [x] **T5** Paralelizar calls read-only dentro do mesmo passo
  - No turno principal cada call de uma mesma resposta é awaited isoladamente; o único paralelismo é o de pipeline, com buffer_unordered(MAX_PARALLEL) e serialização apenas para tool.mutates() (crates/tupi-app/src/pipeline.rs:9, 196-232). run_tool já é spawn_blocking com cancel cooperativo (crates/tupi-app/src/app.rs:20-57).
  - Nuance: Reusar a regra de mutates() em vez de inventar lista nova de tools.
  - Nuance: Ordem da história e dos eventos ToolStarted/ToolFinished é contrato observável (crates/tupi-app/src/app.rs:3409-3433; docs/features/tui-tool-usage.md).
  - Nuance: Mutantes continuam seriais e na ordem declarada.
  - [x] **T5.1** Executar calls não-mutantes em paralelo preservando a ordem da história — Teste com duas calls read-only lentas prova que o tempo total cai para ~o da mais lenta e que história e eventos saem na ordem declarada.
  - [x] **T5.2** Manter mutantes seriais — Teste com duas calls mutantes prova execução sequencial na ordem declarada, mesmo com MAX_PARALLEL > 1.
- [x] **T6** Manifesto do que já veio no turno, para cortar re-leitura às cegas
  - Fora da janela de recent_results o resultado vira ponteiro (crates/tupi-domain/src/context.rs:24, 284-338); o modelo relê, e a releitura de algo já entregue devolve ponteiro para peek — um passo gasto para descobrir que não precisava dele. Proposta: lista compacta path · símbolo · hash · handle de peek no prefixo do turno.
  - Nuance: Entra no prefixo do turno, como a memória, nunca no bloco de resultado da tool (razão registrada em memory.rs:236-243).
  - Nuance: Lista compacta, sem corpos, sem invalidar o prefixo cacheado a cada passo (configs/context.ron:1-4).
  - Nuance: Regra sem verificação é pedido (docs/rules/model-control.md): o manifesto acompanha o mecanismo que já recusa re-leitura.
  - [x] **T6.1** Montar e injetar o manifesto — Teste mostra o manifesto no prefixo do turno com path/símbolo/hash/handle dos resultados anteriores, sem corpos.
  - [x] **T6.2** Provar queda de re-leitura — Na telemetria, o run com manifesto tem menos chamadas de read/peek por turno que o baseline de T1.3, e o número fica em docs/perf/turn-latency.md.
