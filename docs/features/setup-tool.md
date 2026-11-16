# Tool `setup` — bloco de tools em `AGENTS.md` e skill, escrita só sob `apply`

Contrato da tool `setup` depois do plano `philosophy-docs-readme-agents` (T3),
que endureceu o `merge`: o bloco gerado é ancorado por linha, bloco duplicado
colapsa e texto autoral fora dos marcadores sobrevive. A decisão está em
[ADR 0004](../decisions/0004-controle-por-tool.md) e o princípio em
[`../philosophy.md`](../philosophy.md) (P6).

## Parâmetros

`SetupParams` (`crates/tupi-tools/src/tools/setup/mod.rs:18-27`): `action`
(`preview` é o padrão, `apply` grava; qualquer outro valor é recusado com
`InvalidArguments` e a lista do que é aceito), `agents_file` (padrão
`AGENTS.md`), `skills_dir` (padrão `skills/tupi-tools`) e `budget_tokens`
(opcional; sem ele vale o teto da tool).

## Bloco gerado

`block` (`crates/tupi-tools/src/tools/setup/mod.rs:64-72`) monta, a partir da
lista viva de `ToolSpec`: `<!-- tupi-tools:start -->` (`START`, `:14`), o
cabeçalho `## Tools do Tupi`, uma linha `- **<nome>** — <descrição>` por tool,
`<!-- tupi-tools:end -->` (`END`, `:15`) e o newline final. Nada de texto
autoral entra no bloco — e nada dentro do bloco sobrevive a um `apply`.

## Merge ancorado por linha

- `marker_line` (`:90-100`) só reconhece o marcador quando a **linha inteira**,
  depois de `trim`, é igual a ele. Menção do marcador no meio de uma frase é
  prosa, não abertura de bloco (`a_marker_mentioned_in_prose_is_not_a_block`).
- `merge` (`:74-87`): sem `START`, `appended` (`:121-128`) põe o bloco no fim,
  depois de `trim_end` e com uma linha em branco de separação. Com `START`, o
  fim é o primeiro `END` depois dele; **sem `END`, o bloco vai até o fim do
  arquivo** — conserta bloco truncado em vez de duplicar
  (`an_unterminated_block_is_replaced_instead_of_duplicated`).
- `without_extra_blocks` (`:103-119`) descarta todo bloco extra depois do
  primeiro, junto com a linha em branco que o segue: arquivo com duplicatas
  antigas converge para um bloco (`an_old_duplicate_block_collapses_to_one`).

## Escrita

`SetupTool::call` (`:164-222`) confina os dois destinos com
`paths::resolve_for_write` (`crates/tupi-tools/src/support/paths.rs:53`), cria
os diretórios pais e grava com `atomic_write`
(`crates/tupi-tools/src/support/paths.rs:142`). A skill vai para
`<skills_dir>/SKILL.md`, com front-matter `name: tupi-tools` e o mesmo bloco no
corpo. O relatório abre em `### setup — preview` ou `### setup — aplicado` e
lista a contagem de tools, as linhas do bloco, o caminho de `AGENTS.md` e o da
skill; `- gravado` só aparece quando houve escrita.

## Invariantes

- `preview` é o padrão e não toca o disco (`preview_does_not_write`).
- `apply` duas vezes mantém **um** bloco e o texto autoral byte a byte
  (`apply_twice_keeps_a_single_block_and_the_authored_text`).
- Regra que precisa valer mora fora dos marcadores: dentro do bloco, o texto é
  reescrito a cada `apply`.
- `action` inválido é erro explícito (`an_invalid_action_is_rejected`), não
  degradação silenciosa para `preview`.

## Limitações conhecidas

- O bloco cresce com o número de tools e o `AGENTS.md` inteiro entra no prompt
  com teto de 32 KiB (`crates/tupi/src/agents_config.rs:3`): o texto autoral
  precisa caber no que sobra.
- `SKILL.md` é sempre sobrescrito por inteiro — não tem marcador, então edição
  manual nele se perde no próximo `apply`.
- `setup` não valida o markdown resultante nem reindexa `docs`: o efeito só
  aparece no turno seguinte, quando `AGENTS.md` é montado no prefixo.
