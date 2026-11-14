<!-- Gerado automaticamente pelo tupi (somente leitura; fonte de verdade no banco local). -->

# Ajuste de janela de contexto por modelo/provider no ai.project.json (`model-context-window`)

**Status:** closed

## Objetivo

Permitir declarar, em ai.project.json, uma janela de contexto por modelo (`models.<chave>.context_window`, chave composta `provider/modelo` com fallback para o nome do modelo), sobrescrevendo o `budget` de configs/context.ron. O valor passa a valer na inicialização E quando o usuário troca de modelo/provider em runtime (`Command::SetModel`), recalculando a poda e atualizando o indicador `ctx X/budget` da TUI sem reiniciar.

## Restrições

- Somente leitura de ai.project.json: nenhuma escrita nova além do fluxo atual de load_or_create; arquivos existentes sem o campo `models` continuam válidos e não são regravados
- configs/context.ron permanece o default global (budget, target, recent_results); ai.project.json só sobrescreve o budget do modelo, sem apagar o restante
- tupi-app não pode depender de tupi (binário): o tipo de perfil/resolução vive em tupi-app e é preenchido em crates/tupi/src/main.rs
- Não mudar a semântica existente de budget == 0 (desliga poda) nem a correção por token_scale (scaled_budget)
- Resolução de modelo deve ser feita antes do escalonamento por token_scale e antes de plan_context
- Manter compatibilidade dos construtores App::with_context_budget/with_context_target/with_context_recent_results usados em testes
- Nenhuma chamada de rede, telemetria ou I/O no caminho de resolução em runtime

## Nuances

- ProjectConfig hoje tem apenas `name` (crates/tupi/src/project_config.rs:10) e ignora campos desconhecidos (teste unknown_fields_are_ignored)
- ContextConfig em crates/tupi/src/config.rs:114 tem budget/target/recent_results e vem de configs/context.ron (budget 64000, target 32000, recent_results 6)
- App guarda context_budget/context_target/context_recent_results em AtomicUsize (crates/tupi-app/src/app.rs:168) e send_message os lê uma vez por turno via self.context_budget.load
- A troca de modelo já existe: App::set_model (crates/tupi-app/src/app.rs:404) atualiza provider_id/model, salva no redb e devolve CommandOutcome::ModelChanged { model }
- A TUI mostra ocupação via ChatState.context (ContextUsage { tokens, budget }) alimentado por Event::ContextUpdated publicado por passo em send_message; draw_details usa state.context (crates/tupi-tui/src/run.rs:878)
- Event::ContextUpdated carrega tokens+budget por passo; ao trocar de modelo fora de um turno não há evento com tokens, então o budget precisa de um evento próprio para não inventar ocupação
- token_scale corrige subestimação do tokenizador (scaled_budget/observed_scale/next_scale) e deve continuar atuando depois da resolução por modelo
- Comando /modelos abre overlay e despacha SetModel apenas quando ocioso (CommandError::Busy se active != 0)
- load_or_create grava ai.project.json com apenas { name } quando o arquivo não existe; testes exigem que campos desconhecidos não quebrem o parse
- crates/tupi/src/main.rs é o único ponto que lê os arquivos de configs/ e injeta valores no App via builders

## Tasks

- [~] **T1** Modelar `models` no ai.project.json
  - Adicionar `models: BTreeMap<String, ModelConfig>` a ProjectConfig (serde default + skip_serializing_if vazio) e `ModelConfig { context_window: Option<usize> }`. Manter tolerância a campos desconhecidos no topo e dentro da entrada do modelo.
  - [x] **T1.1** Estender ProjectConfig e ModelConfig em crates/tupi/src/project_config.rs — ProjectConfig tem `name` e `models` (default vazio, omitido na serialização quando vazio); ModelConfig tem `context_window: Option<usize>`; parse de arquivo sem `models` continua ok e `write_project` gera o mesmo JSON de antes para o caso mínimo.
  - [ ] **T1.2** Cobrir parse em testes unitários — Testes em project_config.rs cobrem: chave composta `provider/modelo`, chave só com o nome do modelo, `models` ausente (default vazio), entrada com campo desconhecido (ignorado), context_window não numérico gera ProjectConfigError::Parse com o caminho.
