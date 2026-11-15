<!-- Gerado automaticamente pelo tupi. Não edite; a fonte é meta.json/tasks.json. -->

# Retomar plano SDD com /sdd <slug> e autocomplete de specs abertas (`sdd-resume`)

**Status:** completed

## Objetivo

Permitir retomar um plano SDD persistido em `.tupi/sdd/<slug>/` sem depender da sessão em que ele nasceu: `/sdd <slug>` (com autocomplete dos slugs abertos) reativa o slug em `active.json`, entra em `AgentMode::Sdd` e deixa o contexto do plano disponível no histórico, de modo que o usuário digite apenas "continue" no turno seguinte. Sem retomada automática no boot e sem dashboard web nesta entrega (fontes: crates/tupi-tui/src/state.rs:445-481 para sugestões; crates/tupi-tui/src/run.rs:50-533 para o despacho de comandos `/`; crates/tupi-tools/src/tools/sdd/store.rs:181-211 para `active_slug`; crates/tupi-tools/src/tools/sdd/mod.rs:204-210 e 434-445 para o contrato `SddStore` e resolução de slug; crates/tupi-app/src/app.rs:421-472 para modo/`Busy`; crates/tupi-kernel/src/query.rs:9-22 para o bus de queries).

## Restrições

- Somente TUI: nada de `crates/tupi-web` nesta entrega.
- Sem retomada/aviso automático no boot — só por comando explícito.
- Planos `Completed` não aparecem no autocomplete e são recusados pelo comando; planos `Failed` continuam retomáveis (a execução pode ser retentada).
- `/sdd <slug>` não inicia turno: prepara modo + contexto e devolve o controle ao usuário.
- `/sdd` sozinho não lista overlay; a descoberta é pelo autocomplete ao digitar.
- Manter o padrão de `ChatState.workflows` (cache local carregado por query, sem travar o desenho).
- Nada de tocar em disco por acidente nos construtores de teste (`App` sem `sdd_store` deve degradar com mensagem clara).

## Nuances

- `SddStore::list(repo_key)` já devolve os planos do repositório; o filtro de status é da camada app, não do store (crates/tupi-tools/src/tools/sdd/store.rs:147-179).
- `SddStore::set_active_slug` é o mecanismo correto para 'retomar onde parou'; a tool `sdd` já resolve o slug pelo ativo quando `slug` é omitido (crates/tupi-tools/src/tools/sdd/mod.rs:434-445).
- `App::apply_mode` recusa troca com turno em andamento (`active != 0` → `CommandError::Busy`) e trava a saída do modo SDD quando `sdd_flow_started` está ligado (crates/tupi-app/src/app.rs:456-472); a retomada deve ligar `sdd_flow_started`, senão o usuário sai do modo fácil e pode editar o repo em execução.
- `refresh_suggestions` hoje exige `starts_with('/') && !contains(' ')` (crates/tupi-tui/src/state.rs:445-449); `/sdd ` quebra essa regra e precisa de um branch dedicado, no mesmo estilo do namespace `/workflows/`.
- Provedores recebem o histórico da sessão com o `prefix` como primeira mensagem `Role::System` (crates/tupi-app/src/app.rs:1919-1941); mensagem `system` no meio do histórico não é portável entre providers — o brief de retomada entra como `Role::User` com instrução explícita de não ser respondido.
- O brief deve ser curto e derivado do plano: título, status, goal, constraints, nuances, tasks abertas com a próxima subtask. `format_subtask_brief` e `format_execution_handoff` já existem em crates/tupi-app/src/sdd.rs:14-160 e devem ser reaproveitados, não duplicados.
- O autocomplete deve completar com espaço final (`/sdd auth `), senão o usuário digita Enter em `/sdd auth` e nunca chega ao turno; `accept_suggestion` hoje só cola o item (crates/tupi-tui/src/state.rs:517-522).
- Lista de planos na TUI precisa ser recarregada após `close`, `/new` e após a própria retomada, como `state.workflows` é preservado em `/new` (crates/tupi-tui/src/run.rs:50-533).

