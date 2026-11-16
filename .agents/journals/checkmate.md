# Checkmate — Journal

## 2026-11-16 - `mutates()` fail-open: a invariante de mutação já é falsa em 10 tools

**A Premissa Refutada:** `Tool::mutates()` (default `false`) é a fonte de verdade da mutabilidade, e a segurança do paralelismo depende de toda tool mutante sobrescrever `mutates() -> true`.

**A Prova:** Enumeração exaustiva (`assert exists`) mostrou que `write`, `write_many`, `edit`, `patch`, `move`, `shell`, `exec`, `test_many`, `feedback` e `facts` não sobrescrevem `mutates()` — todas reportam `false` apesar de gravar no workspace. A invariante do contrato (P1) é falsa hoje; a única barreira real é a allowlist de strings `BATCHABLE_READS`, redundante e sem tipo. Redução ao absurdo: o default permissivo torna a omissão indistinguível do caso benigno, logo a segurança é por convenção, não por construção — e o mesmo trait declara *fail-closed* em `mode_mask`, contradizendo `mutates()`.

**A Consequência:** O default precisa ser fail-closed (`mutates()` default `true`, ou `read_only()` default `false`) e a allowlist por nome eliminada: `call_is_parallel(t) = ¬mutates(t) ∧ mode_mask(t) ⊇ mode`.

## 2026-11-16 - `RepairingTool::paths`: um `bool` não transporta conjunto de escrita

**A Premissa Refutada:** `RepairingTool::paths(arguments)` enumera exatamente os arquivos que a call alterou, então `mutates() = true` e `repair(paths)` descrevem o mesmo efeito. `ShellParams` não tem chave de caminho: `paths(shell) = ∅` com `mutates(shell) = true` — a mesma struct afirmando "alterei o workspace" e "não alterei nada". O conjunto vazio de `repair` é ainda indistinguível do caso benigno, então a falha é silenciosa.

**A Prova:** Enumeração exaustiva dos mutantes: (a) embrulhado com argumento parseável — falha em `shell` e em movimento de diretório (`from`/`to` são diretórios, o grafo indexa arquivos); (b) embrulhado sem caminho — `repair(∅)` inerte; (c) fora do wrapper (`exec`, `test_many`, `sdd`, `state`, `facts`, `feedback`, `setup`) — nenhum `repair`. O watcher não resgata: `is_source` só aceita `rs|kt|kts`, o evento é assíncrono e `ToolsOptions::hermetic` o desliga. Logo P1 ∧ P2 ∧ P5 → ⊥.

**A Consequência:** O efeito sobe para o trait do kernel como tipo, não como booleano: `Effect { ReadOnly, Writes(Vec<PathBuf>), Opaque }`; `Opaque` invalida o grafo inteiro e `BATCHABLE_READS` deixa de existir. Implementado; resultado em `.agents/results/checkmate/support-graph-write-set.md`.

## 2026-11-16 - O `Effect` implementado ainda mente: a declaração está no lugar errado

**A Premissa Refutada:** a entrada anterior supôs que subir o efeito para o trait como tipo (`Effect`) e derivar o default de `mutates()` fechava o buraco. Não fecha: `effect()` default = `mutates(t) ? Opaque : ReadOnly` (`crates/tupi-kernel/src/tool.rs:171-177`) é **constante por tool**, e `call_is_parallel` (`crates/tupi-app/src/tool_batch.rs:16-20`) passou a confiar só nela. A premissa falsa — "mutabilidade se declara por tool, uma vez" — voltou por outro canal: agora ela governa concorrência, não só invalidação.

**A Prova:** (i) Contradição intra-tipo: `RepairingTool::mutates() = true` fixo (`support/graph/mod.rs:75-77`) e `RepairingTool::effect()` delegado ao inner (`:60-73`); `impl Tool for ShellTool` (`tools/shell/mod.rs:180-680`) não tem nenhum dos dois, logo `shell(cargo fmt)` é simultaneamente "altera o workspace" (`tool.rs:163-165`) e `Effect::ReadOnly` = "Não altera o workspace" (`tool.rs:125-127`) — e o `repair` de `graph/mod.rs:44-52`, que existe para `cargo fmt`, nunca roda. (ii) Enumeração: `shell`, `facts` (`facts/mod.rs:51-203`), `feedback` (`feedback/mod.rs:82-224`), `investigate` (`investigate/mod.rs:140-220`) e `sdd` satisfazem `W` e violam `D`. (iii) Insatisfatibilidade, não esquecimento: `W(t,a)` depende do argumento e `D` é constante por tool ⇒ `∄ b: ∀a, D(state,a)=b` (`status` lê, `note` grava); o remendo `mutates() = true` em `state/mod.rs:379-381` só inverte o erro. (iv) `declared_paths` (`support/effect.rs:15-57`) re-parseia por heurística de string (`path`, `from`, `to`, `writes[].path`, `+++`/`---`) o mesmo `Value` que `call` reinterpreta: duas interpretações sem tipo que as amarre.

**A Consequência:** o defeito não é o default nem a ausência de tipo de efeito; é o *lugar* da declaração. Ela tem que ser por call e nascer do mesmo parse da execução: `fn plan(&self, Value) -> Result<Planned, ToolError>` com `Planned { effect, run }`; `mutates()` e `effect()` apagados, omissão virando erro de compilação, `RepairingTool` compondo `plan`. Resultado em `.agents/results/checkmate/2026-11-16-effect-esta-no-lugar-errado.md`.
