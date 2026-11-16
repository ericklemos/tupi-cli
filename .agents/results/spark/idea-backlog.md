# 💡 Spark — Backlog de ideias (todas, com estado e gatilho)

Registro único de **toda** ideia levantada nas sessões 1–5 do Spark, com o que a
mata, o que a torna urgente e o menor passo para testá-la. É aqui que se olha
antes de abrir uma nova sessão de ideação, para não repetir.

Legenda de estado:
- 🟢 **escolhida** — foi a tese da sessão, tem doc próprio.
- 🟡 **viva** — não escolhida, mas com sinal claro a favor; esperando gatilho.
- 🟠 **condicionada** — só faz sentido se um fato mudar (hardware, cliente, dado).
- 🔴 **morta** — descartada com motivo, para não voltar.

## 🚫 Restrição de projeto (decidida pelo usuário)

**Nada de MCP, nada de servir o espaço a Cursor/Claude, nada de plugin para CLI
de terceiro.** O conhecimento do repo é **fosso, não cano**. Consequência: toda
ideia nova que dependa de expor o espaço (API, protocolo, "integração com X")
nasce 🔴, salvo decisão explícita em contrário. Recibo (Attest) é exceção: é rede,
não conhecimento.

---

## Sessão 1 — Confiança

| # | Ideia | Estado | Doc / âncora |
|---|---|---|---|
| 1.1 | **Tupi Attest** — recibo do código escrito por IA: claim provada com hash/tool vs. palpite | 🟢 | `attestation-receipt.md`; `DeliveryLedger` (`crates/tupi-tools/src/support/ledger.rs:104-267`) |
| 1.2 | **Context Server** — grafo de chamadas como daemon/API ("LSP do contexto") | 🟡 | `support/graph`, `GraphQueries`. ⚠️ revisitar sob a restrição: só serve se o consumidor for o próprio Tupi |
| 1.3 | Marketplace de workflows/personalidades como receita | 🔴 | markdown é distribuível: ótimo para adoção, zero defensabilidade. Vira canal, não produto |
| 1.4 | Memória de projeto por time como produto separado | 🔴 | fundida na Attest: o "git da memória" é o canal de distribuição do recibo |

## Sessão 2 — Validade do conhecimento

| # | Ideia | Estado | Doc / âncora |
|---|---|---|---|
| 2.1 | **Memória que vence** — nota ancorada em `blake3`; código muda → nota expira → CI falha | 🟢 | `expiring-memory.md`; `crates/tupi-app/src/memory.rs` |
| 2.2 | `tupi memory --blame` — qual nota mente e desde quando | 🟡 | derivado direto de 2.1 |
| 2.3 | Gate de review em símbolo ancorado (mudou símbolo com nota → exige revisão) | 🟡 | derivado direto de 2.1 |
| 2.4 | Benchmark público de staleness por ecossistema (`cached_ratio` da telemetria) | 🟡 | `tupi-app/src/telemetry.rs` — conteúdo de mercado, custo quase zero |

## Sessão 3 — Custo

| # | Ideia | Estado | Doc / âncora |
|---|---|---|---|
| 3.1 | **Tupi Delta** — podar contexto por prova (hash) em vez de por idade; ponteiro vira recibo | 🟢 *(estacionada pelo usuário)* | `delta-cost.md`; `shrink_old_results` (`crates/tupi-domain/src/context.rs:284-338`) |
| 3.2 | `cached_ratio` no status bar + prefixo byte-idêntico | 🟡 | degrau quase grátis de 3.1 |
| 3.3 | Lookup que o grafo responde sem tocar no provider | 🟡 | degrau quase grátis de 3.1 |
| 3.4 | `detail=signatures` como default em busca/mapa | 🟡 | degrau quase grátis de 3.1 |

## Sessão 4 — Inteligência

