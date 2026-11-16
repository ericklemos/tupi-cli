# Filosofia

Três decisões explicam o resto do projeto: o trabalho mecânico roda em modelo
local, a API paga só o raciocínio, e o modelo nunca ganha liberdade — ele
escolhe o que pedir dentro de um conjunto fechado de tools, com processos
estritos em volta. Cada princípio cita o código que o sustenta hoje; onde algo
ainda não existe, está dito.

## P1 — Local para o que é mecânico

Embedding e resumo não saem da máquina. O binário decide se baixa os modelos
pequenos a partir da configuração do projeto, não do humor do modelo:

- `crates/tupi/src/config.rs:313-320` (`local_embedding_enabled`), `:322-324`
  (`docs_embedding_enabled`) e `:326-328` (`summaries_enabled`) ligam o
  download quando existe pasta de documentação declarada e nenhum `off`/`0`/
  `false` na variável de ambiente.
- Os modelos são nomeados no binário: `LiquidAI/LFM2.5-Embedding-350M-GGUF`
  (`crates/tupi-infer/src/lib.rs:23-27`) e `LiquidAI/LFM3-1.7B-GGUF`
  (`crates/tupi-infer/src/lib.rs:29-32`).
- Carregar é opcional por construção: `load_embedder`
  (`crates/tupi/src/local_models.rs:14-22`) e `load_generator` (`:36-44`)
  devolvem `Option`; sem a feature `local-models` o slot fica vazio e a busca
  segue lexical (`crates/tupi/src/local_models.rs:26-32` e `:48-54`).
- `transcribe` segue a mesma regra: o subcomando responde no `main` antes de
  abrir redb e provedor (`crates/tupi/src/main.rs:21-506`) e a inferência roda
  no processo local (`crates/tupi-transcribe/src/engine.rs:57-105`).
- O local não decide o turno: embedder e gerador entram como
  `ToolsOptions.embedder` e `ToolsOptions.generator`
  (`crates/tupi-tools/src/lib.rs:52-79`), ambos `Option`, com a busca lexical
  como piso quando não há modelo.

## P2 — API só onde o raciocínio é necessário

O turno principal é o único lugar que sempre fala com provedor remoto, e o
executor do SDD é o único outro consumidor de modelo:

- O provedor e o modelo vêm do ambiente e da seleção salva, com chave
  obrigatória fora do fluxo Antigravity: `from_env`
  (`crates/tupi/src/config.rs:27-59`) devolve `Config { provider, model, … }`.
- O caminho que dispara agentes é explicitamente o do SDD: `AgentExecutor`
  carrega `provider` e `model` (`crates/tupi/src/main.rs:22-37`) e monta o
  `SddExecRequest` com esses valores (`crates/tupi/src/main.rs:39-71`); ele é
  publicado como `ToolsOptions.sdd_executor` (`crates/tupi-tools/src/lib.rs:52-79`).
- **O que ainda não existe:** não há roteador local↔API para o turno
  principal. Não há escolha automática de modelo por dificuldade da pergunta; a
  API é usada porque é o caminho do turno, e o local cobre apenas as tarefas
  mecânicas do P1. Qualquer texto que prometa roteamento hoje mente.

## P3 — Nenhuma liberdade: tools e processo estritos

O modelo não recebe shell livre, não escreve fora do projeto e não edita às
cegas. Cada liberdade que sobra é medida:

- **Root confinado.** `resolve_for_write`
  (`crates/tupi-tools/src/support/paths.rs:53-101`) canonicaliza, normaliza e
  recusa com `OutsideRoot` tudo que escapa do diretório onde o Tupi foi aberto.
- **Edição com hash.** `EditParams` exige `expected_hash`
  (`crates/tupi-tools/src/tools/edit/params.rs:4-11`); o hash velho vira
  `StaleHash`, a escrita é atômica e o arquivo é restaurado se a verificação
  pós-escrita divergir (`crates/tupi-tools/src/tools/edit/mod.rs:41-212`).
- **Execução limitada.** `exec` roda apenas `cargo check/build/test/clippy/
  xtask`, com `package`/`target`/`filter` declarados e `timeout_secs` opcional
  (`crates/tupi-tools/src/tools/exec/mod.rs:42-60`), cache por fingerprint com
  `refresh` para furar (`:163-205`) e teto de timeout (`:25-26`).