- [~] **T2** Resolver janela de contexto por provider/modelo em tupi-app
  - Criar módulo `crates/tupi-app/src/context.rs` com ContextLimits { budget, target, recent_results }, ModelContext { context_window }, ContextProfile { base, models } e `resolve(provider, model) -> ContextLimits`. Precedência: `provider/modelo` exato, depois `modelo`; sem entrada → base. context_window > 0 substitui o budget e o target preserva a proporção do default (cai para 2/5 quando base.budget ou base.target forem 0); context_window 0 ou ausente mantém o default.
  - [x] **T2.1** Implementar ContextProfile::resolve — resolve devolve base quando não há entrada; quando há context_window > 0, budget = context_window e target derivado preservando a proporção target/budget do default (target = window * base.target / base.budget, com fallback 2/5 quando base.budget == 0 ou base.target == 0); target nunca excede o budget efetivo; context_window == 0 ou ausente mantém o default; recent_results sempre vem da base.
  - [ ] **T2.2** Testar prioridade e bordas da resolução — Testes cobrem: hit exato provider/modelo, fallback só-modelo, ausência total, context_window 0, base.budget 0, base.target 0 e target clampado ao budget.
- [~] **T3** Aplicar o perfil no App na inicialização e na troca de modelo
  - App passa a guardar Arc<ContextProfile>, ganha `with_context_profile` e um `apply_context(provider, model)` que grava os atomics efetivos. Chamar na construção (with_tools) e em set_model; em set_model publicar evento com o novo budget para a TUI atualizar o indicador sem depender de um novo turno.
  - [ ] **T3.1** Adicionar perfil e with_context_profile ao App — App mantém `context_profile: Arc<ContextProfile>`; with_context_profile substitui o perfil; with_context_budget/target/recent continuam funcionando e ajustam a base do perfil.
  - [x] **T3.2** Recalcular na inicialização e em set_model — Ao construir o App com perfil, os atomics já refletem o modelo/provider inicial; ao despachar Command::SetModel, os atomics passam a refletir o novo modelo imediatamente e o turno seguinte usa o novo budget.
  - [x] **T3.3** Publicar o novo budget na troca de modelo — set_model publica um evento com o budget efetivo do novo modelo (novo variant em Event, ex.: ContextBudget { budget }), sem tokens inventados; set_model continua retornando CommandOutcome::ModelChanged e respeitando CommandError::Busy.
- [ ] **T4** Atualizar a TUI imediatamente
  - ChatState ganha `set_context_budget(budget)` que atualiza apenas o budget de ContextUsage existente (ou ignora quando não há ocupação conhecida). O loop de eventos de crates/tupi-tui/src/run.rs trata o novo evento.
  - [ ] **T4.1** Adicionar set_context_budget e teste — set_context_budget preserva o tokens já conhecido e troca o budget; sem ContextUsage prévio nada aparece na tela (context continua None) e há teste para ambos os casos.
  - [ ] **T4.2** Consumir o evento no loop do run — O match de eventos em crates/tupi-tui/src/run.rs chama set_context_budget ao receber o novo variant, mantendo o resto do comportamento (notices, overlay, etc.) intacto.
- [~] **T5** Ligar no binário e documentar
  - main.rs converte project.models + config::load_context em ContextProfile e injeta via with_context_profile, logando o budget resolvido para o modelo inicial. Documentar o formato, mantendo o ai.project.json do repositório apenas com `name`.
  - [x] **T5.1** Injetar o perfil em main.rs — main.rs monta ContextProfile a partir de project_config::load_or_create + config::load_context, passa para App e o tracing::debug mostra o budget resolvido para o modelo/provider inicial.
  - [ ] **T5.2** Registrar o formato e a precedência — O formato `models: { "provider/modelo" | "modelo": { context_window } }` e a precedência (exato > só modelo > context.ron) ficam documentados junto do exemplo de configuração, e o ai.project.json do repositório continua válido para os testes existentes.
