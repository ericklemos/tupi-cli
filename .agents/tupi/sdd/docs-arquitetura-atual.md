<!-- Gerado automaticamente pelo tupi (somente leitura; fonte de verdade no banco local). -->

# Documentação .md da arquitetura atual do Tupi (`docs-arquitetura-atual`)

**Status:** draft

## Objetivo

Produzir um conjunto de arquivos Markdown em `docs/arquitetura/` descrevendo a arquitetura ATUAL do workspace (10 crates, binário único `tupi`), derivada só de código lido na sessão e citando `caminho:linha`, sem proposta de mudança de desenho. Fonte: `Cargo.toml:1-13` (10 members, binário único), `crates/tupi-domain/src/lib.rs:1-8` e `:125-194`, `crates/tupi-kernel/src/lib.rs:1-78`, `crates/tupi-app/src/lib.rs:1-13`, `crates/tupi-tools/src/lib.rs:48-101,104-321,356-402`, `crates/tupi/src/project_config.rs:21-36,40-45,192-246`, `crates/tupi/src/main.rs:486-502,577-612`, `ai.project.json:1-11`.

## Restrições

- Somente leitura de código: nenhum crate é alterado por esta iniciativa, exceto a chave `docs_folder` em `ai.project.json:4` (decisão pendente do usuário).
- Documentação em português, no tom declarativo do README.md, sem repetir as descrições de tools já publicadas no README (README.md:10-40).
- Cada afirmação estrutural carrega `caminho:linha` verificável; nada entra por memória não checada.
- Nenhum arquivo .md em `docs/` hoje: `docs/` não existe no workspace (map(path=docs) = 0 arquivos).
- Não-objetivo: desenhar arquitetura-alvo, ADRs de futuro, ou refatorações; só o estado atual.
- SDD está em modo somente-leitura; a escrita dos .md só ocorre após este plano ser fechado.

## Nuances

- Achado de configuração: `ai.project.json:4` declara `"docs_folder"`, mas o campo serializado é `docs_folders` (`crates/tupi/src/project_config.rs:27-28`, sem alias serde) e campos desconhecidos são ignorados — logo `ProjectConfig::docs_folders` fica vazio e a tool `docs` não é registrada (`crates/tupi-tools/src/lib.rs:512-523`). Criar `docs/` sem corrigir a chave deixa a documentação invisível para a tool `docs`.
- Camadas: `tupi-domain` (Role/Message/Session/AgentMode/Event + context/memory/tokens/workflow, `crates/tupi-domain/src/lib.rs:1-8,16-122,125-194`), `tupi-kernel` (command/error/event/provider/query/registry/tool/workflow + pipeline/embed, `crates/tupi-kernel/src/lib.rs:1-9,77-78`), `tupi-app` (app/debug/memory/prompt/sdd/sdd_api/sdd_run/store/telemetry/workflow + pipeline privado, `crates/tupi-app/src/lib.rs:1-13`), `tupi-tools`, `tupi-providers`, `tupi-store` (redb), `tupi-tui` (ratatui), `tupi-web` (axum), `tupi-infer` e o binário `tupi` (`Cargo.toml:2-13`).
- Kernel é CQRS barramento: `CommandBus::dispatch_in(session, command)` (`crates/tupi-kernel/src/command.rs:107-124`) e `QueryBus::dispatch_in(session, query)` (`crates/tupi-kernel/src/query.rs:53-59`), com `CommandHandler` (`crates/tupi-kernel/src/command.rs:87-100`).
- Tools: `ToolsOptions` (`crates/tupi-tools/src/lib.rs:48-101`), `register`/`register_with` que monta o `ToolRegistry` e inicia o watcher de grafo (`:104-321`, `:322-342`), e exposição por modo `Exposure`/`register_exposed_with` (`:356-402`).
- Wiring do binário: `main` carrega `ai.project.json` (`crates/tupi/src/project_config.rs:257-279`), monta o prefixo de startup com workflows e seção de agentes (`crates/tupi/src/main.rs:486-502`, `crates/tupi/src/agents_config.rs:9`), resolve porta web (`crates/tupi/src/main.rs:606-612`) e CLI (`:577-602`).
- Restrição do AGENTS.md: o prompt não carrega o repositório; a doc de arquitetura precisa dizer onde a descoberta começa (`map`) e como o contexto é podado (`crates/tupi-app/src/context.rs`), para não virar peso permanente de prompt.

## Tasks

