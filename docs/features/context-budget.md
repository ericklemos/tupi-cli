# Janela de contexto — piso fixo nomeado, dedup e escala estável

O que viaja em **todo** passo de um turno é o piso fixo (schema + prefixo +
memória + manifesto) mais o histórico podável. Este documento é o contrato
durável dessas três coisas: como o piso é nomeado e somado, qual é a única poda
com p(recall) = 0 e como a escala de correção se comporta dentro do turno. Os
números vivos ficam em [latência do turno](../perf/turn-latency.md).

## Piso fixo: cinco partes, um nome, um ponto de soma

`StepFixedTokens` (`crates/tupi-app/src/app.rs:195-206`) nomeia o que sai no
request mesmo com o histórico inteiro dentro do teto — e que, por isso, compete
com o histórico pelo mesmo orçamento:

| campo | origem |
| --- | --- |
| `tool_schema` | payload de tools serializado (`tool_spec_tokens`, `app.rs:309-313`); 0 no passo final, que vai sem tools |
| `prefix` | sistema + workflows + `AGENTS.md` |
| `memory` | bloco de memória do turno (`begin_turn`) |
| `manifest` | manifesto do que já veio antes do turno |
| `warning` | aviso de freio de passos, quando presente |

O piso tem um único ponto de soma, `StepFixedTokens::total` (`app.rs:212-218`),
e um segundo total só para as quatro partes que têm teto declarado,
`declared_total` (`app.rs:222-227`) — o aviso de freio é transitório, não é piso.

O contrato de `plan_context` (`app.rs:281-307`) declara o parâmetro como
`fixed_tokens`, **não** `tool_tokens`: ele subtrai `fixed_tokens` do teto para
derivar `message_budget` e só então escala `message_target` proporcionalmente
(`app.rs:292-299`). O schema é ~1/3 do valor; chamar o total de `tool_tokens` é
o que faria a subtração do prefixo desaparecer na próxima refatoração. Onde a
prosa antiga de [`../perf/turn-latency.md`](../perf/turn-latency.md) diz
`tool_tokens`, leia `fixed_tokens`.

Ordem que importa: primeiro encolhe o teto pelo piso, depois deriva o alvo. Com
piso grande e alvo não escalado, o runway de amortização colapsa e a poda passa
a agir em quase todo passo — que é justamente o que invalida o prefixo cacheado
([ADR 0005](../decisions/0005-expose-all.md)).

## Dedup por `(path, blake3)` — a única poda com p(recall) = 0

`dedup_identical_results` (`crates/tupi-domain/src/context.rs:485-527`) roda
**antes** de `shrink_old_results` no plano (`context.rs:118-121`). Dois
resultados de tool idênticos: o primeiro fica inteiro, o segundo vira um
ponteiro para o `tool_call_id` do primeiro (`DEDUP_PREFIX`, `context.rs:31`).

- **Só identidade verificável.** `verified_identity` (`context.rs:461-474`) lê a
  linha de cabeçalho `### ` com o marcador `· blake3 \`` (`context.rs:35`) — o
  mesmo `(path, blake3)` que o `read` imprime. Sem esse cabeçalho, nada é
  deduplicado: repetição de `exec`, `patch`, `write`, `write_many` e `relocate`
  é efeito colateral, não economia.
- **Corpo idêntico byte a byte**, não só mesma identidade (`context.rs:508-510`);
  e o ponteiro precisa ser menor em tokens que o corpo que substitui
  (`context.rs:519-521`).
- **Determinístico por construção**: varredura linear na ordem do histórico, sem
  `HashMap` (`context.rs:487-524`), e a segunda passada é idempotente.
- **Não é corte.** O corpo continua no contexto, nada é relido, então
  `p(recall) = 0` por construção — o break-even
  (`tokens_poupados >= p_recall × prompt_tokens`) vale para qualquer token
  poupado aqui, sem depender do `ResponseStore` (TTL curto) nem do campo
  `re-read` da telemetria.
- `Plan::trimmed()` inclui o dedup (`context.rs:53-55`) porque o histórico **foi**
  reescrito e precisa ser regravado na sessão; `Plan::cut()` **não** o inclui
  (`context.rs:60-62`), e só corte publica evento — senão a TUI anuncia uma poda
  que não houve (`app.rs:1323-1331`). O resumo do plano diz quantos resultados
  idênticos foram apontados (`context.rs:79-84`).

