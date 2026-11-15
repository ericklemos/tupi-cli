# Convenções

## Onde cada coisa mora

- Efeito colateral só na composição: o binário (`crates/tupi/src/main.rs`) monta
  store, provider, tools e TUI; os crates de baixo não conhecem o todo.
- Feature que escreve em arquivo do usuário vive em módulo próprio, com as
  decisões isoladas em funções puras e a escrita num único ponto
  (ex.: `crates/tupi/src/install.rs` — `plan` decide, `apply` escreve).
- Subcomando de manutenção é despachado no `main` **antes** de env/redb/provider,
  para rodar sem chave de API e sem subir a TUI.
- Documentação de projeto e regra vivem sob `docs/` (ver
  [knowledge-placement](rules/knowledge-placement.md)).

## Testes

- Teste não toca o perfil real do usuário. Caminho de efeito colateral recebe o
  ambiente por closure (`G: Fn(&str) -> Option<String>` para variáveis,
  `E: Fn(&Path) -> bool` para existência) e ganha `HOME` temporário
  (`tempfile`), no mesmo padrão de `data_dir` (`crates/tupi/src/main.rs:527-538`).
- Quando o alvo é o binário de verdade, o teste de integração monta um sandbox
  próprio e injeta `HOME`/`SHELL`/`LANG` no `Command`
  (ex.: `crates/tupi/tests/install_lang.rs`) — inclusive um locale `pt_BR` para
  provar que o locale **não** decide o idioma da saída.
- Idempotência se prova rodando duas vezes e comparando o arquivo, não por
  inspeção do código.

## Commits

- Um commit por mudança coerente de comportamento; mensagem no imperativo.
- Ao mexer em contrato observável (CLI, flag, saída), o mesmo commit atualiza a
  documentação de `docs/` e o `README.md`.
