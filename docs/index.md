# Documentação do projeto

## Filosofia

- [Filosofia](philosophy.md) — local para o que é mecânico, API só onde há
  raciocínio e nenhuma liberdade ao modelo.

## Arquitetura

- [Arquitetura](architecture.md) — camadas, fronteiras e componentes do workspace.

## Ferramentas

- [Tool `lookup`](features/lookup-tool.md) — um lote heterogêneo de ops
  (`symbol`, `skeleton`, `lines`, `text`, `semantic`, `graph`) com um orçamento,
  cap por item, corte anunciado, erro por item e dedupe por ledger; fundiu
  `read_many` e `search` e cortou o schema para 4.468 tokens no núcleo.
- [Verificação da escrita](features/verify.md) — comando do verificador do repo
  declarado por glob em `configs/verify.ron` (primeira regra que casa vence,
  caminho sem regra é no-op, falha é recusa com comando e saída) e âncora viva
  `caminho:linha@blake3` em documento; os dois módulos estão testados e ainda
  sem chamador no caminho de escrita.
- [Tool `shell`](features/shell-tool.md) — saída bruta por padrão (`terms` só
  filtra com `filter: true`), allowlist restrita (`cargo` fora, `git` só leitura),
  binário ausente vira aviso com equivalente e recusa sempre ensina a alternativa;
  sem memória do projeto dentro do resultado.
- [Tool `setup`](features/setup-tool.md) — bloco da lista viva de tools entre
  marcadores em `AGENTS.md` (merge ancorado por linha, duplicata colapsa,
  `apply` idempotente) e skill em `skills/tupi-tools/SKILL.md`; `preview` é o
  padrão e só `apply` escreve.
- [Prefixo do turno](features/turn-prefix.md) — memória e manifesto do que já veio
  entram no prefixo, nunca no resultado da tool: um julgamento de memória por
  turno e manifesto compacto (path · símbolo · blake3 · handle, ≤24 entradas, sem
  corpos) que corta re-leitura de 2 para 1 chamada por resultado reaberto.
- [Cliente MCP](features/mcp.md) — a seção `mcp` do `ai.project.json` vira uma
  tool só (`servers`/`tools`/`call`) falando JSON-RPC 2.0 por stdio, com spawn
  lazy, timeout por leitura e filho ceifado no drop; `remote` é recusado com
  mensagem que ensina.

## Modelos locais

- [Modelo de decisão `d1`](features/d1-decider.md) — artefato canônico
  (Q4_K_M, 1.099.629.888 bytes, SHA-256 e destino), download sob demanda por
  `ensure_decider_model` e custo de disco/rede; nada o consome ainda.

## Interface (TUI)

- [Uso das ferramentas na TUI](features/tui-tool-usage.md) — linha viva por tool
  (spinner, duração, glifo por categoria), agrupamento por turno e sub-chamadas de
  `pipeline` indentadas.

## Performance

- [Latência do turno](perf/turn-latency.md) — split provider×tool por turno, teto
  de passos (200 → 30, dois tetos separados), tokens de schema por passo: 10.613
  com `expose_all` contra 4.468 no núcleo (+6.145 por passo), `estimate_tokens` e
  `prompt_tokens` separados e a queda de re-leitura esperada.
- [Baseline de performance](perf/baseline.md) — números de build e do grafo,
  ajustes aplicados e lacunas de medição.
- [Baseline de build (T1.1)](perf/build-baseline.md) — tempos e tamanhos a frio
  antes de qualquer `[profile.*]`, com o script de reprodução.
- [Escrita por extensão](perf/write-baseline.md) — medição da task 0 sobre a
  telemetria já gravada: `write` de arquivo inteiro onde caberia edição cirúrgica
  e o veredito de que o endereço textual paga.
- [Alocador e linker: medidos e descartados](perf/allocator-and-linker.md) —
  `mimalloc` e `mold` não trouxeram ganho, inclusive no A/B refeito em release
  com concorrência (RSS 2,3× maior); o que ficou como harness.

## Comandos

- [`tupi install`](features/tupi-install.md) — contrato interno do subcomando que
  declara o diretório do binário no `PATH` (shell detectado, bloco marcado,
  idempotência, idioma da saída).
- [`tupi telemetry`](features/tupi-telemetry.md) — relatório por turno a partir do
  JSONL já gravado (`steps`, Σ `provider_us`, Σ `tool_us`, `re-read`,
  prompt/cached/completion), despachado no `main` antes de env/provider.

## Regras de negócio

- [Regras de negócio](business.md) — entidades, invariantes e fluxos.

## Convenções

- [Convenções](conventions.md) — onde cada coisa mora, escrita (endereço, guarda
  e verificação), testes e commits.

## Decisões

- [ADR 0000 (modelo)](decisions/0000-template.md) — esqueleto de toda decisão:
  contexto, decisão, alternativas rejeitadas e consequências.
- [ADR 0001](decisions/0001-cache-do-exec.md) — o cache de `exec` fica, com a
  chave restrita a insumos de build.
- [ADR 0002](decisions/0002-shell-poder-e-ruido.md) — o `shell` reduz poder e
  ruído: filtro opt-in, allowlist restrita, recusa acionável.
- [ADR 0003](decisions/0003-local-first.md) — o mecânico (embedding, resumo,
  transcrição) roda local; a API paga só o raciocínio do turno principal e do
  executor do SDD, e não existe roteador.
- [ADR 0004](decisions/0004-controle-por-tool.md) — nenhuma liberdade ao modelo:
  root confinado, edição com hash, allowlist no `shell`, orçamentos em
  `configs/` e descoberta obrigatória por `map`.
- [ADR 0005](decisions/0005-expose-all.md) — `expose_all` fica: o schema custa
  +6.145 tokens por passo (10.613 contra 4.468 no núcleo), mas é prefixo
  cacheado e não round trip — cortar arrisca passo extra de ~5,4 s.
- [ADR 0006](decisions/0006-teto-de-passos.md) — teto de 30 passos (2,5× sobre os
  12 medidos) e dois tetos com dois números: o turno lê `max_tool_steps`, o
  subagente do SDD lê `agent_max_steps` do request.
- [ADR 0007](decisions/0007-fusao-lookup.md) — `read_many` e `search` viram
  `lookup(ops)`: um orçamento, erro por item e cap declarado; o schema cai para
  4.468 (núcleo) e 10.613 (`expose_all`).
- [ADR 0008](decisions/0008-mcp-cliente-com-catalogo.md) — MCP entra como uma
  tool `mcp` com catálogo (config no repo), não N tools flat: schema grande não
  viaja no turno e o startup não espera servidor de terceiro.
- [ADR 0009](decisions/0009-mcp-http-cliente.md) — o transporte remoto do MCP
  reusa o `reqwest` já pinado (e já linkado via `tupi-providers`), dirigido por
  um runtime `current_thread` por conexão: nenhum crate novo, nenhuma feature a
  mais no pin e `Tool::call` continua síncrona.

## Regras

- [Documentação em `docs_folders`](rules/documentation.md)
- [Estado de agente vs. conhecimento de projeto](rules/knowledge-placement.md)
- [Local-first: o mecânico roda local, a API paga o raciocínio](rules/local-first.md)
- [Nenhuma instrução ao modelo sem tool ou limite](rules/model-control.md)
- [Workflow nativo em `configs/workflows/*.ron`](rules/workflows.md)
