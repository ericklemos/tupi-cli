# 🤨 Skeptic: o `ExecTool` vai devolver verde velho — o único falsificador do projeto é cacheável

**Data:** 2026-11-11 · **Alvo:** `crates/tupi-tools/src/tools/exec/` · **Veredito:** não pode
ser considerado gate de verificação enquanto a chave de cache não cobrir o conteúdo
real do workspace.

## 💥 The Catastrophe

`exec` não é uma tool qualquer: é o **único falsificador** do projeto (AGENTS.md: "nenhuma
afirmação entra no contexto sem um falsificador"). Um resultado falso-verde aqui não
degrada uma resposta — ele **falsifica a evidência** de todas as afirmações seguintes.

A chave do cache é:

```rust
let key = fingerprint(&root, MAX_FINGERPRINT_FILES).map(|fp| format!("{}|{fp}", args.join(" ")));
```

(`crates/tupi-tools/src/tools/exec/mod.rs:162-203`)

E `fingerprint` (`crates/tupi-tools/src/tools/exec/render.rs:32-75`) só enxerga:

```rust
builder.standard_filters(true).hidden(true).require_git(false)
       .filter_entry(|entry| entry.file_name() != "target");
...
if !matches!(extension, "rs" | "toml" | "lock") { continue; }
```

ou seja: **extensões `rs|toml|lock` apenas, sem dotfiles, sem nada que o `.gitignore`
ignore**, e a identidade de cada arquivo é `(path, len, mtime_nanos)` — **nunca o conteúdo**.

## 🕵️ The Blindspot

Cinco classes de entrada real do build são estruturalmente invisíveis para a chave:

1. **`.cargo/config.toml`** — dotfile/dot-dir. É onde se define `rustflags`, `target-dir`,
   linker, `[env]`, `[alias]`. Alterar isso muda *se o build passa* — e não muda a chave.
   O mesmo vale para `.config/`, `.sqlx/`, `.env`.
2. **Qualquer fixture fora de `rs/toml/lock`** — `tests/fixtures/*.json`, migrações `.sql`,
   `.proto`, `.yaml`, `.html`, `.txt`. O teste passa a falhar; o cache continua dizendo `ok`.
3. **Tudo que o `.gitignore` esconde** — `standard_filters(true)` respeita gitignore/
   git-exclude/global/`.ignore`. Neste repositório, `.env`, `.env.*`, `.brain`,
   `.agents/tupi/*`, `docs/plans` já estão fora (`.gitignore:1-13`). Qualquer crate cujo
   teste leia um arquivo ignorado pelo git fica com o gate cego.
4. **`len` + `mtime` em vez de hash de conteúdo.** Duas escritas de mesmo tamanho dentro da
   granularidade do mtime produzem chave idêntica — e a granularidade não é nanossegundo em
   NFS/SMB/FAT (1–2s). Também não há defesa contra mtime preservado (`rsync -t`, `cp -p`,
   `tar x`, "salvar com timestamp original"). Cargo e rustc usam hash justamente porque
   `(len, mtime)` mente.
5. **A resposta mente sobre o tempo.** `Cached` clona o `Output` inteiro, incluindo
   `duration`; em `sections_for` (`crates/tupi-tools/src/tools/exec/diagnostics.rs:65-...`)
   a linha de status imprime `raw.duration`, então um hit velho mostra
   `status: ok · 3.4s · em cache` com a duração da execução original. O modelo lê "acabei
   de compilar com sucesso em 3.4s".

**A sequência exata do desastre:**

1. Um crate lê `tests/fixtures/case.json` (ou `.cargo/config.toml` define um `--cfg`).
2. O agente roda `exec kind=test` → `ok`, `Cached { key }` gravado.
3. O agente (via `write`/`edit`/`patch`) ou o usuário altera apenas o `case.json`.
4. O agente roda `exec kind=test` → `fingerprint` idêntico → **hit** → `status: ok`.
5. O agente afirma "os testes passam", o PR é aberto, o pipeline quebra depois do review.

E a assimetria é perversa: o caminho que protege (miss) é o caminho comum (todo
`edit` toca `.rs` e invalida a chave). O buraco só aparece no caso que ninguém testa.

## 💸 The Cost

**Custo de confiança:** um falso-verde é pior que nenhum gate. Hoje o projeto trata `ok`
como prova; um `ok` que pode estar velho rebaixa *toda* a cadeia de evidência
(`ledger`, `assert`, `review`) para palpite.

**Custo de tokens:** o cache economiza **uma** execução de cargo por chave idêntica — e a
economia é *latência*, não contexto: o resumo com `ok` já está no histórico, e o pruner vai
substituí-lo por um ponteiro de qualquer jeito. Ele não evita um único token novo no
próximo request. Em troca, **toda** chamada de `exec` paga um `ignore::WalkBuilder`
completo (`standard_filters` + gitignore matching + metadata de cada `rs/toml/lock` +
`sort` de `Vec<(PathBuf,u64,u64)>`), síncrono, dentro do `spawn_blocking`. Em monorepo de
20k arquivos isso é crawl de metadata por chamada para economizar um cargo run que o
próprio cargo já deduplica com hashes melhores.

**Custo de manutenção:** a whitelist de inputs é uma promessa implícita que **nenhum teste
guarda**. O único teste que menciona `fingerprint` é
`fingerprint_refuses_to_cache_when_file_count_exceeds_limit` (`crates/tupi-tools/src/tools/exec/tests.rs:122-129`),
que só cobre o teto de 5.000 arquivos. Cada nova extensão de fixture alarga o buraco em
silêncio e sem alarme.

**Custo de operação:** a única saída documentada é `refresh: true` (spec, `mod.rs:108-160`),
mas o modelo não tem como saber *quando* a chave mentiu — o fingerprint não é exposto.
Como não pode auditar, a única política segura é `refresh: true` em toda chamada, o que
zera o valor do cache e devolve o custo do walk completo. O cache, então, ou é inútil ou
é perigoso.

## 🛡️ The Ultimatum

Antes de qualquer ship, um destes (o 1 é o mínimo):

1. **Chave por conteúdo.** `blake3` do conteúdo de cada arquivo de entrada, ou — mais
   barato e mais honesto — `git rev-parse HEAD` + hash de `git diff` + lista de untracked
   (`git ls-files -o --exclude-standard`). Enquanto a chave não representar bytes, ela não
   representa build.
2. **Ou: cache desligado por padrão em `test`/`check`/`clippy`.** Cache só quando o
   chamador passar `cached: true` explicitamente. O gate de verificação não se otimiza.
3. **Declarar a cobertura no resultado.** Num hit, imprimir o que a chave cobre e *quando*
   foi a execução original (`em cache desde 14:02`, não `3.4s`). Sem isso o modelo não tem
   como desconfiar.
4. **Teste que falha se o buraco voltar.** Um teste que escreve `fixture.json`, roda a
   chave, altera só o `.json` e exige **miss**; outro que altera `.cargo/config.toml` e
   exige **miss**. Não-negociável.

## Anexo A — o que foi afirmado e o veredito de cada item

| # | Afirmação | Veredito |
|---|---|---|
| 1 | `exec` valida cache por `args + fingerprint(root)` | **Provada** — `exec/mod.rs:162-203` |
| 2 | Fingerprint só considera `rs/toml/lock` | **Provada** — `exec/render.rs:32-75` |
| 3 | Fingerprint ignora hidden + gitignored | **Provada por código** (`standard_filters(true).hidden(true)`); semântica da crate `ignore` não refutada nesta sessão — falsificador: docs da crate |
| 4 | Fingerprint usa `(path,len,mtime)`, não conteúdo | **Provada** — `exec/render.rs:32-75` |
| 5 | Hit imprime `ok · em cache` com a duração original | **Provada** — `exec/mod.rs:84-87` + `diagnostics.rs:65-` |
| 6 | Nenhum teste cobre invalidação por fixture/hidden | **Provada (negativa)** — busca lexical não retornou nenhum; único teste de `fingerprint` é o do limite (`tests.rs:122-129`) |
| 7 | `refresh: true` é a única saída | **Provada** — `mod.rs:162-203` e spec `mod.rs:108-160` |
| 8 | Memória do agente cita `crates/tupi-app/src/context.rs` | **Refutada** — `read` → "arquivo não encontrado"; o código real é `crates/tupi-domain/src/context.rs` |
| 9 | Risco #1 da memória ("cliff do target") resolvido | **Provada só a matemática** — `plan_context` (`tupi-app/src/app.rs:131-157`) escala `target` por `message_budget/context_budget`; a tese de "não há mais thrashing" depende de testes (`app.rs:2110-2162`) que **não** foram executados nesta sessão |
| 10 | `shrink_old_results` instrui `repeat="allow"` incondicionalmente e não guarda nome da tool/argumentos | **Provada** — `tupi-domain/src/context.rs:284-338`; já constava da memória como risco em aberto, e continua assim |

## Anexo B — riscos residuais (não são o alvo desta sessão, mas continuam sem falsificador)

- O ponteiro do pruner pede `repeat="allow"` a **qualquer** tool e não registra qual tool
  nem com quais argumentos. Para `patch`, `write_many`, `move`/`relocate` e `exec`, o convite
  a reexecutar é convite a **repetir um efeito colateral não idempotente** (re-aplicar patch,
  re-mover diretório). Nenhum teste cobre o par "resultado podado de tool destrutiva".
- `ResponseStore` com TTL 600s / 8MB: todo ponteiro `h_xxxxxxxx` entregue ao modelo vira
  promessa que expira. `plan` + `store` juntos produzem um histórico que aponta para handles
  mortos depois de uma pausa de almoço. Continua em "assumptions under watch", não em teste.
- `MAX_FINGERPRINT_FILES` retorna `None` (cache desligado) acima do teto — correto, mas é o
  *único* caminho em que o cache admite não saber. A assimetria é o problema: falha fechada
  por contagem de arquivos, falha aberta por tipo de arquivo.