## Escala: desce no turno, sobe na virada

A correção da estimativa local contra `prompt_tokens` é `scale_ppm`
(`SCALE_ONE` = 1.000.000, clamp 0,5–4,0, `app.rs:138-153`). A política é
assimétrica de propósito:

- `next_scale_in_turn` (`app.rs:162-167`, aplicado em `app.rs:1512`) **só desce**:
  subestimar é urgente, e o próximo passo já planeja com o número medido.
- `next_scale_at_turn_boundary` (`app.rs:175-179`, aplicado em `app.rs:1161-1164`)
  é a única janela de subida, e sobe **meio caminho** de volta a `SCALE_ONE` em
  vez de saltar.

Razão: subir no meio do turno reabre a poda e troca o request a cada passo. A
média simétrica antiga `(current + observed) / 2` convergia devagar para cima e
rápido para baixo — encolhia o orçamento na mesma sessão e realimentava a poda,
invalidando o prefixo cacheado ([ADR 0005](../decisions/0005-expose-all.md)).
Medição antes × depois, colunas `escala` e `cache` por passo em
[`tupi telemetry`](tupi-telemetry.md) e
[`../perf/turn-latency.md`](../perf/turn-latency.md) (T4.2).

## Invariantes

- Todo token que não é histórico podável entra em `StepFixedTokens` e é
  subtraído **antes** de derivar `message_target`; nenhum ponto do app soma piso
  por conta própria.
- `plan_context` recebe `fixed_tokens`; o nome do parâmetro diz o que ele carrega.
- Dedup só com identidade verificável no próprio corpo e corpo idêntico; nunca em
  tool que muta.
- Dedup antes do shrink; dedup sozinho não publica evento de poda.
- Dentro do turno a escala nunca sobe; sobe só na virada, sempre dentro de
  `SCALE_MIN..SCALE_MAX`.

## Provas

- `plan_context_scales_target_with_message_budget_when_fixed_tokens_are_large`
  (`crates/tupi-app/src/app.rs:2943`),
  `fixed_tokens_are_subtracted_from_the_message_budget` (`:4401`) e
  `the_fixed_tokens_total_adds_every_non_prunable_part` (`:4414`).
- Determinismo e idempotência do dedup: `crates/tupi-domain/src/context.rs:896-911`,
  mais `Plan::deduped_results` conferido em `crates/tupi-app/src/app.rs:4476-4483`.
- Monotonicidade da escala dentro do turno e volta na virada:
  `crates/tupi-app/src/app.rs:2853-2919`.
- Ganho medido do dedup sobre telemetria já gravada:
  `crates/tupi-app/tests/dedup_gain.rs`.

## Limitações conhecidas

- **Teto declarado do piso ainda não existe em config.** `FixedFloorWatch`
  compara o piso medido com o declarado e devolve **no máximo um** aviso por
  turno, listando as quatro partes (`app.rs:242-270`), mas `configs/context.ron`
  não tem `schema_max`/`prefix_max`/`memory_max`/`manifest_max`: o laço não
  chama a guarda e ela segue com `#[allow(dead_code)]` (`app.rs:236-240`). Só o
  nome (T2.1) entrou; o teto declarado e o aviso por turno continuam pendentes.
- **Dedup só age quando o histórico cruza o orçamento**: abaixo do teto o plano
  devolve o histórico intacto (`context.rs:114-116`). O ganho publicado em
  `../perf/turn-latency.md` é o **disponível** no histórico, não token não
  cobrado — confira `context_budget` antes de somar dedup a uma economia por turno.
- **Nenhum turno pós-T4.1 foi medido** (exige API): o que está travado por teste
  é a política e o formato das colunas `escala`/`cache` do relatório.
- **Defaults divergem do arquivo versionado**: `DEFAULT_CONTEXT_TOKENS` = 24.000
  (`context.rs:21`) e `RECENT_RESULTS_KEPT` = 3 (`context.rs:24`) contra
  64.000/32.000/6 de `configs/context.ron`. Número novo entra no arquivo e no
  default no mesmo commit; o arquivo é a fonte do turno real.
- **A tabela do piso do turno não foi medida**: não há número registrado para
  prefixo + `recent_results` somados ao schema e comparados ao `budget` do
  arquivo, e `prompt_tokens − estimate_scaled` **não** serve para atribuir schema
  (`../perf/turn-latency.md`, T1.2).
