# `tupi install` — declarar o diretório do binário no `PATH`

Subcomando que descobre o diretório do executável em execução e o declara, de
forma persistente e idempotente, no arquivo de inicialização do shell do usuário.
Uso para o usuário final está no `README.md` (seção *Installation*); este arquivo
guarda o **contrato interno** — onde está o código, quais invariantes não podem
ser quebradas e o que ficou fora de escopo.

## Contrato

```text
tupi install [--print] [--shell <auto|bash|zsh|fish|posix|powershell>]
```

`parse_options` (`crates/tupi/src/install.rs:947`) aceita `--shell valor` e
`--shell=valor`; qualquer outra flag aborta com a linha de uso
(`--print, --shell <auto|bash|zsh|fish|posix|powershell>`). Não existe
`--uninstall`, PATH de sistema, cópia do binário nem suporte a `csh`/`tcsh`/`nushell`.

## Onde mora

| peça | arquivo |
|---|---|
| módulo inteiro (funções puras, escrita, CLI, testes unitários) | `crates/tupi/src/install.rs` |
| `mod install;` | `crates/tupi/src/main.rs:12` |
| despacho do subcomando | `crates/tupi/src/main.rs:28-32` (`install_args`, `:729-734`) |
| teste de integração que roda o binário real | `crates/tupi/tests/install_lang.rs` |

## Despacho antes de qualquer inicialização pesada

`main` chama `install_args(&argv)` **antes** de `parse_cli_options`, `dotenvy`,
`Store::open` e da checagem de chave de API. Invariante: `tupi install` funciona
sem `TUPI_CLI_DEEPSEEK_PLATAFORM_APIKEY`, não cria nem abre `~/.tupi/tupi.redb`,
não baixa modelo e não sobe a TUI. Nenhuma inicialização pesada pode ser movida
para cima desse bloco.

## Fluxo

`run` (`:1201`) → `parse_options` → `output_lang(current_dir())` → `current_exe`
→ `plan(options, exe, get_env, exists)` → `preview_in` (com `--print`) ou
`apply` + `report_in`.

`plan` (`:1014`) é a única função que decide: `bin_dir` (pai do
`canonicalize()` do executável), shell (`--shell` ou `detect_shell`), home
(`HOME` ou `USERPROFILE`, erro se ausente), `rc_path`, `action` (via `probe`,
que simula o `apply` sem escrever) e `in_path` (`declared_in_path`).

| shell | arquivo | linha escrita |
|---|---|---|
| `zsh` | `~/.zshrc` | `export PATH="<dir>:$PATH"` |
| `bash` | `~/.bashrc` | `export PATH="<dir>:$PATH"` |
| `posix` | `~/.profile` | `export PATH="<dir>:$PATH"` |
| `fish` | `~/.config/fish/config.fish` | `set -gx PATH "<dir>" $PATH` |
| `powershell` | `$PROFILE` (`Documents/PowerShell/...`) | `$env:Path = "<dir>;$env:Path"` |

Detecção em `auto`: basename de `$SHELL` (zsh/bash/fish/posix/powershell);
sem `$SHELL`, a existência de `~/.zshrc`, `~/.bashrc`, `~/.profile` nessa ordem;
nada encontrado → `Posix`.

## Invariantes

- Marcadores fixos `MARKER_START`/`MARKER_END` (`# >>> tupi install >>>` /
  `# <<< tupi install <<<`) delimitam o bloco; `block` (`:489`) termina com
  newline e cada shell usa só a sua sintaxe (`:` no Unix, `;` no PowerShell).
- `splice` (`:580`) substitui no lugar, preservando a posição e todo o resto
  byte a byte; marcador final ausente significa "até o fim do arquivo"; append
  usa exatamente uma newline e não cria linha em branco quando o arquivo já
  termina em newline.
- Segunda execução com o mesmo diretório devolve `AlreadyPresent` e **não
  escreve**; o arquivo fica idêntico. `Action` é `created` / `updated` /
  `already-present`.
- `apply` (`:517`) cria os diretórios pais (caso do fish), recusa bytes não-UTF8
  e caminho que não é arquivo regular citando o caminho, sonda a escrita sem
  `truncate` antes de alterar e restaura o original se a escrita falhar no meio.
- Diretório já presente no `PATH` do ambiente **não** dispensa o bloco (o efeito
  só vale em shells novos); relatório e dry run acrescentam uma nota dizendo isso.
- Idioma da saída: `lang` explícito de `ai.project.json` via `output_lang`
  (`:1133`); ausente ou inválido → inglês. O locale do sistema é ignorado de
  propósito.
- Sem dependência nova: só `std` (`env`/`fs`/`path`), sem `dirs`, `winreg` ou
  `clap`.

## Limitações conhecidas

- Windows: apenas o perfil do PowerShell; a chave
  `HKCU\Environment\Path` do registry não é tocada (exigiria `winreg`). Blocos
  continuam sendo escritos com `\n` — um perfil CRLF mantém um único par de
  marcadores (testado), mas o arquivo não é convertido.
- Shells fora da lista caem em `Posix` e recebem `~/.profile`.
- O `PATH` da sessão atual não muda: o usuário precisa reabrir o shell (ou dar
  `source` no arquivo).
