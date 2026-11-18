<!-- Gerado automaticamente pelo tupi (somente leitura; fonte de verdade no banco local). -->

# Economia de tokens denominada em passos: piso fixo nomeado, dedup e escala estável (`economia-tokens-passo`)

**Status:** completed

## Objetivo

Reduzir o custo real por turno sem trocar tokens por passos. Escopo: (a) medir e tornar explícito o piso fixo de cada passo — schema de tools, prefixo (sistema + workflows + AGENTS.md), bloco de memória/manifesto e `recent_results`; (b) corrigir a nomenclatura e o contrato de `plan_context`; (c) implementar a única economia com risco zero de reabertura — deduplicação por `(path, blake3)` de resultados de tool já presentes no histórico; (d) impedir que a correção de escala oscile dentro do turno e invalide o prefixo cacheado. Fora de escopo: cortar `expose_all` (ADR 0005), bitmask em `KEPT_ARG_KEYS`.

## Restrições

- Toda proposta de poda/corte declara tokens_poupados_por_passo, p(recall) MEDIDO no campo `re-read` de `tupi telemetry` e o break-even `tokens(u) >= p_recall * prompt_tokens_estimado` (docs/perf/turn-latency.md:1-12).
- Não cortar `expose_all` nesta iniciativa: 10.613 tokens/passo de schema são prefixo cacheado e um único passo extra custa ~64.000 tokens de input (docs/decisions/0005-expose-all.md:32-50).
- Nenhum `unsafe`; nenhuma primitiva de concorrência nova; nenhuma mudança de runtime (regra do solver).
- Correção antes de complexidade: dedup por hash antes de qualquer reorganização de estrutura de dados.
- Não introduzir bitmask em `KEPT_ARG_KEYS` (crates/tupi-domain/src/context.rs:155-157): 11 chaves em comparação linear não pagam o risco.
- Os números vivos ficam em `docs/perf/turn-latency.md` e o teste `crates/tupi-tools/tests/tool_schema_tokens.rs:36-45` trava a deriva do schema no mesmo commit.

## Nuances

- `plan_context` (`crates/tupi-app/src/app.rs:142-168`) declara o parâmetro como `tool_tokens`, mas recebe `fixed_tokens` = schema + memória + manifesto + prefixo + aviso (`crates/tupi-app/src/app.rs:1160-1172`). O nome mente sobre ~2/3 do valor: a próxima refatoração derruba a subtração do prefixo em silêncio.
- Piso fixo atual ≈ 31k de 64k de teto: schema 10.613 (`crates/tupi-tools/tests/tool_schema_tokens.rs:41-45`), `AGENTS.md` até 32 KiB (`crates/tupi/src/agents_config.rs:3`) reenviado como mensagem System em todo passo (`crates/tupi-app/src/app.rs:1181-1182`), `recent_results: 6` (`configs/context.ron:11`) reservando ~metade do alvo de 32.000.
- Custo de descartar não é simétrico ao de manter: `shrink_old_results` (`crates/tupi-domain/src/context.rs:368-422`) troca resultado por ponteiro, e o dado medido é 2 leituras por resultado reaberto sem manifesto, 1 com manifesto (docs/perf/turn-latency.md, T6.2).
- `message_target` é escalado proporcionalmente ao teto (`crates/tupi-app/src/app.rs:154-160`) e o alvo é derivado de `message_budget` — com `fixed_tokens` grande, o runway de amortização colapsa e a poda passa a agir em quase todo passo, invalidando o prefixo cacheado (docs/decisions/0005-expose-all.md:24-30).
- A escala de correção usa média simétrica `(current + observed) / 2` com clamp 0,5–4,0 (`crates/tupi-app/src/app.rs:117-134`): converge devagar para cima e rápido para baixo, o que encolhe o orçamento na mesma sessão e realimenta a poda.
- Defaults divergem do arquivo: `ContextConfig::default` = 32.000/12.800/4 (`crates/tupi/src/config.rs:181-189`), `DEFAULT_CONTEXT_TOKENS` = 24.000 (`crates/tupi-domain/src/context.rs:21`), `RECENT_RESULTS_KEPT` = 3 (`crates/tupi-domain/src/context.rs:24`) e o arquivo do repo manda 64.000/32.000/6 (`configs/context.ron:5-11`). Qualquer número novo entra no arquivo e no default juntos.
- Reabertura é recuperável só pelo `ResponseStore` (TTL 600s, 8MB), já registrado como premissa sob vigilância em `.agents/agents/skeptic/memory.md`: após 10 min parado, o handle `h_xxxxxxxx` morre e o resultado podado vira re-execução da tool.

