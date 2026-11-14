<!-- Gerado automaticamente pelo tupi (somente leitura; fonte de verdade no banco local). -->

# Auto-preenchimento de context_window por modelo no ai.project.json (`context-window-autofill`)

**Status:** closed

## Objetivo

Toda vez que um modelo for selecionado ou usado (startup com modelo restaurado + `Command::SetModel`), garantir que `ai.project.json` tenha a entrada `models["provider/modelo"].context_window` preenchida com o valor que o sistema já usa hoje — o `budget` default de `configs/context.ron` (`crates/tupi/src/config.rs:114`). A entrada fica permanente no arquivo, editável pelo usuário, sem alterar a precedência de resolução existente (`provider/modelo` → `modelo` → default em `crates/tupi-app/src/app.rs:242`).

## Restrições

- Somente `ai.project.json` do root do projeto; nada em `configs/context.ron` (default global) e nada no redb.
- Zero custo de prompt: nenhum texto novo entra no prefixo, nas tools ou no system prompt — só I/O de config em disco.
- A escrita nunca derruba a app nem o turno: erro de escrita/parse vira `tracing::warn!` e a app segue com o perfil em memória.
- Nunca reescrever o arquivo por serialização do `ProjectConfig`: o parse ignora campos desconhecidos e a reserialização os apagaria. Escrita é merge sobre `serde_json::Value`.
- DECIDIDO: gravar apenas quando a chave `provider/modelo` não existir, ou existir sem `context_window`. Valor já preenchido pelo usuário nunca é sobrescrito (edição manual é intenção preservada).
- Não inventar janela: a única fonte é o budget default já carregado em `main`; sem catálogo de modelos, sem chamada extra ao provider, sem estender a trait `Provider`.
- Se o default for 0 (poda desligada), não criar entrada — `context_window: 0` é ignorado pela resolução e só polui o arquivo.
- Nada de escrita automática em outras chaves (`name`, `docs_folders`, `dashboard_web_port`).

## Nuances

- Chave sempre `provider/modelo` (decisão do usuário), mesmo quando o modelo foi trocado sem prefixo: `set_model` já resolve o provider ativo em `crates/tupi-app/src/app.rs:640`.
- O valor gravado é `context_profile.resolve(provider, model).budget`; com a chave ausente isso equivale ao base de `configs/context.ron`, então a entrada nasce sem mudar comportamento nenhum — é ponto de partida editável.
- Consequência da escolha (a): um valor manual editado no arquivo sobrevive a todas as trocas de modelo; o auto-preenchimento nunca é fonte de verdade, só de completude.
- Arquivo JSON inválido: `load_or_create` hoje cai em `fallback` sem sobrescrever (teste `an_invalid_file_falls_back_without_overwriting`, crates/tupi/src/project_config.rs:239). A gravação automática segue a mesma regra: erro de parse = não escreve.
- Ordem no startup: gravar depois de ler `project` e resolver `resolved_context` (main, bloco `let context_profile = { ... }`), para que leitura e escrita não compitam no mesmo boot.
- Ponto único de troca: `Command::SetModel` → `App::set_model`. A TUI (`/modelos`) e o dashboard web passam pelo `CommandBus`; confirmar em t5 que ninguém troca `self.model` por fora.
- Custo em disco: uma reescrita pretty por troca de modelo onde falta a entrada; frequência humana, arquivo pequeno. Nenhum custo por turno.
- Ruído no git: cada modelo novo usado gera uma linha de diff no `ai.project.json` versionado — efeito aceito, é o comportamento pedido (permanente).

## Tasks

