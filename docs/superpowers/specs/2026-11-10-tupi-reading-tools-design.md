# Tupi — tool-call de leitura

Data: 2026-11-10
Status: proposta para aprovação

## Intenção

O Tupi hoje é uma conversa: o modelo recebe o histórico e devolve texto. A
próxima capacidade é o modelo **pedir para ler** arquivos do projeto, receber o
trecho e continuar a resposta com base nele. É o primeiro passo de agente do
Tupi, e o recorte é deliberadamente só leitura: `read`, `read_many` e `peek`.

O desenho vem do `brain_tools`, portado para dentro do Tupi: esqueleto por AST,
corpo de símbolo sob demanda, orçamento por seção, truncamento recuperável e
memória do que já foi entregue na sessão. O objetivo não é economizar tokens por
si só; é entregar ao modelo o trecho certo, não o arquivo inteiro.

## Objetivos

- O modelo consegue ler um arquivo (esqueleto ou símbolo) e ler vários de uma vez.
- O loop de tool-call funciona de ponta a ponta: stream → tool_call → execução →
  mensagem `tool` → nova chamada ao modelo, até o modelo parar.
- Toda resposta de leitura respeita um orçamento de tokens; o excedente fica
  recuperável por `peek`, nunca desaparece em silêncio.
- Na segunda leitura do mesmo símbolo na mesma sessão, o Tupi não reenvia o corpo:
  aponta para o que já foi entregue ou devolve só o diff.
- A leitura é presa ao diretório onde o Tupi foi aberto; nada fora dele é lido.
- A TUI mostra, de forma compacta, qual tool foi chamada e se deu certo.

## Não-objetivos

- `brain_search`, `brain_map`, grafo de chamadas e impacto: próximo passo.
- Edição, escrita, patch, move: o Tupi continua somente leitura nesta versão.
- Cliente MCP ou reutilização dos crates do `brain_tools` como dependência.
- Kotlin, Python, TypeScript e outras gramáticas: só Rust nesta versão.
- Leitura por revisão (`rev:path`) e qualquer integração com git.
- Execução de comandos, shell ou acesso à rede pelas tools.
- Persistir sessão ou tool calls no `tupi-store`.

## Anti-objetivos

Falha mesmo se funcionar tecnicamente:

- despejar arquivo inteiro por padrão quando existe AST;
- devolver saída vazia ou truncada sem envelope e sem handle;
- cair em leitura ampla quando o símbolo pedido não existe;
- deixar o loop de tools rodar sem limite;
- exibir o corpo lido inteiro na TUI, competindo com a resposta do modelo;
- deixar a tool lançar panic ou derrubar a TUI por argumento inválido.

## Restrições

- Workspace Rust, edição 2021, microkernel + CQRS: contratos no `tupi-kernel`,
  implementações nos crates, TUI só reage a eventos.
- A DeepSeek é OpenAI-compatível: `tools`, `tool_calls` e `role: "tool"` no
  corpo, com `tool_calls` fragmentados no stream SSE.
- `Message` é o tipo de histórico; `ChatRequest` é o contrato do provider.
- O orçamento é estimado, não exato: 3,3 caracteres por token, como no
  `brain_tools`.
- O processo do Tupi é uma sessão: o ledger de entregas pode ser em memória e
  com escopo de processo.
- O projeto `tupi-cli` não é um repositório git; este documento e as mudanças
  ficam na árvore de trabalho.

## Abordagens consideradas

1. **Depender dos crates do `brain_tools`.** Menos código, mas cria dependência
   de caminho entre dois repositórios, carrega tipos de MCP sem uso e acopla a
   evolução dos dois projetos. Descartada.
2. **Portar o miolo para `crates/tupi-tools`.** O Tupi fica dono do AST, do
   orçamento, do store, do ledger e das tools; sem acoplar repositórios. Mais
   código, mas a lógica portada já tem desenho e testes comprovados. **Escolhida.**