## Tasks

- [x] **T1** Medir o piso e a reabertura antes de tocar em qualquer orçamento
  - Produzir os dois números que faltam para denominar economia em passos: custo fixo por passo e taxa de reabertura. Rodar `cargo test -p tupi-tools --test tool_schema_tokens -- --nocapture` (piso do schema: 4.468 Core / 10.613 All, `crates/tupi-tools/tests/tool_schema_tokens.rs:36-45`) e `tupi telemetry` sobre o turno de referência, lendo `re-read` (`crates/tupi/src/telemetry_report.rs:142-219`), `prompt_tokens` vs `estimate_scaled` e o top-8 de tools. Escrever a tabela resultante em `docs/perf/turn-latency.md` com data, run id e comando reproduzível; nada de número sem comando ao lado. Não alterar código neste passo.
  - Nuance: O piso de prefixo não é medido hoje: `AGENTS.md` entra inteiro até 32 KiB (`crates/tupi/src/agents_config.rs:3`) e é reenviado como mensagem System em todo passo (`crates/tupi-app/src/app.rs:1181-1182`).
  - Nuance: Sem `re-read` medido não existe p(recall): toda decisão de poda posterior fica sem denominador (docs/perf/turn-latency.md:1-12).
  - [x] **T1.1** Travar o piso do schema com o teste existente — `cargo test -p tupi-tools --test tool_schema_tokens` verde e os dois pares (11, 4468) e (29, 10613) confirmados na sessão.
  - [x] **T1.2** Extrair `re-read`, `prompt_tokens` e top tools do turno de referência — Saída de `tupi telemetry` colada em `docs/perf/turn-latency.md` com run id, número de passos e a conta `prompt_tokens − estimate_scaled` por passo.
  - [x] **T1.3** Medir o prefixo fixo real do turno — Tabela com tokens de schema, tokens do prefixo (sistema + workflows + AGENTS.md) e tokens de `recent_results`, somando o piso e comparando com `budget: 64000` de `configs/context.ron:5`.
- [x] **T2** Dar dono ao piso fixo e corrigir o contrato de `plan_context`
  - Renascer `plan_context` com o nome verdadeiro do parâmetro (`fixed_tokens`, não `tool_tokens`) em `crates/tupi-app/src/app.rs:142-168`, documentando no doc-comment que ele carrega schema + memória + manifesto + prefixo + aviso (`crates/tupi-app/src/app.rs:1160-1172`). Expor o piso em `configs/context.ron` como números nomeados independentes (schema, prefix, memory, manifest), cada um com teto próprio validado em `crates/tupi/src/config.rs:195-225`, e fazer `plan_context` usar o valor declarado quando o medido exceder o teto, com aviso único por turno. Alinhar os defaults de código com o arquivo (hoje `32_000/12_800/4` em `crates/tupi/src/config.rs:181-189` contra `64_000/32_000/6` em `configs/context.ron:5-11`).
  - Nuance: O nome mentiroso é o risco de regressão número um: quem 'consertar' a assinatura remove a subtração do prefixo e nenhum teste atual grita.
  - Nuance: `recent_results: 6` reserva 6 resultados inteiros dentro do alvo de 32.000; pelo histograma real são ~11k tokens de piso fixo invisível (`docs/perf/turn-latency.md`, histograma por tool).
  - Nuance: Campo novo em RON exige `deny_unknown_fields`-equivalente respeitado: `an_unknown_field_is_refused` (`crates/tupi/src/config.rs:274-277`) e `the_checked_in_file_is_valid` (`:240-242`) continuam verdes.
  - [x] **T2.1** Renomear `tool_tokens` para `fixed_tokens` e mover o cálculo para um só lugar — `plan_context` recebe `fixed_tokens`; um único ponto do app calcula o valor; os 6 testes de `plan_context` em `crates/tupi-app/src/app.rs:2729-4210` continuam verdes com nomes atualizados.
  - [x] **T2.2** Declarar os quatro tetos do piso em `configs/context.ron` — `crates/tupi/src/config.rs` lê e valida `schema_max`, `prefix_max`, `memory_max`, `manifest_max`; omitir o campo usa o default; valor explícito vence; teste que falha se um campo sumir do arquivo versionado.
  - [x] **T2.3** Avisar uma vez por turno quando o piso medido estoura o declarado — Teste em `crates/tupi-app` injeta `fixed_tokens` acima do teto e verifica exatamente um evento/aviso por turno, com os quatro valores no texto.
