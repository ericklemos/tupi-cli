<!-- Gerado automaticamente pelo tupi (somente leitura; fonte de verdade no banco local). -->

# Scaffold automático de docs/ com esqueleto de headers (`docs-scaffold`)

**Status:** closed

## Objetivo

No startup do binário, garantir `<root>/docs` com 5 arquivos markdown de esqueleto (H1 de propósito + H2 nomeados, sem corpo), na língua do campo `lang` de `ai.project.json`. Política de escrita: semeia arquivo ausente e mantém o esqueleto atualizado apenas enquanto o arquivo não ganhou conteúdo próprio; arquivo com corpo é autoral e fica intocado, porque documentação é viva. `lang` passa a ser campo obrigatório, auto-preenchido do locale do sistema. Tornar `docs` a pasta de documentação padrão quando `docs_folders` está ausente, mantendo `docs_folders` como substituto (nunca soma). Fontes: crates/tupi/src/main.rs:17-434, crates/tupi/src/project_config.rs:21-36, crates/tupi/src/project_config.rs:68-149, crates/tupi/src/project_config.rs:192-288, crates/tupi-tools/src/tools/docs/chunk.rs:35-47, crates/tupi-tools/src/tools/docs/index.rs:78-364, crates/tupi/src/agents_config.rs:5-39.

## Restrições

- Documentação é artefato vivo: o scaffold NUNCA apaga conteúdo escrito por humano. Só escreve arquivo ausente, ou arquivo cujo conteúdo ainda é exatamente o esqueleto gerado.
- Só os 5 caminhos gerenciados são escritos; nenhum outro arquivo é criado, movido ou apagado pelo scaffold.
- Sem heurística de idioma sobre texto do repo: a língua vem do campo `lang` de ai.project.json, que nunca pode faltar — ausente é preenchido na hora a partir do locale do sistema (LC_ALL/LANG/LANGUAGE) e gravado de volta no arquivo.
- Idiomas suportados: apenas pt-BR e en; locale do sistema sem template equivalente resolve para en.
- `lang` inválido é erro que derruba a configuração inteira, seguindo o precedente de dashboard_web_port (crates/tupi/src/project_config.rs:485-524).
- Gravar apenas quando os bytes diferem do arquivo existente, para não invalidar o índice de docs por mtime (crates/tupi-tools/src/tools/docs/index.rs:78-364).
- Não criar tool nova: o scaffold roda no startup, junto de load_or_create (crates/tupi/src/main.rs:17-434).
- Módulo novo em crates/tupi/src, seguindo o padrão de agents_config (crates/tupi/src/agents_config.rs:5-39); nada em tupi-tools.

## Nuances

- Política de escrita (b/c híbrida, decidida): (1) arquivo ausente -> grava o esqueleto; (2) arquivo presente cujo conteúdo é byte a byte o esqueleto da lingua atual -> não reescreve (idempotência) ou atualiza se o template mudou entre versões; (3) arquivo presente com qualquer linha além dos headings -> não toca, nem para 'melhorar'. A verificação do caso (3) é comparar com o esqueleto esperado, não heurística de markdown.
- Não existe rede de segurança para conteúdo apagado: o índice de docs guarda hash e chunks, não histórico (crates/tupi-tools/src/tools/docs/store.rs), nenhuma tool devolve versão anterior de arquivo, e o único socorro seria o git. Por isso a política padrão é nunca sobrescrever prosa.
- Distinção central do `lang`: AUSENTE não é erro — é detectado do locale do sistema e gravado de volta no arquivo, preservando os demais campos; PRESENTE E INVÁLIDO é erro que derruba a config, por decisão do usuário. Sem essa distinção, todo projeto anterior a esta mudança quebraria na primeira execução.
- O backfill tem precedente pronto: record_model_window (crates/tupi/src/project_config.rs:68-149) reescreve o JSON preservando campos desconhecidos e só grava quando falta a chave; a detecção de locale segue o mesmo shape, com precedência LC_ALL > LANG > LANGUAGE e normalização `pt_BR.UTF-8` -> `pt-BR`.
- Consequência aceita de `lang` inválido derrubar a config: load_or_create cai no fallback com tracing::warn (crates/tupi/src/project_config.rs:257-288) e o projeto perde name, models, docs_folders e dashboard_web_port de uma vez por causa de um campo cosmético. Isso só acontece por edição humana, já que o campo passa a ser auto-preenchido.
- `docs_folders` hoje cai vazio no fallback (crates/tupi/src/project_config.rs:281-288): o default `["docs"]` precisa entrar em fallback e no caso vazio de normalize_docs_folders (crates/tupi/src/project_config.rs:193-231), e isso altera o ai.project.json gerado e o teste a_missing_file_is_created_with_the_folder_name (crates/tupi/src/project_config.rs:321).
- O índice chunka por seção de heading e o breadcrumb do H1/H2 é o identificador do pedaço recuperado (crates/tupi-tools/src/tools/docs/chunk.rs:35-47): o esqueleto tem de nascer com H2 nomeados, não vazio de verdade.
- Documentar a lista de tools em docs/ duplicaria o `setup`, que já grava o bloco de tools no AGENTS.md e na skill (crates/tupi-tools/src/tools/setup/mod.rs:127-185) — o esqueleto não tem seção de tools.
- `parse_project` ignora chaves desconhecidas (crates/tupi/src/project_config.rs:192-246, teste unknown_fields_are_ignored em :457-461), então `lang` precisa de validação explícita e `docs_folders` explícito continua substituindo o default, nunca somando.

## Tasks

