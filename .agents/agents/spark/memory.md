# Memória — The Spark 💡

Registro compacto de ideias propostas. Condensar quando crescer.

## Sessão 1

- **ESCOLHIDA — Tupi Attest (recibo de código escrito por IA).** Claims de PR
  separadas em "provado com evidência" vs "palpite". Reaproveita o
  `DeliveryLedger` como notário (hash do que o agente leu) + `assert` /
  `impact_check` / `exec` como falsificadores. Negócio: open source o agente,
  cobra o selo/on-prem. Nano-teste: `tupi attest --range main..HEAD` em 3 PRs.
  Recepção: não submetida ainda. Doc: `.agents/results/spark/attestation-receipt.md`.
- **Context Server** (backlog, próxima sessão) — grafo de chamadas como
  daemon/API para qualquer agente ("LSP do contexto"). Mesmo negócio, outro
  preço.
- **Marketplace de workflows/personalidades** (descartada como receita) —
  distribuível em markdown; ótima para adoção, zero defensabilidade.
- **Memória de projeto por time** (mergeada na Attest) — o "git da memória" é
  o canal de distribuição do recibo, não produto separado.

## Potenciais inexplorados no código (para futuras sessões)

- `crates/tupi-app/src/memory.rs` — memória com `Evidence`/`MemoryBlock`:
  pode virar ativo exportável entre repos, não só contexto de sessão.
- `crates/tupi-tools/src/support/graph` — grafo em RAM com `GraphQueries`:
  candidato a produto "contexto por subscrição".
- Telemetria (`tupi-app/src/telemetry.rs`) tem `cached_ratio`: base para
  benchmarking público de custo por modelo/repo — conteúdo de mercado.
