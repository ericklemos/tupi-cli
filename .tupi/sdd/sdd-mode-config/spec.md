<!-- Gerado automaticamente pelo tupi. Não edite; a fonte é meta.json/tasks.json. -->

# Modo SDD parametrizável por configs/modes/sdd.ron embutido no binário (`sdd-mode-config`)

**Status:** failed

## Objetivo

Mover TODOS os textos de prompt e TODOS os parâmetros de comportamento do modo SDD para `configs/modes/sdd.ron`, parseado em tempo de compilação e embutido no binário, eliminando as constantes hardcoded hoje espalhadas entre crates/tupi-app, crates/tupi-tools e crates/tupi-kernel.

Escopo de prompt (hoje `const &str`): postura de planejamento `SDD_POSTURE_PROMPT` (crates/tupi-app/src/sdd.rs:5-9), prefixo do agente de implementação `AGENT_PREFIX` (crates/tupi-app/src/sdd_api.rs:16), prefixo do agente de docs `DOCS_AGENT_PREFIX` (crates/tupi-app/src/sdd.rs:3), brief da subtarefa (crates/tupi-app/src/sdd.rs:14-60), brief do agente de docs (crates/tupi-app/src/sdd_run.rs:406-417), handoff para execução nos dois ramos (crates/tupi-app/src/sdd.rs:89-160), relatório de execução (crates/tupi-app/src/sdd.rs:63-87) e as descrições/enums da tool `sdd` por modo (crates/tupi-tools/src/tools/sdd/mod.rs:342-456 e :651+).

Escopo de comportamento: allowlist de tools do modo (hoje `sdd_allowed` default `false` em crates/tupi-kernel/src/tool.rs:108-110 + filtro em :139-154), `max_parallel`/`agent_max_steps`/`agent_timeout_seconds`/`auto_execute` do runner (hoje settings do redb consumidos em crates/tupi/src/main.rs:19-480 e injetados no RunContext de crates/tupi-app/src/sdd_run.rs:116-134), trava do modo ao abrir plano (crates/tupi-app/src/app.rs:2452), docs_agent on/off, mirror markdown e budget default de tokens das tools.

Desenho do arquivo (quatro blocos, todo campo opcional): `planning` (posture, tools, lock_mode_on_open, tool_budget_tokens), `execution` (agent_prefix, docs_agent_prefix, subtask_brief, docs_brief, handoff_done, handoff_open, run_report), `tool` (planning_actions, execution_actions, planning_description, execution_description) e `runner` (max_parallel, agent_max_steps, agent_timeout_secs, auto_execute, docs_agent, mirror_markdown).

O arquivo é EMBUTIDO (sem override por projeto nem leitura em runtime), com parse no build.rs e acesso por `tupi_kernel::sdd_config()` sobre `OnceLock`.

## Restrições

- Sem override em runtime: o RON entra no binário em tempo de compilação; não existe .tupi/sdd.ron, nem leitura do arquivo em produção, nem env var.
- RON é a ÚNICA fonte dos valores do modo SDD: as chaves do redb `sdd.max_parallel`, `sdd.agent_max_steps`, `sdd.agent_timeout_seconds` e `sdd.auto_execute` (lidas em crates/tupi/src/main.rs:19-480 via `setting_usize`) saem de cena, e o helper `setting_usize` (crates/tupi/src/main.rs:65-72) é removido junto.
- Arquivo único em `configs/modes/sdd.ron`, irmão de `configs/workflows/`; nenhuma outra pasta de config é criada.
- Sem duplicar a config base do agente: configs/prompts.ron (crates/tupi/src/prompt_config.rs:8-11) continua sendo a fonte do `system` geral, e compose_prefix (crates/tupi-app/src/sdd.rs:162-173) continua anexando a postura SDD por cima.
- Campos ausentes usam como default os textos e valores atuais — compatibilidade de comportamento e de testes.
- Não quebrar o contrato tool<->prompt: o enum de ações da tool `sdd` e o texto que a descreve saem do mesmo bloco de config.
- O tipo da config vive em `tupi-kernel` (já é dependência de tupi-tools e tupi-app); `tupi-domain` NÃO ganha a dependência `ron`.
- Parse em tempo de compilação no `build.rs` de tupi-kernel (novo), gerando código tipado com `ron` como build-dependency; RON inválido quebra o build apontando o caminho. `include_str!` cru é proibido.
- Manter o filtro por modo em ToolRegistry::specs_for_mode (crates/tupi-kernel/src/tool.rs:139-154) como único ponto de corte: a config só troca a lista de nomes, não a mecânica.
- Nada de I/O de arquivo no caminho quente do turno: config resolvida uma vez e cacheada.

## Nuances

- Chaves do redb hoje em uso (todas morrem com esta mudança): `sdd.auto_execute` (default true), `sdd.max_parallel` (4), `sdd.agent_max_steps` (24), `sdd.agent_timeout_seconds` (600) — crates/tupi/src/main.rs:19-480.
- `auto_execute` controla se o AgentExecutor é registrado em `tool_options.sdd_executor`; desligado, `sdd(action="close")` não dispara execução automática.
- build.rs de crates/tupi-tools:5-45 já lê configs/workflows relativo a CARGO_MANIFEST_DIR/../..; o build.rs novo de tupi-kernel usa o mesmo esquema e o mesmo padrão de rerun-if-changed.
- Existem testes que fixam literais do prompt: crates/tupi-app/tests/sdd_mode.rs:215-267 exige que a primeira mensagem de sistema contenha "SDD"; crates/tupi-app/src/prompt.rs:20-25 e :34-37 fixam o texto de TUPI_SYSTEM; crates/tupi-tools/src/lib.rs:537-576 e :596-617 fixam a allowlist do modo.
- format_subtask_brief, format_execution_handoff e format_run_report intercalam prosa fixa com listas dinâmicas (constraints, nuances, tasks, subtasks, result.md em .tupi/sdd/<slug>/); os trechos dinâmicos viram placeholders resolvidos em código, não templates de laço no RON.
- O handoff seleciona entre dois textos conforme a existência de .tupi/sdd/<slug>/result.md (crates/tupi-app/src/sdd.rs:89-101): são dois campos distintos, não um com condicional.
- format_run_report tem dois formatos de cabeçalho: quando há subtasks conta subtasks; quando não há, conta tasks (crates/tupi-app/src/sdd.rs:74-82).
- DocsFeedbackStatus é calculado em run_docs_feedback (crates/tupi-app/src/sdd_run.rs:326-404) e as strings "docs desabilitado" / "plano falhou; docs não realimentados" aparecem no plan.reason.
- As Notices em português do runner (ex.: "subtasks com arquivos sobrepostos; rodando em série", crates/tupi-app/src/sdd_run.rs:143-309) e as mensagens de erro de require_sdd_mode/require_execution_mode (crates/tupi-tools/src/tools/sdd/mod.rs:412-419) são texto de UI, não de prompt: ficam fora do escopo inicial.
- RunContext é pub(crate) e alimentado por SddExecRequest (crates/tupi-app/src/sdd_api.rs:27-45); o AgentExecutor em crates/tupi/src/main.rs:19-480 é quem injeta os valores hoje.
- A docsa_agent factory em main depende de docs_folders não vazio; `runner.docs_agent: false` deve forçar a fábrica a None mesmo com docs_folders configurado.
- `Cargo.toml` da workspace já tem `ron = "0.12"` (Cargo.toml:26); em tupi-kernel ele entra como [build-dependencies], não como dependência de runtime.