- **Shell só de leitura.** `allowed` (`crates/tupi-tools/src/tools/shell/mod.rs:61-157`)
  aceita formatador e `git` de leitura, recusa composição, wrappers
  (`sh`, `xargs`, `python`, `make`, …) e `cargo` fora de `cargo fmt`; a
  allowlist padrão é curta e explícita (`:434-437`).
- **Orçamentos que fecham o turno.** `max_tokens` por resposta
  (`crates/tupi/src/token.rs:17-20`), `max_tool_steps`/`expose`/`expose_all`
  (`crates/tupi/src/tool_config.rs:16-25`) e a poda de contexto com
  `budget`/`target`/`recent_results` (`crates/tupi/src/config.rs:170-178`).
- **Processo, não sugestão.** A descoberta começa em `map` → `read`/`search`
  e o modelo é proibido de presumir o repositório
  (`configs/prompts.ron:6-12`); as instruções de projeto entram inteiras no
  prompt com teto de 32 KiB, truncadas se passarem
  (`crates/tupi/src/agents_config.rs:3`, `:9-39`).

## P4 — Prompt curto: o que entra é escolhido, não acumulado

O modelo não recebe o repositório. Recebe um prefixo de três partes, e só:
`compose_startup_prefix` (`crates/tupi/src/main.rs:579-595`) concatena prompt
de sistema, seção de workflows e `AGENTS.md`, nessa ordem, nada mais.

- O prompt de sistema é um arquivo curto: o `system` de
  `configs/prompts.ron:4-15` são sete regras de uma linha, e a primeira obriga
  a descobrir por `map` → `read`/`search` sem presumir caminho (`:6`).
- `AGENTS.md` inteiro entra no prefixo com teto de 32 KiB, cortado em fronteira
  de caractere e anunciado (`MAX_AGENTS_BYTES`,
  `crates/tupi/src/agents_config.rs:3`; `load_agents_section`, `:9-39`). O teto
  é contrato: instrução de projeto precisa ser curta e densa para caber.
- As descrições de tool também são orçamento: `expose`/`expose_all`
  (`crates/tupi/src/tool_config.rs:16-25`) escolhem entre núcleo, lista
  explícita e todas (`Exposure`, `crates/tupi-tools/src/lib.rs:452-459`),
  aplicado por `register_exposed` (`:475-481`). O arquivo atual expõe todas e
  limita 100 passos (`configs/tools.ron:5-6`).
- Resultado de tool não fica inteiro no prompt: `finish`
  (`crates/tupi-tools/src/tools/exec/render.rs:21-33`) seleciona as seções sob
  `budget_tokens` e manda o excedente ao store, que volta por handle em `peek`.
- O histórico é podado, não eternizado: `ContextConfig { budget, target,
  recent_results }` (`crates/tupi/src/config.rs:170-178`) chega a
  `plan_context` (`crates/tupi-app/src/app.rs:142-168`), que corta até o alvo
  amortizado e mantém inteiros só os resultados recentes.
- A resposta também tem teto: `TokenConfig`/`DEFAULT_MAX_TOKENS`
  (`crates/tupi/src/token.rs:12`, `:17-20`).

## P5 — Medir antes de otimizar; sem número, não fica

Mudança de desempenho só sobrevive com número, e o critério vem escrito antes
do resultado.

- Toda mudança de perfil de build se compara com `docs/perf/build-baseline.md`:
  build release a frio de 82.3 s e binário de 33.2 MiB, reproduzíveis por
  `scripts/perf/measure-build-baseline.sh`. Sem ganho medido, a mudança é
  revertida em um commit.
- O que foi medido e descartado fica registrado, não apagado: `mimalloc`
  (build do grafo mais lento, pico de RSS maior) e `mold` (19.84 s → 19.89 s,
  dentro do ruído) foram implementados, medidos e removidos; as sondas ficaram
  como harness permanente (`crates/tupi/tests/alloc_bench.rs`,
  `crates/tupi/tests/link_probe.rs`) em `docs/perf/allocator-and-linker.md`.