| # | Ideia | Estado | Doc / âncora |
|---|---|---|---|
| 4.1 | **A Escada de Inteligência** — modelo caro vira *tool* (`escalate` + `judge`), comprado por veredito | 🟢 | `intelligence-ladder.md`; `ProviderRegistry` (`crates/tupi-kernel/src/registry.rs:11-32`) |
| 4.2 | `panel` — discordância entre modelos como detector de risco grátis | 🟡 | lateral de 4.1 |
| 4.3 | Camadas `driver/think/review` em `ai.project.json` | 🟡 | hoje `ModelConfig` só tem `context_window` (`crates/tupi/src/project_config.rs:12-17`) |
| 4.4 | `tupi-infer` como estagiário grátis (`distill`/`rerank`/`classify`) | 🟡→🟠 | virou eixo na sessão 5; ver 5.1 |
| 4.5 | `thinkpad` — raciocínio com hash que sobrevive ao `shrink_old_results` | 🟡 | combina 3.1 + 4.1 |
| 4.6 | Router aprendido por patch aceito + `tupi bench models` publicado | 🟡 | precisa de volume de uso antes |

## Sessão 5 — O espaço vetorial do repo

| # | Ideia | Estado | Doc / âncora |
|---|---|---|---|
| 5.1 | **Tupi Space** — o repo todo embedado offline com o 350M; `search`, ranking do `map`, retrieval de memória e roteamento consultam o córtex local antes de gastar token | 🟢 | `repo-vector-space.md`; `EmbedderSlot` (`crates/tupi-kernel/src/embed.rs`), `GeneratorSlot` (`generate.rs`) |
| 5.2 | **`tupi twins`** — duplicata semântica como produto de review ("o bug que é copy-paste esquecido") | 🟡 | derivado de 5.1 |
| 5.3 | **Drift semântico doc↔código** — hash pega forma, embedding pega sentido | 🟡 | complementa 2.1; o doc mente com a assinatura intacta |
| 5.4 | **Roteador local de workflow/personalidade** — KNN do prompt nos journals ("quem já resolveu algo parecido?") | 🟡 | transforma `.agents/journals/` de diário em base de decisão |
| 5.5 | **Juiz local de primeira linha** — o 1.7B responde primeiro; só escala onde `assert`/`exec`/`impact` discordarem | 🟡 | é o *gate de gratuidade* da Escada (4.1), não a substitui |
| 5.6 | **LoRA do LFM3 no repo** — "o sotaque da casa"; dataset já no git (histórico + reviews + memória) | 🟠 | custa um mês em CPU, uma tarde em 5090 (8.6) |
| 5.7 | ~~`space serve` por MCP — servir o espaço a Cursor/Claude~~ | 🔴 | **decisão do usuário:** não se dá nada a CLI de terceiro. O espaço é fosso, não cano — `repo-vector-space.md` §9 |
| 5.8 | **SKU air-gapped** — binário + GGUF com sha256; banco, governo, defesa, saúde compram por política, não por preço | 🟡 **reforçada** por 5.7 | é a *consequência de negócio* de 5.1, e o oposto de servir terceiros |

## Sessão 5, anexo — o que a RTX 5090 destrava

| # | Ideia | Estado | Gatilho / âncora |
|---|---|---|---|
| 8.1 | **Ligar a GPU de verdade** — feature `cuda` no `tupi-infer` + `n_gpu_layers`; hoje carrega com `LlamaModelParams::default()` (`crates/tupi-infer/src/llama.rs:28`) | 🟠 | pré-requisito de tudo abaixo; duas linhas |
| 8.2 | **Indexação como efeito colateral** — indexa no save (`notify` já é dependência); hash deixa de ser economia e vira invalidação | 🟠 | depende de 8.1 |
| 8.3 | **Lint semântico em tempo real** — pairwise total (2.244² ≈ 2,5M cossenos de 1024 dims ≈ 5 GFLOP, milissegundos) desenha duplicatas no editor | 🟠 **promovível a tese** | a ideia que mais ganha com hardware; em CPU viraria batch noturno e morreria de tédio |
| 8.4 | **Reranker cross-encoder** — top-50 → top-8 por consulta; hoje **não existe** `llama_colbert.rs` na árvore | 🟠 | só é justificável com GPU; em CPU é caro demais para rodar sempre |
| 8.5 | **Speculative decoding** — 350M como draft do 14B (decode é bandwidth-bound; os dois pesos já estão baixados) | 🟠 | depende de 8.1 |
| 8.6 | **LoRA numa tarde** — o gargalo do treino passa a ser o dataset, e o dataset já está no git | 🟠 | ver 5.6 |
| 8.7 | **Degrau do meio local** — 14–32B Q4 cabe em 32 GB; cloud vira topo, não miolo | 🟠 | reescopa a Escada (4.1) |
| 8.8 | **Fallback CPU determinístico para CI** | 🟡 **obrigação, não ideia** | sem isso, os testes reais (`crates/tupi/src/config.rs:119-132`) param de rodar onde não há placa |

