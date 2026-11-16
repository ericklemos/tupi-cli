# ADR 0001: o cache de `exec` fica, com a chave restrita a insumos de build

## Contexto

O `exec` guarda o último resultado numa `Mutex<Option<Cached>>` em RAM
(`crates/tupi-tools/src/tools/exec/mod.rs:163-205`). Existe um `ExecTool` por
`ToolRegistry` (`crates/tupi-tools/src/lib.rs:293`) e o registry é clonado por
`Arc` (`crates/tupi-app/src/app.rs:1005`): o cache sobrevive entre chamadas do
mesmo processo, então o *miss* medido não é tempo de vida do objeto.

A chave é `args.join(" ") + fingerprint(root, 5_000)`, e o `fingerprint`
(`crates/tupi-tools/src/tools/exec/render.rs:66-127`) percorre **todo** arquivo
sob o root, exceto `target/`, `.git/`, `.brain/` e `.agents/tupi/`, com teto de
5.000 arquivos / 64 MB; estourado o teto ele devolve `None` e o cache desliga em
silêncio (`exec/mod.rs:29`). Do ambiente entram só cinco nomes de variável
(`render.rs:32-53`), e `RUSTUP_TOOLCHAIN` entra pelo *nome*, nunca pela versão
resolvida; `CARGO_HOME`, `CARGO_NET_OFFLINE`, `SQLX_OFFLINE`, `DATABASE_URL`,
`CC`/`CFLAGS` e `PATH` ficam fora.

Dois defeitos hoje se cancelam:

1. A chave inclui conteúdo volátil do próprio agente
   (`.tupi/sessions/<id>/log.jsonl` cresce a cada tool call), então ela muda
   entre duas chamadas idênticas e o cache **nunca acerta**.
2. O miss permanente é o que esconde o verde velho. Corrigir a chave sem fechar
   o ambiente transformaria um desperdício num `ok` cacheado que sobrevive a
   `rustup update`, a `~/.cargo/config.toml` e a `CARGO_NET_OFFLINE=1`.

O custo do miss também é invisível: o resumo reporta `raw.duration`, a duração
do processo cargo; o walk + blake3 acontece antes e não aparece
(`exec/diagnostics.rs:65-93`).

### Medição (T1.1)

Critério do gate, escrito antes de decidir: **o cache fica se, no caso típico
(segunda verificação sem mudança de código), o acerto economizar mais do que o
walk + blake3 custa.**

Harness: `fingerprint_cost`
(`crates/tupi-tools/src/tools/exec/tests.rs:286-324`, `#[ignore]`), que separa as
duas fases — `fingerprint_entries` (metadados) e `fingerprint_digest` (bytes) —
e mede três rodadas em cada cenário. Offline, sem rede, sem toolchain extra.

Host Linux x86_64, 2026-11-16 (tempos por rodada; `target/` aquecido):

| cenário | arquivos | bytes | walk | hash blake3 | total |
|---|---|---|---|---|---|
| repositório real | 437 | 5,9 MB | 1,5–3,4 ms | 3,6–3,8 ms | **5,3–7,1 ms** |
| repo sintético de 500 arquivos de 4 KiB | 500 | 2,0 MB | 0,42–0,53 ms | 1,7–1,9 ms | **2,2–2,5 ms** |

O custo é dominado pelos bytes lidos, não pelo número de inodes: ~1 ms/MB. A
medição de 2026-11-16 no mesmo repo, citada no plano, foi de **803 inodes /
25,2 MB** — extrapolando a mesma taxa, ~20–25 ms de chave, e a conclusão do gate
não muda.

Do outro lado da balança, o que o acerto evita:

- `cargo build -p tupi-tools` aquecido sem mudanças: **0,1 s**
  (`docs/perf/baseline.md`).
- `cargo check -p tupi-tools` aquecido: **0,5 s** (medido por `exec` nesta
  sessão).
- `test`/`clippy` aquecidos ficam na casa dos segundos
  (`cargo test` completo da workspace: 250,6 s).

No caso típico o acerto devolve **0,1–0,5 s por 5,4 ms de chave**, ou seja
18×–90× o custo do walk + blake3.

Achado que decide o *desenho*, não só o gate: duas execuções da sonda, separadas
por segundos e sem nenhuma mudança de código, produziram chaves diferentes
(`c1853b75` → `1dfe661a`). A composição da chave no repo real é `.tupi` com
**65 arquivos / 2,73 MB (46 % dos bytes)**, `crates` com 273 / 2,32 MB, `docs`
com 36 / 0,37 MB, `.agents` com 16 / 0,10 MB, `.idea` com 5 / 0,02 MB. O `.tupi`
é o log da sessão corrente: conteúdo que muda a cada turno e que o cargo não lê.
Qualquer repositório com arquivo volátil análogo (lock de IDE, log) desliga o
cache sem erro nenhum — negação de serviço silenciosa.

## Decisão

**Consertar a chave. O cache fica.** O escopo de T2–T5 permanece "consertar e
documentar", não "apagar e documentar".

1. A chave passa a ser *insumos de build + ambiente que muda o resultado*: o
   conteúdo dos arquivos que o cargo de fato lê e as variáveis/versões que
   alteram a compilação (versão resolvida do toolchain, `CARGO_HOME`,
   `CARGO_NET_OFFLINE`, `SQLX_OFFLINE`, `DATABASE_URL`, `CC`/`CFLAGS`, `PATH`).