- O critério vem antes do resultado: o gate do cache de `exec` ("o acerto
  economiza mais do que o walk + blake3 custa") está no
  [ADR 0001](decisions/0001-cache-do-exec.md), junto da medição que decidiu
  manter o cache e consertar a chave.
- Medição cara roda fora do caminho do turno: as sondas são testes de
  integração próprios (`crates/tupi/tests/alloc_bench.rs`,
  `link_probe.rs`, `release_probe.rs`), não passos do agente, e o resumo de uma
  tool não cresce para reportar medição — reportar custa token e a medição fica
  no harness ([ADR 0001](decisions/0001-cache-do-exec.md)).
- Quem executa o efeito é quem mede: `Event::ToolFinished` carrega
  `duration_us` de `tupi-app`, e o consumidor formata o número em vez de
  reinventá-lo (`docs/architecture.md`, seção Fronteiras).

## P6 — Doc e contrato: o que não está indexado não existe

Documento e regra são contrato: valem quando o agente os alcança pela tool e
quando o arquivo gerado respeita os marcadores.

- Só é indexado o que está declarado: `docs_folders` em `ai.project.json:5`
  (`./docs`), varrido por `DocsIndex`
  (`crates/tupi-tools/src/tools/docs/index.rs:58-69`) e nada além disso. Regra
  escrita em `.agents/` existe no disco e não existe para a busca
  (`docs/rules/knowledge-placement.md`, `docs/rules/documentation.md`).
- A leitura é por tool, não por sorte: `docs` expõe `map`, `query`, `read`,
  `status` e `refresh`, mais `write` quando a pasta é gravável, e trata o
  conteúdo como referência, não como instrução
  (`crates/tupi-tools/src/tools/docs/mod.rs:180-204`). Documento novo entra em
  `docs/index.md`, e `refresh` reindexa o que mudou.
- `AGENTS.md` é gerado, não escrito à mão: `setup` mantém o bloco entre dois
  marcadores fixos (`crates/tupi-tools/src/tools/setup/mod.rs:14-15`) e `merge`
  (`:74-87`) troca só esse trecho — texto autoral fora dos marcadores sobrevive
  ao `apply`, `preview` é o padrão e só `apply` grava (`SetupTool::call`,
  `:164-222`). O marcador só conta quando ocupa a linha inteira (`marker_line`,
  `:90-100`) e bloco duplicado é descartado (`without_extra_blocks`,
  `:103-119`), então duas execuções convergem para um bloco
  ([`features/setup-tool.md`](features/setup-tool.md)).
- O contrato também é de tamanho: o `AGENTS.md` inteiro entra no prompt (P4)
  com teto de 32 KiB, e o texto gerado é a lista viva de tools
  (`SetupTool::spec`, `crates/tupi-tools/src/tools/setup/mod.rs:131-162`).
- Mudança de contrato observável fecha no mesmo commit: CLI, flag ou saída
  mudaram, `docs/` e `README.md` mudam junto (`docs/conventions.md`).

## Local × API: hoje e direção

A coluna **hoje** cita o código. A coluna **direção** é intenção declarada e
**nada dela está implementada**: enquanto não existir roteador, o turno
principal é sempre API.

| tarefa | hoje | direção |
|---|---|---|
| turno principal (chat/TUI) | API: `provider`/`model` de `Config::from_env` (`crates/tupi/src/config.rs:27-59`) | local primeiro, API só quando o turno exigir raciocínio |
| executor do SDD | API: `AgentExecutor` carrega `provider` e `model` (`crates/tupi/src/main.rs:22-37`) | mesma regra do turno principal |
| embedding de docs | local, `LFM2.5-Embedding-350M`, opcional; sem modelo a busca cai no lexical (`crates/tupi/src/config.rs:313-320`) | cobrir também a busca dentro do código, não só a documentação |
| resumo de documentos | local, `LFM3-1.7B`, opcional, desligável por ambiente (`crates/tupi/src/config.rs:326-328`) | resumo do próprio contexto do turno (compactação) também local |
| `transcribe` | local (whisper), sem chave de API (`crates/tupi/src/main.rs:21-506`) | idem, sem depender de rede |
| roteamento local↔API por dificuldade | **não existe** | escolher no turno qual modelo responde |
