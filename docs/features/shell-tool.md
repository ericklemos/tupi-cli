# Tool `shell` — saída bruta por padrão, allowlist restrita e recusa que ensina

Contrato da tool `shell` depois do plano `shell-friction`, que fechou o atrito
medido no run `1794840347760-112433` (8 das 22 chamadas de shell perdidas: 2
recusas de política, 4 falhas por binário/caminho, 3 saídas truncadas em
silêncio, 2 retries). A decisão está em
[ADR 0002](../decisions/0002-shell-poder-e-ruido.md).

## Parâmetros

`ShellParams` (`crates/tupi-tools/src/tools/shell/mod.rs:19-41`): `command`
(obrigatório, comando **único**, executado por `bash -lc` com cwd no root),
`terms`, `filter`, `question`, `context` (padrão 2, teto 20), `max_blocks`
(padrão 6, teto 40), `timeout_secs` (padrão 120) e `budget_tokens`. O contrato de
`command` e o formato de `describe_call` (`:643`) não mudaram.

## Filtro é opt-in

- `terms` sozinhos **não omitem nada**: a saída volta inteira e o cabeçalho
  avisa `terms ignorados: o filtro so liga com "filter": true; nada foi omitido`.
- `filter: true` + `terms` liga o filtro duro e o cabeçalho declara o que saiu:
  `- linhas: <casadas> de <total> · <N> linhas omitidas pelo filtro · <K> blocos`.
- `question` sozinho ranqueia parágrafos (grupos de até 40 linhas) sem omitir
  nada por termo; o cabeçalho lista `- blocos: <K>`.
- Sem filtro e sem `question`, a saída bruta volta em bloco `~~~text` e a seção
  `saida completa` guarda a versão longa, cortada por `clip`
  (`crates/tupi-tools/src/tools/exec/render.rs:4-16`), que **declara quantos
  caracteres omitiu** e manda o excedente para o `peek`.
- Comparação: substring case-insensitive, OR entre os termos — não é regex.

## Fallback sem casamento

Filtro ligado e nenhum termo casou (`ShellTool::call`):

| saída | o que aparece |
|---|---|
| até 10 linhas | a saída **uma única vez**, em `nenhum termo casou · saida bruta`, sem `...` e sem repetir linha em volta |
| mais de 10 linhas | `nenhum termo casou · inicio e fim da saida`, com 5 primeiras + `...` + 5 últimas |
| vazia | `nenhum termo casou · sem saida` |

## Allowlist

- A lista vem de `ai.project.json` → `shell.allow`; quando a chave existe, ela
  **substitui** o padrão (não soma). Sem a chave, vale `DEFAULT_ALLOW`:
  `git`, `rg`, `grep`, `ls`, `cat`, `head`, `tail`, `wc`, `find`, `jq`, `sed`,
  `awk`, `sort`, `uniq`, `diff`, `file`, `stat`, `du`, `df`, `tree`, `echo`,
  `pwd`, `date`.
- `cargo` **saiu** de `DEFAULT_ALLOW`: só `cargo fmt` e `rustfmt` (com no máximo
  flags) passam, pela regra fixa de formatador; build/check/test/clippy/run
  pertencem à tool `exec`, que declara os alvos.
- `git` só aceita subcomando de leitura (`GIT_READ`: `status`, `diff`, `log`,
  `show`, `blame`, `annotate`, `grep`, `ls-files`, `ls-tree`, `rev-parse`,
  `rev-list`, `describe`, `shortlog`, `cat-file`, `show-ref`, `name-rev`,
  `whatchanged`, `diff-tree`, `diff-files`, `diff-index`, `for-each-ref`,
  `merge-base`, `show-branch`, `check-ignore`, `check-attr`, `version`, `help`).
  `-c`/`--config-env` são recusados (alias rodaria comando por dentro do git) e
  nenhum global com valor passa antes do subcomando.
- Wrappers (`WRAPPERS`: `sh`, `bash`, `env`, `xargs`, `timeout`, `nice`,
  `python`, `node`, `make`, `just`, …) nunca passam — nem quando estão
  explicitamente em `shell.allow`.
- `allowed` (`:61-157`) é a única fonte de verdade: `ShellTool::call` recusa
  antes de executar.

