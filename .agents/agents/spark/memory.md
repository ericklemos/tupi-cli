# Memória — The Spark 💡

Registro compacto de ideias propostas. Condensar quando crescer.

## Regra de ouro (decisão do usuário, sessão 5)

**Não se entrega nada ao Cursor ou ao Claude.** Nenhum servidor MCP, nenhuma API
do espaço vetorial, nenhum plugin para CLI de terceiro. O conhecimento do repo é
**fosso, não cano**: o Tupi é o único CLI cuja busca entende a intenção porque o
córtex mora dentro do binário. Toda ideia futura que dependa de expor o espaço
entra já como 🔴 no backlog, salvo decisão explícita em contrário. (Obs.: a Attest
continua podendo virar cano — recibo é rede; conhecimento não é.)

## Sessão 1

- **ESCOLHIDA — Tupi Attest (recibo de código escrito por IA).** Claims de PR
  separadas em "provado com evidência" vs "palpite". Reaproveita o
  `DeliveryLedger` como notário (hash do que o agente leu) + `assert` /
  `impact_check` / `exec` como falsificadores. Negócio: open source o agente,
  cobra o selo/on-prem. Nano-teste: `tupi attest --range main..HEAD` em 3 PRs.
  Recepção: não submetida ainda. Doc: `.agents/results/spark/attestation-receipt.md`.
- **Context Server** (backlog, próxima sessão) — grafo de chamadas como
  daemon/API para qualquer agente ("LSP do contexto"). Mesmo negócio, outro
  preço. ⚠️ revisitar sob a regra de ouro: só vale se o consumidor for o próprio
  Tupi.
- **Marketplace de workflows/personalidades** (descartada como receita) —
  distribuível em markdown; ótima para adoção, zero defensabilidade.
- **Memória de projeto por time** (mergeada na Attest) — o "git da memória" é
  o canal de distribuição do recibo, não produto separado.

## Sessão 2

- **ESCOLHIDA — Memória que vence (hash como prazo de validade).** Cada nota de
  memória ancora no `blake3` do símbolo que cita; código muda → nota vence →
  `tupi memory --check` falha o CI. Não é Attest (não julga PR, julga o repo) e
  não é Context Server (não é retrieval, é expiração). Comprador: quem sofre
  ramp-up; sênior curando memória vira ativo licenciável. Nano-teste: ancorar 10
  bullets deste repo e medir quantos já estão falsos (>30% = produto; ~0% = morre
  barato). Doc: `.agents/results/spark/expiring-memory.md`.
- **Bolso (laterais, não produto):** `tupi memory --blame` (qual nota mente,
  desde quando), gate de review em símbolo ancorado, e benchmark público de
  staleness por ecossistema usando `cached_ratio` da telemetria.
- **Correção da sessão 1:** "Memória de projeto por time" foi fundida como
  distribuição do recibo; não cobria expiração. A tese nova é a validade.

## Potenciais inexplorados no código (para futuras sessões)

- `crates/tupi-app/src/memory.rs` — memória com `Evidence`/`MemoryBlock`:
  pode virar ativo exportável entre repos, não só contexto de sessão.
- `crates/tupi-tools/src/support/graph` — grafo em RAM com `GraphQueries`:
  candidato a produto "contexto por subscrição".
- Telemetria (`tupi-app/src/telemetry.rs`) tem `cached_ratio`: base para
  benchmarking público de custo por modelo/repo — conteúdo de mercado.

- **Consumido na sessão 2:** `crates/tupi-app/src/memory.rs` deixa de ser "potencial" — a memória passa a ancorar no hash dos símbolos (campo `anchors`).

## Sessão 3

- **ESCOLHIDA — Tupi Delta (custo ∝ churn, não ∝ tamanho).** Podar contexto por
  **prova** (hash da fonte inalterado) em vez de por idade; `shrink_old_results`
  (`crates/tupi-domain/src/context.rs:284-338`) já colapsa, mas cego e caro de
  confiar. Ponteiro vira recibo (tool + caminho + hash + handle). Nano-teste:
  `tupi cost --session` somando tokens reenviados de bytes já provados; >40% =
  produto. Doc: `.agents/results/spark/delta-cost.md`.
- **Sinergia com sessões 1 e 2:** o recibo da Attest é a chave de cache; a
  memória ancorada da sessão 2 zera o cold start (verificar em vez de reler). Um
  só mecanismo de hash serve confiança e economia — por isso não é incremental.
- **Degraus quase grátis:** `cached_ratio` (`crates/tupi-domain/src/usage.rs:37`)
  no status bar + prefixo byte-idêntico; lookup que o grafo responde sem tocar no
  provider; `detail=signatures` como default em busca/mapa.
- **STATUS: ESTACIONADO pelo usuário para discussão posterior** (nada
  implementado). Retomar pelo doc, seção "Perguntas em aberto" — 4 decisões
  pendentes: escopo de "econômico", ordem de ataque, tolerância a miss e quando
  colapsar sem quebrar o prefix cache.

## Sessão 4

- **ESCOLHIDA — A Escada de Inteligência (QI comprado por veredito).** O modelo
  caro deixa de ser motor e vira **tool**: `escalate` chama outro provider do
  `ProviderRegistry` com prompt só de recibos (tool+caminho+hash);
  `judge` aplica `assert`/`exec`/`impact` em N candidatos e devolve o sobrevivente.
  Consequência: rodar com modelos top vira econômico porque só se escala onde há
  falsificador. Métrica de negócio: **bugs pegos por dólar** (não existe benchmark
  assim). Nano-teste: 20 perguntas verificáveis sobre o repo, barato vs. caro;
  ≥20% de divergência com o caro certo = prêmio. Doc:
  `.agents/results/spark/intelligence-ladder.md`.