- [x] **T3** Dedup por `(path, blake3)` — a única economia com p(recall) zero
  - Quando dois resultados de tool já presentes no histórico carregam o mesmo `(path, blake3)` (o `read` já imprime o blake3 no cabeçalho, `crates/tupi-tools/src/tools/read`), o segundo vira ponteiro interno apontando para o primeiro resultado ainda presente, com o conteúdo recuperável no próprio contexto — sem depender do `ResponseStore` (TTL 600s/8MB). Implementar em `crates/tupi-domain/src/context.rs` como função pura testável (`dedup_identical_results`), chamada por `plan_with` antes de `shrink_old_results` (`crates/tupi-domain/src/context.rs:368-422`), e regerar o número em `docs/perf/turn-latency.md`.
  - Nuance: Dedup só para resultados com identidade verificável (blake3 no corpo); nunca para `exec`, `patch`, `write`, `write_many`, `relocate` — repetição ali é efeito colateral, não economia (risco aberto #8 em `.agents/agents/skeptic/memory.md`).
  - Nuance: É a única poda cujo conteúdo continua no contexto: p(recall) = 0 por construção, então o break-even não precisa de telemetria para valer.
  - Nuance: Não usar `HashMap` desordenado para decidir ordem de poda: a saída precisa ser determinística (teste `dropped_units_leave_a_bounded_deterministic_summary_on_first_surviving_user_message`, `crates/tupi-domain/src/context.rs:763-818`).
  - [x] **T3.1** Função pura de dedup com teste de determinismo — Teste em `crates/tupi-domain/src/context.rs` com dois `read` idênticos verifica o segundo substituído por ponteiro ao primeiro, saída idêntica em duas execuções e `shrunk_results` contando só o que realmente mudou.
  - [x] **T3.2** Ligar o dedup no caminho do plano e no evento de poda — `plan_with` chama o dedup antes do shrink; `Plan` reporta `deduped_results`; teste em `crates/tupi-app/src/app.rs` confirma que histórico deduplicado não emite evento de corte falso (reaproveitar o padrão de `replanning_already_shrunk_history_does_not_report_shrunk_results_again`, `crates/tupi-domain/src/context.rs:731-760`).
  - [x] **T3.3** Medir o ganho real do dedup no turno de referência — `docs/perf/turn-latency.md` ganha uma linha com tokens deduplicados por turno e a contagem de duplicatas por tool, a partir de um run real.
- [x] **T4** Escala de correção sem oscilação dentro do turno
  - Substituir a média simétrica `(current + observed) / 2` (`crates/tupi-app/src/app.rs:125-130`) por uma política assimétrica explícita: dentro do turno a escala só pode cair (corrigir subestimativa é urgente) e volta a subir apenas na virada de turno, com o clamp atual 0,5–4,0 preservado. Registrar em telemetria a escala em vigor por passo junto de `cached_tokens` para tornar visível a invalidação de prefixo causada pela poda.
  - Nuance: A média simétrica converge devagar para cima e rápido para baixo: encolhe o orçamento na mesma sessão e realimenta a poda, invalidando o prefixo cacheado (`docs/decisions/0005-expose-all.md:24-30`).
  - Nuance: Mudança de comportamento é observável: `observed_scale`/`next_scale` são funções puras (`crates/tupi-app/src/app.rs:117-130`) e há teste de `token_scale` no app — a assimetria entra por teste, não por ajuste de constante.
  - Nuance: Não prometer ganho sem medir: a prova é `cached_tokens` por passo antes/depois no mesmo turno de referência.
  - [x] **T4.1** Funções puras assimétricas com teste de monotonicidade — Teste prova que a escala nunca sobe no meio do turno, pode cair a cada passo, sobe na virada e continua dentro de `SCALE_MIN..SCALE_MAX` (`crates/tupi-app/src/app.rs:109-110`).
  - [x] **T4.2** Telemetria de escala e `cached_tokens` por passo — `crates/tupi/src/telemetry_report.rs` imprime escala e `cached_tokens` por passo; `docs/perf/turn-latency.md` compara um turno antes e um depois.
