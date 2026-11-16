# Local-first: o mecânico roda local, a API paga só o raciocínio

**Context:** O local cobre hoje três tarefas e só elas: embedding da busca de
documentação/espaço, resumo de documento e transcrição. Quem liga é o binário,
nunca o modelo — `local_embedding_enabled`, `docs_embedding_enabled` e
`summaries_enabled` (`crates/tupi/src/config.rs:311-331`) combinam a feature
`local-models`, as pastas de `docs_folders`/`space` e o desligamento explícito
(`off`/`0`/`false`), e `should_download` (`:304-308`) separa quem baixa de quem
só carrega. O modelo entra como slot opcional: `load_embedder` e `load_generator`
devolvem `Option` (`crates/tupi/src/local_models.rs:14-22`, `:36-44`), o
consumidor recebe `ToolsOptions.embedder`/`.generator`
(`crates/tupi-tools/src/lib.rs:52-79`) e a busca lexical é o piso quando não há
modelo; o resumo é do indexador de `docs` (`process_one_summary`,
`crates/tupi-tools/src/tools/docs/index.rs:523-584`). `tupi transcribe` responde
antes de abrir redb e provedor, com inferência no processo local
(`crates/tupi-transcribe/src/engine.rs:123-131`). Rota remota existe em dois
lugares: o turno principal, cujo `provider`/`model` vem do ambiente e da seleção
salva (`crates/tupi/src/config.rs:27-59`), e o executor do SDD (`AgentExecutor`,
`crates/tupi/src/main.rs:22-71`), publicado em `ToolsOptions.sdd_executor`.
**Não existe roteador local↔API**: nada escolhe modelo pela dificuldade da
pergunta, então texto que prometa isso mente.
**Gatilho:** ao decidir onde uma tarefa roda, ao criar capacidade que chama
modelo (tool, subcomando, workflow) e ao descrever o tema em `README.md`,
`docs/` ou `AGENTS.md`.
**Exceção:** o turno principal e o executor do SDD PODEM falar com provedor
remoto — é o desenho atual, e enquanto não houver roteador a API é o caminho
deles. Nenhuma outra rota herda essa licença: tarefa mecânica que já tem slot
local não pode cair na API por conveniência.
**Rule:** Tarefa mecânica DEVE rodar no processo local por slot `Option` e
degradar para o piso (busca lexical, sem resumo, aviso de modelo ausente) quando
a feature ou o arquivo do modelo não existem. A escolha local × API é de
configuração do projeto/binário, NUNCA do modelo: nada de baixar modelo, ligar
RAG ou trocar de rota a pedido do agente. Capacidade nova que exija raciocínio
usa o turno principal — não abre provider próprio. Documentação DEVE declarar a
lacuna (sem roteador no turno) em vez de prometer local-first que o código não
faz. Rodar local não afrouxa controle: valem o root confinado
(`crates/tupi-tools/src/support/paths.rs:53-101`), a allowlist do `shell`
(`crates/tupi-tools/src/tools/shell/mod.rs:61-157`) e os mesmos orçamentos
(`max_tool_steps`, `crates/tupi/src/tool_config.rs:16-25`).
**Example (Do):** resumo de documento em `LFM3-1.7B-GGUF` dentro do indexador de
`docs`, busca vetorial que volta a lexical sem embedder, e a nota de que o turno
principal continua API porque não há roteador (`docs/philosophy.md`).
**Example (Don't):** acionar `tupi-infer`/provedor remoto dentro de tool
mecânica quando o slot está `None`; aceitar "use o modelo grande" vindo do
modelo; ou escrever no `README.md` que o Tupi escolhe local ou API conforme a
dificuldade — o roteador não existe.