- **Degraus laterais (não escolhidos, vivos):** `panel` (discordância entre
  modelos como detector de risco grátis), camadas `driver/think/review` em
  `ai.project.json` (hoje `ModelConfig` só tem `context_window`,
  `crates/tupi/src/project_config.rs:12-17`), `tupi-infer` como estagiário grátis
  (`distill`/`rerank`/`classify`), `thinkpad` (raciocínio com hash que sobrevive
  ao `shrink_old_results`), e router aprendido por patch aceito + `tupi bench
  models` publicado.
- **Sinergia com sessões 1–3:** o recibo da Attest ganha coluna "qual
  inteligência decidiu o quê, a que preço"; sem ledger não há escada, só fé.
  Continua sem substituir Delta (custo intra-sessão) nem memória que vence.
- **Consumido:** `crates/tupi-kernel/src/provider.rs:47-58` (trait minúsculo) e
  `registry.rs:11-32` deixam de ser "infra" e passam a ser o eixo do produto.

## Sessão 5

- **ESCOLHIDA — Tupi Space (o repo como espaço vetorial, gerado offline).** Os
  dois modelos Liquid já baixados e verificados por sha256
  (`crates/tupi-infer/src/download.rs:34-62`) têm **um único consumidor**: a tool
  `docs`, escopada em `docs_folders` — dense+RRF (`docs/search.rs:17-162`) e
  resumo de uma linha (`docs/index.rs:516-591`). Enquanto isso `search` é
  lexical puro (`search/query.rs`, `LexicalRanker`) e a memória é despejada
  inteira. Tese: o espaço vetorial é **do repo**, é cache incremental por hash e
  é grátis — `search`, ranking do `map`, retrieval da memória e roteamento de
  agente consultam o córtex local antes de gastar token. Negócio: descoberta com
  custo zero (barateia a Escada, sessão 4) + SKU air-gapped (banco/governo/
  defesa/saúde, onde SaaS é proibido por política). **Sem MCP:** o espaço não é
  servido a ninguém, é o que faz ninguém querer trocar de ferramenta. Nano-teste:
  `tupi space build` (2.244 símbolos + docs + bullets), 20 perguntas em português
  sem identificador, recall@5 semântico vs. lexical, rede desligada; ≥40% de
  ganho = paga; ≥3 gêmeos semânticos reais = produto de review; empate = morte
  barata em CPU, zero API. Doc: `.agents/results/spark/repo-vector-space.md`.
- **Laterais vivas:** `tupi twins` (duplicata semântica como review), drift
  semântico doc↔código (hash pega forma, embedding pega sentido — complementa a
  memória que vence da sessão 2), roteador local de workflow por KNN nos
  journals, juiz local de primeira linha (gate de gratuidade da Escada) e LoRA do
  LFM3 no repo ("sotaque da casa").
- **MATADA — `space serve` por MCP.** Decisão do usuário: nada para Cursor/
  Claude. Não volta sem pedido explícito; ver "Regra de ouro".
- **Não repete:** Delta (custo intra-sessão), Escada (QI comprado) e Attest
  (julgar PR). Funde-se com as três: o recibo ganha procedência "achado pelo
  córtex local, sem API".
- **Consumido:** `crates/tupi-kernel/src/embed.rs` (`EmbedderSlot`) e
  `generate.rs` (`GeneratorSlot`) deixam de ser "infra da tool docs" e passam a
  ser o eixo do produto local-first.
- **Correção de fato:** a nota de memória sobre `llama_colbert.rs` não
  corresponde à árvore atual — `crates/tupi-infer/` tem só `llama.rs`,
  `llama_generate.rs`, `backend.rs`, `cache.rs`, `download.rs`. Não há reranker
  ColBERT hoje: qualquer ideia de reranking começaria do zero, sobre
  `LlamaEmbedder` (`llama.rs:83-116`).

### Sessão 5, anexo — com RTX 5090

- **Fato primeiro:** o Tupi hoje não usa GPU — `llama-cpp-2` sem feature de
  aceleração (`crates/tupi-infer/Cargo.toml`) e `LlamaModelParams::default()`
  (`crates/tupi-infer/src/llama.rs:28`). Destravar = feature `cuda` +
  `n_gpu_layers`, duas linhas.
- **O que a placa compra (em ordem):** indexar vira efeito colateral (repo todo
  em segundos, hash muda de *economia* para *invalidação*); **lint semântico em
  tempo real** (pairwise 2.244² ≈ 2,5M cossenos ≈ 5 GFLOP ≈ ms — duplicata
  esquecida vira rabisco no editor, categoria nova); degrau do meio local
  (14–32B Q4 cabe); reranker cross-encoder justificável (top-50 → top-8);
  speculative decoding com o 350M de draft; LoRA "sotaque da casa" numa tarde.
- **O que não compra:** 32B local não é frontier, contexto longo estoura 32 GB,
  e CI sem GPU exige fallback CPU determinístico (senão os testes reais de
  `crates/tupi/src/config.rs:119-132` param).
- **Promoção pendente:** se indexar o repo todo levar < 5 s, a tese deixa de ser
  "cache por hash" e passa a ser "lint contínuo" — item 8.3 do backlog.
- **Backlog completo de todas as ideias (sessões 1–5) com estado 🟢/🟡/🟠/🔴,
  gatilho e correções de fato:** `.agents/results/spark/idea-backlog.md`.
