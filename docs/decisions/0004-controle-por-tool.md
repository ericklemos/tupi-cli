# ADR 0004: controle por tool — o modelo não ganha liberdade, ganha contrato

## Contexto

O que o modelo obedece é prompt; o que o código garante é contrato. O Tupi
escolheu o segundo: o agente não recebe shell livre, não escreve fora do
projeto, não edita sem hash e não decide quantos passos dar. Tudo o que limita o
modelo hoje é uma tool que recusa ou um número em `configs/`:

- **Root confinado.** `resolve_for_write` canonicaliza, normaliza e recusa com
  `OutsideRoot` tudo que resolva fora do diretório onde o Tupi foi aberto
  (`crates/tupi-tools/src/support/paths.rs:53-101`); `write`, `write_many`,
  `move`, `patch` e `setup` passam por ele.
- **Edição com hash.** `EditParams` exige `expected_hash`
  (`crates/tupi-tools/src/tools/edit/params.rs:4-11`); hash divergente vira
  `StaleHash` sem escrever, a escrita é atômica e o original é restaurado quando
  o que ficou em disco não é o enviado (`Edit::edit`,
  `crates/tupi-tools/src/tools/edit/mod.rs:41-212`; `a_stale_hash_never_writes`,
  `crates/tupi-tools/src/tools/edit/tests.rs:62-73`). `write`/`write_many` só
  sobrescrevem com `expected_hash`.
- **Execução limitada a alvos declarados.** `exec` roda `cargo
  check/build/test/clippy/xtask` com `package`/`target`/`filter`/`task`
  explícitos, `budget_tokens` e `timeout_secs` opcional
  (`ExecParams`, `crates/tupi-tools/src/tools/exec/mod.rs:42-60`), com o timeout
  preso entre `DEFAULT_TIMEOUT_SECS` e `MAX_TIMEOUT_SECS`
  (`crates/tupi-tools/src/tools/exec/args.rs:122-129`) e cache que se anula em
  silêncio quando a chave não é confiável (ver
  [ADR 0001](0001-cache-do-exec.md)).
- **Shell de leitura, allowlist curta.** `allowed`
  (`crates/tupi-tools/src/tools/shell/mod.rs:61-157`) recusa composição, recusa
  `--manifest-path`, recusa wrappers (`sh`, `env`, `xargs`, `python`, `make`,
  `just`, …), mantém `cargo` fora exceto `cargo fmt`/`rustfmt`, aceita `git`
  apenas em subcomandos de leitura (`GIT_READ`) e recusa `-c`/`--config-env`; o
  padrão é uma lista curta e explícita (`ShellTool::DEFAULT_ALLOW`,
  `:434-437`). A recusa ensina a alternativa (ver
  [ADR 0002](0002-shell-poder-e-ruido.md)).
- **Orçamentos que fecham o turno.** `max_tokens` por resposta
  (`TokenConfig`, `crates/tupi/src/token.rs:17-20`),
  `max_tool_steps`/`expose`/`expose_all` (`crates/tupi/src/tool_config.rs:16-25`,
  hoje 200 passos e `expose_all` em `configs/tools.ron:5-6`), a escolha entre
  núcleo, lista e todas (`Exposure`, `crates/tupi-tools/src/lib.rs:454-461`) e a
  poda de contexto com `budget`/`target`/`recent_results`
  (`crates/tupi/src/config.rs:170-178`). Cada resultado de tool ainda cabe em
  `budget_tokens`, com o excedente no store e volta por `peek`.
- **Prompt curto e gerado.** `AGENTS.md` inteiro entra no prefixo com teto de
  32 KiB, truncado em fronteira de caractere e anunciado
  (`MAX_AGENTS_BYTES`, `crates/tupi/src/agents_config.rs:3`;
  `load_agents_section`, `:9-39`), e o bloco de tools é gerado pela tool `setup`
  entre marcadores, com `merge` trocando só esse trecho
  (`crates/tupi-tools/src/tools/setup/mod.rs:74-87`, `:164-222`).
- **Descoberta por tool.** O prompt de sistema obriga `map(depth="dirs")` →
  `map(depth="files", path)` → `read`/`search` e proíbe presumir caminho fora do
  `map` (`configs/prompts.ron:6-8`), e a memória nunca vence a tool
  (`:9-10`). Hoje isso é texto de prompt; este ADR o torna regra dura.

O incentivo por trás: toda instrução que não cai em tool ou em número é pedido,
e o descumprimento só aparece depois do estrago. A telemetria de `shell`
registrada em [ADR 0002](0002-shell-poder-e-ruido.md) mostra o custo do
contrário — liberdade ampla produzida por allowlist larga e filtro silencioso.

## Decisão

1. **Toda instrução dada ao modelo tem um contrato executável**: ou uma tool que
   verifica, recusa ou desfaz, ou um limite numérico em `configs/*.ron`. O que
   não tem nenhum dos dois é declarado como pedido, com o contrato que falta.