2. O verde velho é fechado ou recusado explicitamente: `fingerprint` `None` é
   sempre *miss*, nunca *hit*; ambiente que não pode ser lido com fidelidade
   também vira *miss*. Fail-closed, como já vale em `render.rs:99-103` e
   `render.rs:132-154`.
3. Exclusão de caminho volátil (`.tupi/`, `.idea/`, `.agents/results/`,
   `.agents/journals/`) só entra com justificativa escrita no código e um teste
   que a trave, no padrão de comentário de `render.rs:60-68`. `docs/`,
   `.agents/results/` e `.agents/journals/` só saem da chave se um teste provar
   que nenhum build/teste os lê como fixture.
4. O caminho de acerto ganha teste de ponta a ponta: duas chamadas de `call` com
   o mesmo estado e a segunda reporta `em cache há`. Hoje o único teste
   (`exec/tests.rs:177`) injeta a idade direto em `sections_for` e nada prova o
   acerto real.
5. O resumo não cresce: no máximo uma linha a mais por chamada
   (`sections_for`, `exec/diagnostics.rs:65-169`), porque token é requisito de
   projeto — cada chamada traz `budget_tokens` e o resumo inteiro entra nesse
   orçamento por `render::finish` (`render.rs:18-30`).
6. `refresh: true` continua sendo a válvula de escape manual, sem mudança no
   `ExecParams` público nem no schema da tool.

## Alternativas rejeitadas

- **Remover o cache e os flags mortos.** Rejeitada pela medição: o acerto
  devolve 18×–90× o que a chave custa, e remover não resolveria nada do verde
  velho — o defeito é a chave, não a existência do cache. Também jogaria fora a
  única economia que cobre `test`/`clippy` de vários segundos.
- **Manter "todo byte sob o root" e só excluir `.tupi/`.** Rejeitada: conserta o
  sintoma num repositório e deixa o mesmo defeito em qualquer outro com arquivo
  volátil, sem erro visível; e o ambiente continuaria fora da chave, então agora
  o acerto passaria a servir verde velho de verdade.
- **Chave por `(len, mtime)`.** Rejeitada antes deste ADR: mentem com mesmo
  tamanho + mtime preservado (`cp -p`, `rsync -t`, `tar x`) — argumento já
  escrito em `render.rs:32-53`.
- **Persistir o cache em disco.** Fora de escopo e sem sujeito: não existe cache
  web no código, e o ganho medido é do processo, que já vive entre chamadas.
- **Cronometrar o walk + hash em `fingerprint` e reportar no resumo.** Rejeitada:
  o resumo não pode crescer por medição. A medição vive no harness
  (`fingerprint_cost`), fora do caminho de token.

## Consequências

- O cache passa a acertar — e é justamente por isso que o item 2 da decisão não
  é opcional: acerto com chave incompleta é o verde velho. A ordem de T2→T3
  importa.
- A chave continua sendo O(bytes lidos), ~1 ms/MB. Acima de 5.000 arquivos ou
  64 MB o `fingerprint` devolve `None` e o cache desliga; a descrição da tool
  (`exec/mod.rs:109-161`) e o resumo precisam dizer isso em vez de prometer
  cache que não existe (T4).
- `ToolsOptions::{exec_cache, web_cache}` (`crates/tupi-tools/src/lib.rs:52-81`)
  seguem documentados como "cache persistente", `true` por padrão e `false` em
  `hermetic`, mas não são lidos por ninguém — e `web_cache` não tem sujeito no
  código. Fica para T5: ligar `exec_cache` ao cache real e apagar `web_cache`.
- O miss continua silencioso: quem não acerta paga ~5 ms sem ver. Aceito — o
  preço de reportar seria uma linha de resumo por chamada.
- Arquivo volátil novo sob o root volta a derrubar o acerto. O teste de
  ponta a ponta do item 4 precisa de um root com `.tupi/` presente para provar
  que o acerto sobrevive a ele.
- Reprodução (offline):
  `cargo test -p tupi-tools -- fingerprint_cost --ignored --nocapture`, mais a
  sonda de composição por diretório de topo descrita acima.

## Execução (2026-11-16)

Implementado o conserto decidido acima, em `crates/tupi-tools`:

- Exclusões nomeadas com justificativa em `exec/render.rs`
  (`CACHE_EXCLUSIONS`), com `.tupi` incluído; `docs/`, `.agents/results/` e
  `.agents/journals/` continuam **na** chave, travadas por
  `exec/cache_tests.rs::docs_and_agent_results_stay_in_the_key`.
- Ambiente por prefixo (`CARGO_`, `RUSTUP_`, `RUSTFLAGS`, `RUSTC_`, `SQLX_`) e
  nomes soltos (`DATABASE_URL`, `CC`, `CFLAGS`, `PATH`), mais a versão resolvida
  de `rustc -Vv` (uma vez por processo, `OnceLock`) e o `config.toml` de
  `$CARGO_HOME`/`~/.cargo`. `RUST_LOG` fica de fora de propósito.
- `fingerprint` devolve `Result<_, CacheOff>`; o resumo diz
  `cache: hit|miss · chave Xms` ou `cache desligado: <motivo>` em uma linha.
- `ToolsOptions::exec_cache` passa a ser lido; `web_cache` foi removido (não
  existe cache web). `ExecTool::call` duas vezes seguidas acerta:
  `cache_tests.rs::two_identical_calls_hit_the_cache_and_a_source_change_misses`.
- Medição refeita com `fingerprint_cost`: repositório real **391 arquivos /
  3,3 MB — walk 3,7 ms + blake3 3,3 ms = 7,0 ms**; repo de 500 arquivos /
