# Convenções

## Onde cada coisa mora

- Efeito colateral só na composição: o binário (`crates/tupi/src/main.rs`) monta
  store, provider, tools e TUI; os crates de baixo não conhecem o todo.
- Feature que escreve em arquivo do usuário vive em módulo próprio, com as
  decisões isoladas em funções puras e a escrita num único ponto
  (ex.: `crates/tupi/src/install.rs` — `plan` decide, `apply` escreve).
- Subcomando de manutenção é despachado no `main` **antes** de env/redb/provider,
  para rodar sem chave de API e sem subir a TUI.
- Texto novo no transcript da TUI nasce dentro de `transcript_layout_at`
  (`crates/tupi-tui/src/run.rs`), como `Line` + `owners.push(...)`: row desenhada
  fora quebra o hit-test de seleção e a cópia por arraste
  (ver [`features/tui-tool-usage.md`](features/tui-tool-usage.md)).
- Tempo de execução é medido por quem executa o efeito, não por quem consome o
  evento: o dado atravessa bus assíncrono e chegaria tarde. Na TUI, o relógio de
  uma tool nasce no estado (`push_tool_started`), nunca no render.
- Documentação de projeto e regra vivem sob `docs/` (ver
  [knowledge-placement](rules/knowledge-placement.md)).

## Escrita: endereço, guarda e verificação

Escrita segue o mesmo invariante da leitura: endereço barato, guarda que recusa
e verificação que não depende de parser do tupi. A tool é `edit`; as três rotas
não se misturam (`EditParams::address`,
`crates/tupi-tools/src/tools/edit/params.rs:63-163`).

- **`old`+`new` é o caminho normal em qualquer arquivo de texto.** `old` é o
  trecho copiado byte a byte de `read`/`lookup lines` (indentação e quebras de
  linha incluídas) e precisa casar **exatamente uma vez**; `new` entra no lugar.
  Vale para md, yaml, toml, py, ts, go, sql — nenhum parser envolvido
  (`edit_literal`, `crates/tupi-tools/src/tools/edit/literal.rs:158-288`).
  Âncora ambígua é recusa, não aviso: 0 ocorrências cita que o trecho não existe,
  >1 cita a contagem e as linhas; nada é escrito
  (`single`, `crates/tupi-tools/src/tools/edit/literal.rs:80-110`). É o análogo
  textual de `StaleHash` e não tem `--force`.
- **`start`+`end`+`expect`+`new` cobre o que a âncora não resolve**: linhas
  repetidas e arquivo grande onde copiar o trecho é desconfortável. `expect` é o
  texto atual de `start..=end` copiado do `read`, sem a numeração e sem a quebra
  final; faixa **sem** `expect` é recusa explícita de escrita às cegas, e faixa
  invertida, base zero ou meio par é recusa com a regra
  (`EditParams::lines`, `crates/tupi-tools/src/tools/edit/params.rs:168-201`).
- **As rotas são exclusivas, sem precedência implícita**: misturar duas é recusa
  que nomeia as duas e a regra, e uma rota nunca empresta campo da outra
  (`ROUTES`, `params.rs:4-7`; testes em `params.rs:306-348`).
- **`symbol`+`new_body` é atalho de AST, não pré-requisito.** Só em `.rs`
  (`crates/tupi-tools/src/tools/edit/mod.rs:55-94`); reparseia, confere o splice
  e restaura o original quando o resultado em disco não é o enviado. Fora de
  Rust, a resposta é a rota `old`+`new`.
- **`write` de arquivo inteiro é a exceção declarada**: arquivo novo ou
  substituição integral, nunca para mudar um trecho de arquivo que já existe.
  Toda sobrescrita declara `expected_hash` e todo caminho passa por
  `paths::resolve_for_write` (`crates/tupi-tools/src/support/paths.rs:53-101`);
  `write_many` acrescenta transação com rollback.
- **Guardas iguais em toda linguagem**: `expected_hash` obrigatório, casamento
  único e releitura imediata antes do rename; hash divergente ou arquivo trocado
  no meio viram `StaleHash` sem escrever
  (`crates/tupi-tools/src/tools/edit/literal.rs:181-216`).
- **Quem julga a escrita é o verificador do repo**, não o parser do tupi:
  `cargo fmt --check`, `tsc --noEmit`, `ruff`, `pytest`, `sqlfluff`,
  `markdownlint`, chamados por `exec` com timeout e cache
  ([ADR 0001](decisions/0001-cache-do-exec.md)). O comando já está declarado por
  glob em `configs/verify.ron`, mas a chamada automática ainda não está ligada ao
  caminho de escrita — ver [`features/verify.md`](features/verify.md).
- **Diff é formato de revisão, âncora é formato de entrada.** `patch` segue
  aceito para revisar e reverter (`-R`, `check_only`), mas exige aritmética de
  linha (`@@`) que a âncora literal não exige
  (`crates/tupi-tools/src/tools/patch/mod.rs:228-312`).
- **Documento segue o mesmo invariante**: afirmação entra com âncora
  `caminho:linha` conferida contra a leitura; âncora que não casa com o arquivo
  atual falha a checagem em vez de envelhecer em silêncio
  (`anchors::check`, `crates/tupi-tools/src/verify/anchors.rs:107-142`).

