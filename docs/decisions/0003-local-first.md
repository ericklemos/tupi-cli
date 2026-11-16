# ADR 0003: local-first — o mecânico roda local, a API paga só o raciocínio

## Contexto

O Tupi nasceu com um único caminho de modelo: o turno principal, sempre remoto,
com `provider`/`model` vindo do ambiente e da seleção salva
(`Config::from_env`, `crates/tupi/src/config.rs:27-59`). Isso mistura no mesmo
provedor três tarefas que não pedem raciocínio — vetorizar documento para busca,
resumir um arquivo do índice e transcrever áudio — e cada uma delas era paga em
rede, token e latência por arquivo processado.

O que já está implementado hoje são exatamente três slots locais, e nada além:

- Embedder `LiquidAI/LFM2.5-Embedding-350M-GGUF` e gerador
  `LiquidAI/LFM3-1.7B-GGUF`, com repositório, arquivo, SHA-256 e tamanho no
  binário (`crates/tupi-infer/src/lib.rs:23-32`).
- Carregamento opcional: `load_embedder` e `load_generator` devolvem `Option`
  (`crates/tupi/src/local_models.rs:14-22`, `:26-32`, `:36-44`, `:48-54`), e o
  consumidor recebe `ToolsOptions.embedder`/`.generator`
  (`crates/tupi-tools/src/lib.rs:52-79`).
- Ligação decidida pelo binário, nunca pelo modelo:
  `local_embedding_enabled`, `docs_embedding_enabled` e `summaries_enabled`
  (`crates/tupi/src/config.rs:311-331`) combinam a feature `local-models`, a
  existência de pasta de documentação (`docs_folders`, `ai.project.json:5`) ou
  `space`, e o desligamento explícito (`off`/`0`/`false`); `should_download`
  (`:304-308`) separa quem baixa de quem só carrega.
- Resumo é do indexador de `docs`
  (`DocsIndex::process_one_summary`, `crates/tupi-tools/src/tools/docs/index.rs:523-584`),
  que grava a linha saneada no índice e descarta o resumo se o arquivo mudou.
- Transcrição roda no processo local, com `whisper` (`small` por padrão,
  `turbo` com CUDA e GPU disponível — `crates/tupi-transcribe/src/model.rs:13-16`,
  `default_model`:24-30) e o subcomando responde antes de abrir redb e provedor
  (`crates/tupi/src/main.rs:81-85`).

Rota remota existe em dois pontos: o turno principal
(`crates/tupi/src/config.rs:27-59`) e o executor do SDD, que carrega `provider`
e `model` e monta o `SddExecRequest` (`AgentExecutor`,
`crates/tupi/src/main.rs:28-76`; publicado em `ToolsOptions.sdd_executor`).
**Não existe roteador local↔API**: nada escolhe modelo pela dificuldade da
pergunta, nem no turno nem nas tools.

## Decisão

1. **Tarefa mecânica roda no processo local**, com contrato estreito: vetorizar
   trecho de documentação/espaço, resumir documento do índice e transcrever
   áudio. Cada uma recebe um modelo pequeno fixado no binário, com SHA-256 e
   tamanho verificados (`crates/tupi-infer/src/lib.rs:23-32`,
   `crates/tupi-transcribe/src/model.rs:18-64`).
2. **Local é slot `Option`, com piso explícito**: sem a feature `local-models`,
   sem o arquivo do modelo ou com o download desligado, a busca continua
   lexical e o resumo simplesmente não existe. Nunca há queda silenciosa para
   a API — degradar é visível no resultado, trocar de rota não é permitido.
3. **Quem baixa e quem carrega é o binário, a partir da configuração do
   projeto** (`crates/tupi/src/config.rs:304-331`): pasta declarada em
   `docs_folders`, `space` ligado, `off`/`0`/`false` como desligamento. O modelo
   não pede download, não liga RAG e não escolhe rota: nenhuma tool expõe isso.