3. **Tupi como cliente MCP do `brain_tools`.** Chega ao poder total mais rápido,
   mas transforma o produto em host MCP, com configuração, processo externo e
   display genérico. Descartada: o objetivo é trazer a leitura para dentro.

## Fluxo de uma mensagem

1. A TUI envia `SendMessage`.
2. O `App` monta `ChatRequest` com histórico e as specs das três tools.
3. O provider faz o stream. Texto vira `Event::Token`; fragmentos de
   `tool_calls` são acumulados por índice.
4. Ao fim do stream, o provider entrega `ProviderEvent::ToolCalls`.
5. Se não há tool calls, o turno termina como hoje.
6. Se há, o `App` grava a mensagem do assistant (texto + tool_calls), executa
   cada chamada em ordem pelo `ToolRegistry`, publica `ToolStarted` e
   `ToolFinished`, e grava uma mensagem `Role::Tool` por resultado.
7. O `App` volta ao passo 2 com o histórico ampliado, até o modelo parar ou
   atingir o limite de passos.
8. `Esc` cancela o stream atual. Resultados de tools já gravados permanecem no
   histórico; nenhuma execução nova começa depois do cancelamento.

Invariante de protocolo: toda `tool_call` do assistant precisa de exatamente
uma mensagem `Role::Tool` correspondente antes da próxima requisição ao
provider. Se o turno for cancelado ou o limite de passos for atingido com calls
sem resposta, o `App` sintetiza uma mensagem `tool` para cada `call_id` pendente
(`cancelado pelo usuário` / `limite de passos atingido: chamada não executada`).
Sem isso, a próxima mensagem do usuário pode ser recusada pelo provider.

Limite de passos: 8 idas ao provider por mensagem do usuário. Ao estourar, o
turno termina com `StreamFailed` explicando o limite, sem loop silencioso.

## Mudanças por camada

### `tupi-domain`

- `Role::Tool`.
- `ToolCall { id: String, name: String, arguments: String }` — `arguments` é o
  JSON cru recebido do modelo, como string.
- `Message` ganha `tool_calls: Vec<ToolCall>` e `tool_call_id: Option<String>`.
  Construtores (`Message::user`, `Message::assistant`, `Message::tool`) evitam
  atualizar todos os literais de teste.
- `Event` ganha `ToolStarted` e `ToolFinished` (request_id, call_id, nome,
  resumo curto e sucesso/falha).

### `tupi-kernel`

- `ToolSpec { name, description, schema: serde_json::Value }` e
  `ChatRequest.tools: Vec<ToolSpec>`.
- `ProviderEvent::ToolCalls(Vec<ToolCall>)`, emitido uma vez, com os argumentos
  já completos.
- `Tool` trait e `ToolRegistry` em `tool.rs`:
  `fn spec(&self) -> ToolSpec` e
  `fn call(&self, arguments: serde_json::Value) -> Result<String, ToolError>`.
  As tools de leitura são síncronas; o trait não precisa de `async_trait`.
- `ToolError` explícito: argumento inválido, caminho fora do root, arquivo
  ausente, handle expirado, símbolo não encontrado.

### `tupi-tools` (novo crate)

- `read`: esqueleto ou corpo de símbolo, com hash blake3 no cabeçalho.
- `read_many`: lote atômico, um orçamento compartilhado.
- `peek`: paginação do corpo omitido por handle.
- `ast`: tree-sitter Rust (`tree-sitter`, `tree-sitter-rust`), escopos
  `function_item`, `impl_item`, `struct_item`, `enum_item`, `trait_item` e
  `mod_item`; extrai nome e assinatura; `compact_signature` colapsa assinatura
  multiline em uma linha.
- `span`: o fim do corpo é derivado do texto verbatim do nó
  (`start_line + code.lines().count() - 1`), não de `end_position`, evitando o
  erro de uma linha do tree-sitter.
