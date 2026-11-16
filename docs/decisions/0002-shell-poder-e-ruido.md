# ADR 0002: o `shell` reduz poder e ruído — filtro opt-in, allowlist restrita, recusa acionável

## Contexto

A telemetria do run `1794840347760-112433` (`.agents/tupi/debug/telemetry.shell.jsonl`,
seq 1-44) mostra 8 das 22 chamadas de shell desperdiçadas. Nenhuma delas falhou
por falta de poder; todas falharam por ruído ou por recusa muda:

- **Filtro duro silencioso.** `terms` era aplicado por padrão
  (`crates/tupi-tools/src/tools/shell/mod.rs:19-37`, `:163-213`). `terms:["-"]`
  em `tail -n 6` devolveu `linhas: 4 de 6` e o modelo não percebeu que duas
  linhas sumiram; `stat -c` em 3 arquivos devolveu `2 de 3` porque `meta.json`
  não casava com os termos, e o modelo re-rodou o comando com 1 arquivo — um
  round trip inteiro perdido.
- **Fallback que repetia a saída.** `du --inodes -s .`, com uma linha de saída,
  foi rotulado `0 correspondências · 0 de 1 · 0 blocos`, com a mesma linha
  repetida entre `...`.
- **Allowlist larga demais.** `cargo` cru permitia `cargo run`; `git` sem
  restrição de subcomando permitia `git checkout`/`commit` — o bloqueio de
  wrappers não cobria esses dois. E `rg` estava liberado numa máquina sem
  ripgrep: a primeira chamada do turno morreu com `rg: command not found`.
- **Recusa sem saída.** `sha256sum target/debug/tupi` foi recusado sem dizer o
  que fazer; `grep -rn "A\|B"` foi recusado por composição e custou um retry até
  o modelo achar `-e`. O texto já citava a chave `shell.allow`, mas não propunha
  equivalente.
- **Memória colada no resultado.** Resultados de shell vinham prefixados com até
  4 fatos de memória do projeto (seq 12, 20, 22, 24) que o comando não pediu: o
  modelo lê fato lembrado como saída do comando.

## Decisão

1. **`terms` deixa de filtrar por padrão.** Só `filter: true` explícito liga o
   filtro; sem ele a saída volta inteira e o cabeçalho avisa que `terms` foi
   ignorado. Com filtro ligado, o cabeçalho declara `N linhas omitidas` e o total
   real.
2. **Fallback sem casamento mostra a saída bruta uma vez.** Até 10 linhas cabem
   inteiras, sem `...` e sem repetir linha; acima disso, 5 + `...` + 5.
3. **Allowlist encolhe.** `cargo` sai de `DEFAULT_ALLOW` (só `cargo fmt`/`rustfmt`
   pela regra de formatador) e `git` fica restrito aos subcomandos de leitura já
   aceitos (`GIT_READ`), com `-c`/`--config-env` recusados. Wrappers continuam
   fora mesmo quando declarados em `shell.allow`.
4. **Binário ausente é checado antes de executar** e vira aviso com equivalente
   liberado (`rg` → `grep -rn`), não falha do turno.
5. **Recusa ensina.** Composição cita `-e` por padrão e a tool `search`; binário
   fora da allowlist cita a chave e o substituto nativo (`read`, `search`,
   `edit`, `map`, `fetch`, `move`, `write`).
6. **Memória do projeto não entra no resultado do `shell`.** Continua no prefixo
   do turno, fora do bloco de resultado; as demais tools não mudaram.
7. **Contrato público intacto**: `ShellParams.command` e `describe_call` não
   mudam; nada disso exige versão nova do schema.

## Alternativas rejeitadas

- **Escalar a recusa com `--force`/flag de escape.** Rejeitada pelo requisito do
  projeto: correção aqui só pode reduzir poder. Uma saída de emergência
  reproduziria `cargo run` e `git checkout` por outro caminho.
- **Remover `terms` de vez.** Rejeitada: `terms` com `filter: true` continua
  sendo a forma barata de recortar saída grande, e a spec de `native-search`
  ainda não substitui a tool `grep` no fluxo.
- **Manter `terms` ligado e só "avisar melhor" no cabeçalho.** Rejeitada: o
  prejuízo não é o aviso faltando, é a omissão acontecer sem pedido — duas linhas
  de 6 somem e `2 de 3` parece resposta completa.
- **Bloquear o turno quando o binário falta.** Rejeitada: era exatamente o
  defeito (`rg: command not found` matou a primeira chamada). Aviso com
  equivalente mantém a chamada útil e não executa nada.
- **Deixar `cargo`/`git` na allowlist e confiar no bloqueio de wrappers.**
  Rejeitada: wrapper é sobre o binário, não sobre o subcomando; `cargo run` e
  `git commit` passavam sem wrapper nenhum.
- **Anexar a memória só "quando relevante" ao shell.** Rejeitada: qualquer
  heurística de relevância deixa o fato lembrado indistinguível da saída do
  comando, que é o defeito. O prefixo do turno já é o lugar da memória.

## Consequências

- Resultado de shell é evidência crua: ou tem a saída, ou diz por que não tem
  (binário ausente, recusa com alternativa). Cabeçalho de filtro nunca mais
  esconde omissão.
- `shell.allow` do projeto continua **substituindo** o padrão: quem enumerar um
  binário perde `grep`/`sed`/`find`. Está documentado como limitação
  ([`features/shell-tool.md`](../features/shell-tool.md)).
- `rg` segue em `DEFAULT_ALLOW` para quem o tem; a dívida real é a tool nativa de
  busca, dona da spec `native-search`.
- Testes que travam o contrato: `no_filter_keeps_the_previous_summary`,
  `filter_declares_the_omitted_lines`, `a_single_line_without_match_appears_once`,
  `long_output_without_match_keeps_head_and_tail`,
  `the_default_allowlist_refuses_cargo_run`,
  `a_missing_binary_is_reported_instead_of_failing_the_call`,
  `rg_without_ripgrep_points_to_grep`,
  `a_composition_refusal_teaches_the_alternative`,
  `a_binary_refusal_names_the_release_path_and_the_substitute` (todos em
  `crates/tupi-tools/src/tools/shell/mod.rs`) e
  `the_shell_result_never_carries_project_memory`
  (`crates/tupi-app/tests/shell_memory.rs`).
- O caminho de "reduzir poder" tem custo de descoberta: binário novo exige editar
  `shell.allow`, e o retorno disso é a mensagem de recusa, não uma tentativa às
  cegas. Aceito — é o preço de não deixar `cargo run` passar.