## Binário ausente não derruba o turno

`installed`/`on_disk` (`:352-370`) conferem o primeiro token antes de executar:
caminho com `/` é checado no disco, nome simples é procurado no `PATH` do
próprio processo (o bash herda o mesmo ambiente), builtins do bash ficam isentos
e ausência de `PATH` **não** bloqueia. Faltando o binário, nada roda e a resposta
traz a seção `### shell — binario ausente` com o equivalente já liberado
(`EQUIVALENT`: `rg`/`ripgrep`/`ag`/`ack` → `grep -rn -e "<padrao>" <caminho>`,
`fd`/`fdfind` → `find`, `bat` → `sed -n '1,200p'`, `tree` → `find`, `sd` →
`sed -i`) ou, sem equivalente, a lista de binários já liberados. Isso é texto:
nada é executado, e a chamada seguinte funciona.

## Recusa sempre ensina a alternativa

`refusal` (`:400-429`) separa três casos:

1. **Composição** (`|`, `;`, `>`, `<`, `$`, crase) ou `--manifest-path`: um
   comando por chamada; para vários padrões, um `-e` por padrão
   (`grep -rn -e "A" -e "B" <caminho>`) ou a tool `search` com
   `keywords: ["A", "B"]`.
2. **Binário fora da allowlist**: nome do binário, a chave `shell.allow` onde
   liberar e, via `native_hint` (`:388-395`), a tool nativa equivalente —
   `cat`/`head`/`tail`/`wc`/`*sum` → `read`, `grep`/`awk`/`rg` → `search`,
   `sed` → `edit`, `ls`/`tree`/`find` → `map`, `curl`/`wget` → `fetch`,
   `rm`/`mv`/`cp` → `move`, `touch` → `write`.
3. **Binário liberado mas proibido** (`cargo`, `git` de escrita, wrapper): diz
   qual regra específica bloqueia, além do hint nativo.

## Memória do projeto fora do resultado

`Memory::recall_for_tool` (`crates/tupi-app/src/memory.rs:246-281`) devolve
`None` para `shell`: resultado de inspeção não carrega fato lembrado. A memória
continua entrando no prefixo do turno, fora do bloco de resultado, e as demais
tools não mudaram. Teste:
`crates/tupi-app/tests/shell_memory.rs` (`the_shell_result_never_carries_project_memory`).

## Onde mora

| o quê | onde |
|---|---|
| parâmetros e schema | `crates/tupi-tools/src/tools/shell/mod.rs:19-41`, `:168-216` |
| allowlist e regras de binário | `allowed` (`:61-157`) |
| detecção de ausência e equivalentes | `installed` (`:352`), `on_disk`, `EQUIVALENT` |
| recusas acionáveis | `refusal` (`:400`), `native_hint` (`:388`) |
| montagem do resultado e fallback | `ShellTool::call` (`:218-641`) |
| supressão da memória | `crates/tupi-app/src/memory.rs:246-281` |

## Invariantes

- Nada aqui amplia poder: o filtro é opt-in, a allowlist só encolheu, e hint é
  texto (nenhum comando sugerido é executado).
- Saída bruta sem casamento aparece **uma vez**; `...` só separa início e fim
  quando a saída não cabe inteira.
- Truncamento é declarado — `clip` diz quantos caracteres omitiu, e o excedente
  está no `peek` pelo `budget_tokens`.
- Toda recusa nomeia o caminho de liberação **e** a alternativa equivalente.

## Limitações conhecidas

- `shell.allow` do projeto substitui `DEFAULT_ALLOW`: enumerar um binário
  derruba todos os outros, inclusive `grep`/`sed`/`find`.
- `rg` continua em `DEFAULT_ALLOW` para quem o tem instalado; sem ele a chamada
  cai no aviso de binário ausente apontando `grep -rn`.
- `terms` é substring, não regex; não há `-v` nem exclusão por termo.
- A checagem de ausência olha só o primeiro token: intérprete ou comando
  chamado de dentro de um script não é inspecionado (e script não passa, por ser
  wrapper).
- O fallback de saída longa mostra apenas 5 linhas de cada ponta.