### Habilitar uma linguagem nova

Linguagem é dado, não ramo de código: as rotas textuais já cobrem qualquer
arquivo de texto e a única pergunta de linguagem do `edit` é
`symbol_parser` (`crates/tupi-tools/src/tools/edit/resolve.rs:33-39`), que lê a
tabela `SYMBOL_PARSERS` (`resolve.rs:15`). O passo a passo tem duas partes: o
que não se faz e o que se faz quando — e só quando — vale a pena.

**Obrigatório em toda linguagem — já vale sem escrever uma linha.** Nada a
habilitar: `old`+`new` e `start`+`end`+`expect`+`new` entram por
`EditParams::address` (`crates/tupi-tools/src/tools/edit/params.rs:63-163`) e
carregam as guardas de sempre:

1. `expected_hash` obrigatório (`params.rs:28`): hash divergente vira
   `StaleHash` antes de qualquer byte escrito.
2. Casamento único: `single` recusa 0 ocorrência (diz que a âncora não
   aparece byte a byte) e >1 (diz a contagem e as linhas)
   (`crates/tupi-tools/src/tools/edit/literal.rs:80-110`); faixa de linhas
   exige `expect` igual ao texto atual (`params.rs:168-201`).
3. Caminho confinado: toda escrita passa por `resolve_for_write`
   (`crates/tupi-tools/src/support/paths.rs`).
4. Verificação pelo verificador do repo, não pelo tupi: `cargo fmt --check`,
   `tsc --noEmit`, `ruff`, `pytest`, `sqlfluff`, `markdownlint` via `exec`.
   Parser do tupi não julga escrita.

**Opcional — a rota `symbol`.** Só onde há gramática tree-sitter e o modelo
pede `symbol`:

1. Gramática: dependência tree-sitter pinada no `Cargo.toml` de `tupi-tools`.
2. Variante em `Language`, com a extensão em `Language::from_path`
   (`crates/tupi-tools/src/support/ast.rs:16-22`) e a gramática em
   `tree_sitter_language` (`:24-29`).
3. Tabela: uma linha em `SYMBOL_PARSERS` (`resolve.rs:15`), no formato
   `("extensão", "Nome")` — e é o que a recusa lista em `no_symbol_parser`
   (`resolve.rs:42-51`).
4. Tipos de nó: `collect_children` reconhece os `kind` de item e de container
   (`crates/tupi-tools/src/support/span.rs:180-241`), `body_node` acha o corpo
   (`:243-265`) e `display_kind` traduz o `kind` para a recusa e o relatório
   (`:298-312`). Sem os três, a tabela de símbolos sai vazia.
5. Teste: extensão nova resolvendo `symbol` e a recusa de arquivo sem parser
   citando as rotas textuais, como em `resolve.rs:54-78`.

**O que não muda ao habilitar parser.** As guardas continuam as mesmas:
`symbol` não dispensa `expected_hash`, não tem precedência sobre as rotas
textuais (mistura de duas é recusa, `params.rs:63-163`) e o despacho segue um
só (`EditTool::edit`, `crates/tupi-tools/src/tools/edit/mod.rs:55-94`). Não há
`--force` para símbolo não encontrado: a recusa ensina `old`+`new`.

### Verificação declarada e âncora viva

Regra sem verificação é pedido: os dois controles objetivos da escrita já têm
contrato em `crates/tupi-tools/src/verify.rs` e são descritos em
[`features/verify.md`](features/verify.md) — comando do verificador por glob em
`configs/verify.ron` (primeira regra que casa vence, caminho sem regra é no-op,
falha é recusa que cita comando e saída, comando ausente só avisa) e âncora
`caminho:linha@blake3` conferida contra o disco. **Estado atual**: os dois
módulos estão testados e sem chamador — a verificação pós-escrita ainda é passo
manual via `exec`.

## Testes

- Teste não toca o perfil real do usuário. Caminho de efeito colateral recebe o
  ambiente por closure (`G: Fn(&str) -> Option<String>` para variáveis,
  `E: Fn(&Path) -> bool` para existência) e ganha `HOME` temporário
  (`tempfile`), no mesmo padrão de `data_dir` (`crates/tupi/src/main.rs:598-609`).
- Quando o alvo é o binário de verdade, o teste de integração monta um sandbox
  próprio e injeta `HOME`/`SHELL`/`LANG` no `Command`
  (ex.: `crates/tupi/tests/install_lang.rs`) — inclusive um locale `pt_BR` para
  provar que o locale **não** decide o idioma da saída.
- Idempotência se prova rodando duas vezes e comparando o arquivo, não por
  inspeção do código.
- Formato visual da TUI é contrato: teste que trava a linha exata (glifos, tempo,
  indentação) se atualiza junto com o render, não se contorna.

## Commits

- Um commit por mudança coerente de comportamento; mensagem no imperativo.
- Ao mexer em contrato observável (CLI, flag, saída), o mesmo commit atualiza a
  documentação de `docs/` e o `README.md`.