---

## Correções de fato registradas (para não repetir erro)

- **`llama_colbert.rs` não existe.** `crates/tupi-infer/src/` tem apenas
  `llama.rs`, `llama_generate.rs`, `backend.rs`, `cache.rs`, `download.rs`.
  Qualquer ideia de reranking começa do zero sobre `LlamaEmbedder`
  (`crates/tupi-infer/src/llama.rs:83-116`).
- **A GPU não está ligada.** `llama-cpp-2` entra sem feature de aceleração
  (`crates/tupi-infer/Cargo.toml`) e não há `n_gpu_layers` no workspace. Uma RTX
  5090 hoje roda o Tupi inteiro em CPU.
- **O único consumidor dos dois modelos é a tool `docs`**
  (`crates/tupi-tools/src/tools/docs/mod.rs:36-65`), escopada em `docs_folders` —
  que aliás vem **vazio por padrão** (`crates/tupi/src/project_config.rs:281-288`).

## Regras de higiene deste backlog

1. Ideia que não tem **nano-teste de uma tarde** não entra como 🟡 — entra como
   🟠 com o gatilho explícito.
2. Ideia 🔴 só sai daqui se aparecer fato novo; o motivo fica escrito.
3. Quando uma 🟡 for implementada ou refutada, mover a linha para o fim da seção
   com `— resolvida` / `— refutada`, em vez de apagar.
4. Qualquer ideia que dependa de expor o espaço a terceiros nasce 🔴 (§ restrição).

## Sessão 6 — Decisão

| # | Ideia | Estado | Doc / âncora |
|---|---|---|---|
| 6.1 | **Tupi Veredito** — camada de decisão local (`noul`/`choice`/`score` com confiança); o provider caro só responde o que o local errou | 🟢 | `local-verdict-engine.md`; `crates/tupi-kernel/src/generate.rs:23` |
| 6.2 | **Arquivo multimodal** — d1 lê imagem/vídeo e d1-Omni lê áudio, no lugar de `CommandOcr`/`Transcriber` (hoje *shell-out*) | 🟡 | `archive/derive.rs:69-94` |
| 6.3 | **Log de decisão auditável air-gapped** — "decidiu X, offline, confiança 0.93, hash do modelo Y" como artefato de compliance | 🟡 | reforça 5.8; deriva de 6.1 |
| 6.4 | **Benchmark calibrado de decisões por dólar** — público, porque o gabarito já existe (`VERDICTS`) | 🟡 | `archive/store.rs`; complementa 2.4 |
| 6.5 | **d1 como guardrail do loop de tools** — `noul` antes de efeito colateral | 🟡 | deriva de 6.1 |
| 6.6 | **Spike do `/v1/systemone` em `llama-cpp-2`** — pré-requisito; se falhar, `Decider` sobre o `Generator` atual ainda vale | 🟠 | exige decisão sobre `llama-server` local |

### Correções de fato (sessão 6)

- **"O único consumidor dos dois modelos é a tool `docs`" está obsoleto.**
  Consomem também `space/build.rs`, `tools/search/mod.rs:66` (embedder) e
  `archive`/`investigate` (generator).
- **O `GeneratorSlot` é gated por `docs_folders`**
  (`crates/tupi/src/main.rs:330` → `config.rs:326-328`): sem pasta de docs, o
  `archive` e o `investigate` ficam sem gerador — e o `Verdict` não tem campo de
  confiança (`archive/judge.rs:13-17`).