- [x] **t1** Campo `lang` obrigatório, auto-preenchido do sistema
  - Adicionar `lang` a ProjectConfig (crates/tupi/src/project_config.rs:21-36). Tipo `Lang` (PtBr, En) serializado como `pt-BR`/`en`; campo `lang: Option<Lang>` na leitura para permitir detectar ausência; `ProjectConfig::lang()` devolve o valor resolvido. parse_project (crates/tupi/src/project_config.rs:192-246): presente e inválido = ProjectConfigError; ausente = None, preenchido pela t1.4. Fontes: crates/tupi/src/project_config.rs:21-246, crates/tupi/src/project_config.rs:485-524.
  - [x] **t1.1** Tipo `Lang` e campo `lang` em ProjectConfig — parse_project devolve None para {"name":"x"}, Some(En) para {"name":"x","lang":"en"} e não quebra unknown_fields_are_ignored (crates/tupi/src/project_config.rs:457-461).
  - [x] **t1.2** Normalização e recusa de valor inválido — `pt-BR`, `pt_br`, `PT-br` viram PtBr; `en`, `EN`, `en_US` viram En; `fr` produz ProjectConfigError novo com caminho e valor na mensagem e load_or_create cai no fallback com warn.
  - [x] **t1.3** Detecção do locale do sistema — Função pura `Lang::from_locale` devolve PtBr para `pt_BR.UTF-8` e `pt-BR`, En para `en_US.UTF-8`, En para `fr_FR.UTF-8` (sem template) e En para env vazia; teste sem tocar em env global.
  - [x] **t1.4** Backfill: ausente é gravado de volta preservando os demais campos — load_or_create com arquivo existente sem `lang` grava o detectado preservando name, models, docs_folders e chaves desconhecidas; segunda execução não reescreve, seguindo o padrão de record_model_window (crates/tupi/src/project_config.rs:68-149).
- [x] **t2** Módulo docs_scaffold com os 5 templates
  - Novo módulo crates/tupi/src/docs_scaffold.rs, no formato de crates/tupi/src/agents_config.rs:5-39: `scaffold(root, lang)` escreve docs/index.md, docs/architecture.md, docs/business.md, docs/conventions.md e docs/decisions/0000-template.md. Cada arquivo: H1 de propósito em uma linha + H2 vazios. Não sobrescreve arquivo que já tenha conteúdo autoral.
  - [x] **t2.1** Templates pt-BR e en com os H2 definidos — index.md tem ## Arquitetura / ## Regras de negócio / ## Convenções (ou equivalentes en) linkando os outros três; architecture.md tem ## Camadas / ## Fronteiras / ## Componentes; business.md tem ## Entidades / ## Invariantes / ## Fluxos; conventions.md tem ## Onde cada coisa mora / ## Testes / ## Commits; decisions/0000-template.md tem ## Contexto / ## Decisão / ## Alternativas rejeitadas / ## Consequências. Nenhum H2 com corpo.
  - [x] **t2.2** Nunca apaga conteúdo: só escreve se ausente ou se o conteúdo ainda é o esqueleto — Arquivo com conteúdo autoral (linha além dos headings) permanece byte a byte igual após scaffold; arquivo ainda idêntico ao esqueleto não é reescrito (mtime intacto na segunda execução).
  - [x] **t2.3** Escopo estrito: nada além dos 5 caminhos — Teste cria docs/notas.md, docs/architecture.md com corpo autoral e docs/business.md ausente: notas.md e architecture.md intactos, business.md criado, nenhum arquivo novo fora dos 5 caminhos.
  - [x] **t2.4** Falha de I/O não derruba o startup — Erro de escrita em docs/ gera tracing::warn e o processo segue, como em load_or_create (crates/tupi/src/project_config.rs:257-279).
- [x] **t3** Wiring no startup
  - Chamar o scaffold em main logo após `project_config::load_or_create(&root)` e antes de montar tools/indexação (crates/tupi/src/main.rs:17-434), no mesmo root usado pelas tools.
  - [x] **t3.1** Chamada após load_or_create, antes da montagem do ToolRegistry — Ordem: load_or_create -> scaffold -> docs_folders/tools; teste de função extraída verifica que docs/index.md existe antes de o índice de docs ser construído.
  - [x] **t3.2** Declarar `mod docs_scaffold;` e logar o resultado em tracing::debug — cargo build passa e o tracing::debug mostra o lang resolvido e quantos arquivos foram escritos/preservados.
- [x] **t4** `docs` como pasta padrão de documentação
  - Quando `docs_folders` está ausente ou vazio, usar ["docs"]. Alterar fallback (crates/tupi/src/project_config.rs:281-288) e normalize_docs_folders (crates/tupi/src/project_config.rs:193-231); lista explícita continua substituindo o default.
  - [x] **t4.1** Default ["docs"] no fallback e no normalize vazio — ai.project.json criado do zero contém docs_folders: ["docs"] e lang preenchido; lista explícita ["wiki"] produz apenas wiki.
  - [x] **t4.2** Reconciliar testes existentes — a_missing_file_is_created_with_the_folder_name (crates/tupi/src/project_config.rs:321) passa a esperar ["docs"] e o lang do sistema; nenhum outro teste de project_config quebra.
- [ ] **t5** Verificação de ponta a ponta
  - Fechar o ciclo com tool, não apenas com teste unitário.
  - [ ] **t5.1** cargo test nos crates afetados — cargo test -p tupi e cargo test -p tupi-tools verdes.
  - [ ] **t5.2** docs status indexa a pasta criada — Depois de subir num projeto limpo, docs(action=status) reporta 5 arquivos e 0 skipped com reason missing_folder; map/query de docs devolve os H2 dos 5 arquivos.