- `budget`: `Section { heading, body, priority }`, estimativa a 3,3
  caracteres/token, seleção por prioridade, split de seção grande em
  `parte i/n` reabrindo o fence, envelope de truncamento.
- `store`: `ResponseStore` em memória, com TTL e teto de bytes, handles
  `h_xxxxxxxx`, evicção LRU.
- `ledger`: `DeliveryLedger` com escopo de processo; `Fresh`, `Seen`, `Changed`,
  `Full`; até 2 omissões antes de reenviar o corpo; diff com 1 linha de contexto
  e teto de 600 linhas. Chave = caminho + nome qualificado, não o span.
- `rev` fica fora: não há leitura por revisão nesta versão.

Dependências novas: `tree-sitter`, `tree-sitter-rust`, `blake3`, `serde`,
`serde_json`, `thiserror`.

### `tupi-providers` (DeepSeek)

- `build_body` inclui `tools` quando `ChatRequest.tools` não está vazio, no
  formato `{ type: "function", function: { name, description, parameters } }`.
- Serialização de mensagens: assistant com `tool_calls`; `role: "tool"` com
  `tool_call_id` e `content`.
- O parser SSE passa a ler `choices[0].delta.tool_calls[]`, acumulando por
  `index` (`id`, `function.name`, `function.arguments`), e `finish_reason`.
- Texto e tool calls podem vir no mesmo turno; o texto é emitido incremental e
  as calls completas ao final.
- Argumentos inválidos não são erro de protocolo: a string crua chega ao `App`,
  que tenta o parse. Falha de parse vira mensagem `tool` de erro explícito, sem
  invocar a tool.

### `tupi-app`

- `send_message` passa a rodar o loop de passos dentro da task já existente.
- Histórico: assistant (texto + calls), depois uma mensagem `tool` por chamada.
  Se o assistant não produziu texto nem calls, nada é gravado.
- Cancelamento continua abortando a task; o loop verifica `active` antes de
  executar a próxima tool ou pedir outro stream.
- Tools são injetadas no `App` via `ToolRegistry`; o `App` não conhece a
  implementação de leitura.

### `tupi-tui`

- `ChatState` deixa de guardar só `(Role, String)` e passa a ter
  `ChatEntry`: mensagem normal (bolha atual) ou linha de tool.
- Linha de tool: `⚙ read src/app.rs · new` em cinza, com `✓`/`✗` no fim.
  Nunca exibe o corpo lido.
- `Event::ToolStarted` cria a linha; `ToolFinished` a atualiza.
- O resto do layout, sugestões, modal de modelos e status não muda.

### `tupi` (binário)

- Descobre o root com `std::env::current_dir()` no start, canonicaliza e
  registra as tools antes de construir o `App`.
- Se o root falhar, o programa sai com erro claro antes de abrir a TUI.

## Contrato das tools

### `read`

Entrada: `path` (obrigatório, relativo ao root), `symbol` (opcional),
`budget_tokens` (opcional), `repeat` (`"allow"` opcional).

Saída sem `symbol` (Rust com AST): cabeçalho
``### `path` — N linhas · blake3 `hash`` e uma linha por símbolo,
``- `assinatura` (:início-fim)``. Sem corpos.

Saída com `symbol`: cabeçalho
``### `nome` (`path:início-fim`) · blake3 `hash`` e o corpo verbatim em fence.
Símbolo inexistente é erro explícito, nunca resposta vazia.

Arquivo sem gramática suportada (ou Rust sem símbolos): uma seção de arquivo
inteiro, sujeita ao mesmo orçamento e ao mesmo split/peek. É a exceção
explícita, limitada pelo orçamento.

Semântica de símbolo: o esqueleto lista os escopos encontrados no arquivo —
`impl Foo` aparece como `impl Foo`, e seus métodos aparecem como itens próprios
com a assinatura. `symbol` é o nome simples; se houver mais de um escopo com o
mesmo nome (dois `new`, por exemplo), todos os corpos correspondentes são
devolvidos, cada um com seu cabeçalho. Não há erro de ambiguidade. A identidade
usada no ledger é qualificada pelo container quando existe (`Foo::new`), para
que dois métodos de mesmo nome não colidam.