2. **Para restringir, reduza poder; nunca acrescente escape.** Não existe
   `--force`, `dangerously`, modo permissivo nem confirmação que libere o que a
   allowlist recusa. Correção no `shell`/`exec` só pode encolher o conjunto do
   que pode ser executado (`docs/architecture.md`, "Poder de execução só
   encolhe").
3. **Recusa ensina.** Composição cita `-e` e a tool `search`; binário fora da
   allowlist cita `shell.allow` e o substituto nativo; binário ausente vira aviso
   com equivalente. Recusa muda custa um round trip inteiro ao modelo.
4. **Escrita é verificável por hash e confinada ao root.** Nenhum caminho novo de
   escrita ignora `paths::resolve_for_write` nem dispensa `expected_hash` em
   sobrescrita; edição cirúrgica é o caminho normal, `write` de arquivo inteiro é
   a exceção declarada.
5. **Descoberta é regra dura, não sugestão**: o agente começa por
   `map(depth="dirs")` e só lê caminho que o `map` devolveu; tool vence memória —
   conteúdo lido fora da sessão não pode ser afirmado. Vale para `AGENTS.md`,
   `README.md` e qualquer prompt novo.
6. **O limite mora em `configs/`, não em adjetivo.** Para gastar menos, baixe
   `max_tokens`, `max_tool_steps`, troque `expose_all` por `expose` ou aperte
   `budget`/`target`/`recent_results`; "seja econômico" no prompt não é limite.
7. **`AGENTS.md` é curto e gerado.** O texto autoral fica fora dos marcadores
   (sobrevive a `setup apply`) e o arquivo inteiro precisa caber em 32 KiB,
   porque é requisito de prompt, não documentação livre.
8. **Capacidade nova entra pelo mesmo teste**: ao criar tool, subcomando ou
   workflow que chama modelo, dizer na descrição qual contrato a limita.

## Alternativas rejeitadas

- **Shell livre com confirmação humana por comando.** Rejeitada: a TUI roda
  turnos autônomos e confirmação em série vira "sim" por reflexo — o custo
  sobra e a garantia some. Sem confirmação, `cargo run` e `git commit` passariam.
- **Flag de escape (`--force`, modo permissivo) para destravar recusa.**
  Rejeitada: reproduziria por outro caminho exatamente o que a allowlist corta;
  o requisito do projeto é que poder de execução só encolha.
- **Confiar no prompt ("seja cuidadoso", "não edite às cegas", "não invente
  caminho").** Rejeitada: não verificável e sem desfazer. O que existe é hash,
  root confinado, allowlist e orçamento — e é isso que a documentação pode
  chamar de garantia.
- **Sandbox de sistema operacional (container, seccomp) em vez de allowlist.**
  Rejeitada no estado atual: isola processos, não isola decisão — o modelo
  continuaria podendo `cargo run` e `git push` dentro do sandbox —, e o custo de
  portabilidade é alto. Reduzir superfície na tool ataca a causa direta.
- **Manter `terms` filtrando por padrão e melhorar só o aviso.** Rejeitada em
  [ADR 0002](0002-shell-poder-e-ruido.md): omissão sem pedido é o defeito, não a
  falta de aviso.
- **Descrição longa de todas as tools para "ensinar" o modelo.**
  Rejeitada: descrição de tool é orçamento de prompt; `expose`/`expose_all`
  escolhem o conjunto e o resultado é cortado por `budget_tokens`.
- **`map` opcional ("presuma o caminho se souber").** Rejeitada: leitura por
  memória é como caminho inventado entra na sessão; o `map` é a única porta de
  caminho descoberto.

## Consequências

- O agente paga descoberta: passos em `map`/`read` antes de agir. Aceito — sai
  mais barato que uma edição em alvo errado ou uma afirmação não lida.
- Toda tool nova tem custo de contrato: precisa recusar, ensinar a alternativa e
  caber no orçamento. Sem isso, o texto sobre ela é pedido, não regra.
- Recusas são superfície de manutenção: mudar de allowlist ou de limite é editar
  código/`configs/` e ajustar a mensagem — nunca afrouxar em silêncio.
- `AGENTS.md` é apertado por construção: 32 KiB é teto duro
  (`crates/tupi/src/agents_config.rs:3`) e o bloco gerado por `setup` cresce com
  o número de tools; texto autoral precisa ser denso.
- Testes que travam o contrato hoje: `a_stale_hash_never_writes`,
  `an_already_applied_body_with_a_stale_hash_is_idempotent`
  (`crates/tupi-tools/src/tools/edit/tests.rs`),
  `the_default_allowlist_refuses_cargo_run`,
  `a_composition_refusal_teaches_the_alternative`,
  `a_missing_binary_is_reported_instead_of_failing_the_call`
  (`crates/tupi-tools/src/tools/shell/mod.rs`),
  `preview_does_not_write` e `apply`
  (`crates/tupi-tools/src/tools/setup/mod.rs`) e
  `the_shell_result_never_carries_project_memory`
  (`crates/tupi-app/tests/shell_memory.rs`).
- Regra escrita em `.agents/` continua invisível: regra de projeto vive sob
  `docs/` (`docs_folders`, `ai.project.json:5`) — ver
  [`docs/rules/knowledge-placement.md`](../rules/knowledge-placement.md) e
  [`docs/rules/model-control.md`](../rules/model-control.md).