- [x] **t1** Escritor de merge no project_config
  - Nova função `record_model_window(root, key, context_window) -> Result<bool, ProjectConfigError>` em `crates/tupi/src/project_config.rs`, ao lado de `write_project` (:191): lê o texto cru de `project_path(root)`, faz `serde_json::from_str::<serde_json::Value>`, garante `models` como objeto, insere `models[key].context_window = n` só quando a chave ainda não existe (ou existe sem `context_window`), reescreve pretty + newline. `Ok(false)` quando não escreveu (entrada já presente ou `n == 0`). Erro de parse ou I/O volta como `ProjectConfigError` sem tocar no arquivo.
  - [x] **t1.1** Teste: chave ausente grava a entrada com o default — Arquivo ausente → cria `{name, models: {key: {context_window: n}}}` e o resultado é lido por `read_project`.
  - [x] **t1.2** Teste: campos desconhecidos e outras entradas sobrevivem — Arquivo com `"futuro": 1` na raiz, `docs_folders` e outra entrada em `models` continua com todos eles, mais a nova entrada.
  - [x] **t1.3** Teste: arquivo inválido não é reescrito — JSON inválido → `Err`, e o conteúdo em disco permanece byte a byte o original.
  - [x] **t1.4** Teste: entrada existente e default zero não são tocados — Valor já presente (qualquer n) → `Ok(false)` e conteúdo intacto; `n == 0` → `Ok(false)` e conteúdo intacto.
- [~] **t2** Chave provider/modelo em um único lugar
  - Auxiliar `model_key(provider, model)` perto de `project_path` monta `format!("{provider}/{model}")`, usada pela gravação e pelos testes. Provider ou modelo vazio → `Ok(false)`/`Err` em vez de gravar `"/modelo"`.
  - [ ] **t2.1** Teste: chave gravada é provider/modelo — Após `Command::SetModel { model: "prov_b/model_2" }` o arquivo tem a chave `prov_b/model_2`, não `model_2`.
  - [x] **t2.2** Teste: entrada vazia é recusada — Chamada com provider ou modelo vazio não produz entrada no arquivo.
- [ ] **t3** Hook de gravação no App::set_model
  - Novo campo em `App` (`crates/tupi-app/src/app.rs:369`): `Option<Arc<dyn Fn(&str, &str) + Send + Sync>>`, com builder `with_model_recorder` no padrão de `with_context_profile` (:575). Em `set_model` (:640), depois de `save_selected_model` e de `apply_context`, chamar o recorder com provider/modelo resolvidos; o log de erro fica no closure. Sem recorder = no-op, para os construtores de teste atuais não tocarem em disco.
  - [ ] **t3.1** Teste: set_model dispara o recorder — Recorder de teste (contador em `Mutex`) recebe exatamente `(provider, model)` após `Command::SetModel`, inclusive com app sem `store`.
  - [ ] **t3.2** Teste: app sem recorder não quebra — App construído sem `with_model_recorder` responde `ModelChanged` normalmente.
- [ ] **t4** Ligação no binário: boot + recorder
  - Em `crates/tupi/src/main.rs`, após o bloco que monta `context_profile` (parte 2/3 do `main`): chamar `project_config::record_model_window(&root, &model_key(&provider, &model), resolved_context.budget)` para o modelo restaurado no boot, e ligar `.with_model_recorder(...)` no builder do `App` com closure que fecha sobre `root` e o budget base, logando `tracing::warn!` em erro. O boot grava depois de resolver `resolved_context`, então não regrava o que acabou de ler.
  - [ ] **t4.1** Teste: startup grava a entrada do modelo restaurado — Com root temporário, simular o boot com `provider/model` grava a chave com o budget de `configs/context.ron`.
  - [ ] **t4.2** Teste: falha de escrita não derruba o boot — Arquivo somente-leitura → boot segue, prompt/telemetria inalterados, log com o aviso.
- [ ] **t5** Fechar rotas paralelas de troca de modelo
  - Procurar em `crates/tupi-tui/src` e `crates/tupi-web/src` caminho que mude modelo/provider sem `Command::SetModel` (ex.: `/modelos` mexendo em estado local, handler HTTP de settings). Se existir, rotear pelo `CommandBus` ou chamar o recorder; se não existir, registrar a conclusão na spec e não inventar hook novo.
  - [ ] **t5.1** Varredura: nenhum caminho paralelo de troca de modelo — Nenhum caminho de troca fora de `App::set_model`; arquivos verificados anotados nas nuances da spec.