Segunda leitura do mesmo símbolo, arquivo inalterado: ponteiro
`Já entregue nesta sessão` com handle vivo. Arquivo alterado dentro do símbolo:
só o diff. `repeat: "allow"` força o corpo e registra a entrega de novo.

### `read_many`

Entrada: `reads: [{ path, symbol? }]`, `budget_tokens`, `repeat`.
Valida todas as leituras antes de responder; uma inválida derruba o lote
inteiro com a lista de erros e nada é devolvido. Leitura repetida no mesmo lote
é recusada. Um único orçamento é dividido entre os itens.

### `peek`

Entrada: `handle`, `offset_chars`, `budget_tokens`.
Handle desconhecido ou expirado é erro explícito. O fim do corpo responde
`Fim do corpo.`.

## Decisões tomadas pelo autor

- **Root = diretório atual na abertura.** Sem configuração nesta versão; um
  `TUPI_TOOL_ROOT` pode vir depois. Simples, previsível e contido.
- **Orçamento padrão de 2000 tokens**, constante no crate; sem variável de
  ambiente nesta versão.
- **Rust apenas.** É a linguagem do Tupi e cobre o uso imediato; outras
  gramáticas entram quando houver demanda, uma por vez.
- **Ledger em memória, com escopo de processo.** Um processo do Tupi é uma
  sessão; não há persistência de tool calls nesta versão.
- **Sem `rev`.** Leitura de git não é leitura do projeto aberto e adiciona
  subprocesso; fica para o passo em que git entrar no Tupi.
- **Execução sequencial das tool calls.** O modelo pode pedir várias; executar
  em ordem mantém o histórico determinístico e o display legível. Paralelismo
  fica para depois.
- **A TUI não mostra o corpo lido.** O consumidor do corpo é o modelo; o
  usuário vê qual leitura aconteceu.
- **Toda `tool_call` pendente recebe uma resposta sintética no cancelamento ou
  no limite de passos.** O protocolo OpenAI exige o par call/result; sintetizar
  a mensagem é mais simples e mais seguro que adiar o commit do assistant.
- **O parse dos argumentos é do `App`, não da tool.** O trait recebe
  `serde_json::Value`; JSON inválido vira erro de tool sem execução, com o
  mesmo caminho para todas as tools.
- **Nome simples, todos os corpos em caso de repetição.** É o que o modelo
  escreve naturalmente; qualificar o ledger evita colisão sem exigir que o
  modelo saiba o container.

## O que fica para o builder

- Nomes exatos de módulos, tipos auxiliares e organização interna do
  `tupi-tools`.
- Estratégia de testes de cada módulo portado (os testes originais do
  `brain_tools` são referência, não obrigação).
- Como o `ChatEntry` da TUI é desenhado em detalhe (ícones, cores, indentação).
- Redação final das mensagens de erro e dos textos das tools.
- Ordem de implementação e granularidade dos commits, se houver.

## Verificação

- `cargo test --workspace`
- `cargo clippy --workspace --all-targets`
- Teste de integração com provider falso: pedido de `read` → tool executa →
  segunda chamada recebe a mensagem `tool` → resposta final.
- Teste do loop: várias tool calls no mesmo turno; limite de passos; cancelamento
  no meio; toda call pendente recebe uma mensagem `tool` sintética.
- Teste de argumento malformado: nenhuma tool é invocada e o erro volta como
  mensagem `tool`.
- Testes de barreira: caminho fora do root, símbolo inexistente, arquivo ausente,
  handle expirado, segunda leitura vira ponteiro, arquivo alterado vira diff,
  lote com um item inválido não devolve nada.
- Teste do parser SSE com `tool_calls` partidos no meio de um argumento.
- Teste da TUI: linha de tool aparece e atualiza sem quebrar as bolhas.