4. **API só onde há raciocínio**: o turno principal e o executor do SDD são as
   duas únicas rotas remotas. Capacidade nova que exija raciocínio usa o turno
   principal; nenhuma tool mecânica abre provider próprio.
5. **A lacuna é declarada, não prometida.** Enquanto não houver roteador, a
   documentação (`README.md`, `docs/`, `AGENTS.md`) diz que o turno principal é
   API e que o local cobre apenas o mecânico.
6. **Rodar local não afrouxa controle**: valem o root confinado
   (`crates/tupi-tools/src/support/paths.rs:53-101`), a allowlist do `shell`
   (`crates/tupi-tools/src/tools/shell/mod.rs:61-157`) e os mesmos orçamentos
   (`max_tool_steps`, `crates/tupi/src/tool_config.rs:16-25`).

## Alternativas rejeitadas

- **Roteador local↔API que escolhe pela dificuldade da pergunta.** Rejeitada
  por não existir e por não ser barato criar: decidir "difícil" exige outro
  modelo julgando, o que soma latência e variância a todo turno, e transforma
  uma decisão de configuração numa heurística invisível. Prometer roteamento
  hoje é documentar o que o código não faz.
- **Turno principal inteiramente local (LFM3-1.7B).** Rejeitada no estado
  atual: o projeto configura janela de 200k tokens (`ai.project.json:8-10`) e o
  trabalho do turno é justamente o raciocínio que a divisão reserva à API.
  Direção futura, não implementada — e não pode ser escrita como se fosse.
- **Tudo na API, removendo os slots locais.** Rejeitada: embedding e resumo são
  por arquivo, crescendo com a documentação, e transcrição de áudio longo é
  cara em token. São tarefas determinísticas, com prompt e saída fixos, em que
  o modelo pequeno basta.
- **Download e carga sob demanda do modelo, pedidos pelo agente.** Rejeitada:
  liberaria rede e disco a pedido do modelo, contra o princípio de que a escolha
  local × API é configuração do projeto. Hoje o gatilho é `should_download`
  (`crates/tupi/src/config.rs:304-308`), não uma tool.
- **Serviço/daemon local separado para inferência.** Rejeitada: um processo a
  mais, protocolo próprio e inventário de porta para o mesmo resultado que o
  slot in-process entrega; o custo real (carregar o GGUF) não muda.
- **Escolher modelo pelo tamanho do arquivo ou do áudio.** Rejeitada: heurística
  implícita vira comportamento que ninguém vê. O que se quer é degradação
  declarada — o piso lexical ou a ausência de resumo aparecem no resultado.

## Consequências

- Primeiro uso tem custo de disco e rede (três modelos baixados sob demanda); o
  desligamento é explícito por ambiente (`off`/`0`/`false`) e o binário separa
  baixar de carregar (`should_download`, `crates/tupi/src/config.rs:304-308`).
- `docs` e `space` pagam indexação em tempo de CPU para economizar token
  depois; o índice guarda o hash do arquivo e descarta resumo obsoleto.
- Degradação precisa ser legível: busca lexical quando não há embedder, ausência
  de resumo quando não há gerador. Tool que prometer resumo sem gerador mente.
- `tupi transcribe` roda sem chave de API e sem redb, porque responde no `main`
  antes da pilha da TUI (`crates/tupi/src/main.rs:81-85`).
- A documentação do tema fica em [`docs/philosophy.md`](../philosophy.md) (P1 e
  P2), [`docs/rules/local-first.md`](../rules/local-first.md) e na linha "hoje ×
  direção" do philosophy; qualquer texto novo sobre local-first precisa manter a
  nota de que **não existe roteador no turno principal**.
- Caminho de evolução aceito: quando houver roteador, ele nasce como decisão de
  configuração do binário e ganha ADR próprio — não como heurística dentro da
  tool nem como pedido do modelo.
