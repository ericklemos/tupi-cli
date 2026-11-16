# ♟️ Checkmate: `RepairingTool` — o conjunto de escrita não pode ser inferido dos argumentos

## 🎯 O Alvo

`crates/tupi-tools/src/support/graph/mod.rs` — o decorador `RepairingTool`, único
mecanismo síncrono pelo qual o `GraphCache` sabe que o disco mudou.

## 📜 As Premissas da Estrutura Refutada

- **P1 (efeito declarado).** `Tool::mutates()` é *a* declaração de efeito:
  `true` ⟺ a call altera o workspace (`crates/tupi-kernel/src/tool.rs`).
- **P2 (conjunto de escrita).** Para uma call mutante
  `RepairingTool::paths(&arguments)` devolve **exatamente** os caminhos alterados.
- **P3 (cobertura).** Toda tool que grava é embrulhada em `RepairingTool`.
- **P4 (rede de segurança).** O `GraphWatcher` cobre o que P2/P3 errarem.
- **P5 (frescor).** Logo, após qualquer call mutante, leitura de símbolo não
  devolve estrutura obsoleta.

## 💥 A Refutação

**Lemma A — `mutates()` e `paths()` se contradizem dentro da mesma struct.**
`mutates()` retornava `true` incondicionalmente; `paths()` só enxergava `path`,
`from`, `to`, `writes[].path` e cabeçalhos `+++`/`---`. `ShellParams` não tem
nenhuma dessas chaves. Segue-se `paths(shell) = ∅` com `mutates(shell) = true`:
a mesma call afirma "alterei o workspace" e "o conjunto alterado é vazio" — `⊥`.

**Lemma B — `repair(∅)` é indistinguível do caso benigno.** Um shell que reescreve
`src/lib.rs` produzia o mesmo efeito no cache de um shell que só lê.

**Lemma C — há mutantes fora do wrapper.** Enumeração em `crates/tupi-tools/src/lib.rs`:
`exec`, `test_many`, `sdd`, `state`, `facts`, `feedback`, `setup` gravam e não
eram embrulhados; `exec` nem declarava mutação (default `false`).

**Lemma D — P4 não resgata.** `is_source` só aceita `rs|kt|kts`; o evento é
assíncrono; `ToolsOptions::hermetic` desliga o watcher.

Enumeração exaustiva: (a) wrapper com argumento parseável — falha em `shell` e em
movimento de diretório; (b) mutante sem caminho — `repair(∅)` inerte; (c) mutante
fora do wrapper — nenhum `repair`. Todos os casos falham, e o modo de falha é
silencioso.

## 🏗️ A Nova Estrutura (implementada)

O conjunto de escrita é conhecimento **da tool**, e um `bool` não transporta
conjunto. O efeito subiu para a fronteira do trait:

```rust
pub enum Effect { ReadOnly, Writes(Vec<PathBuf>), Opaque }

fn mutates(&self) -> bool { false }
fn effect(&self, _arguments: &Value) -> Effect {
    if self.mutates() { Effect::Opaque } else { Effect::ReadOnly }
}
```

- `RepairingTool::call` calcula `self.effect(&arguments)` e mapeia:
  `ReadOnly` → nada; `Writes(paths)` → `cache.repair(paths)`;
  `Opaque` → `cache.repair(&[])` (invalidação total).
- `RepairingTool::effect` = efeito do inner; quando o inner devolve `Opaque`, o
  wrapper usa o vocabulário de argumentos como **sobre-aproximação** do conjunto
  declarado (nunca um conjunto vazio).
- `crates/tupi-tools/src/support/effect.rs` concentra a extração
  (`declared_paths`, `writes`, `is_read_action`); `RepairingTool::paths` delega.
- Declarações: `edit`/`write`/`write_many`/`patch`/`move` → `Writes`;
  `exec`/`test_many` → `Opaque`, agora embrulhados em `RepairingTool`;
  `docs` → `ReadOnly` para `query`/`read`/`map`, `Opaque` nas demais.
- `crates/tupi-app/src/tool_batch.rs`: `BATCHABLE_READS` removida;
  `call_is_parallel` = `effect(arguments).is_read_only() && mode_mask().contains(mode)`.

## ✅ A Prova

O que a nova estrutura satisfaz e a antiga viola: `mutates(t) ⟹ effect(t) ≠ ∅`.
Nenhuma tool mutante consegue mais produzir conjunto vazio silencioso — ou declara
os caminhos, ou invalida tudo. As três lacunas caem: `shell` deixa de mentir,
`exec`/`test_many` entram no contrato, e o watcher vira otimização em vez de
premissa. `BATCHABLE_READS` deixa de ser fonte de verdade: a lista de nomes some e
o paralelismo passa a ler a declaração tipada, sem permitir que uma tool mutante
entre no lote por estar na lista.

Verificação executada: `cargo test -p tupi-tools --test graph_incremental` (ok),
`cargo test -p tupi-app --test tool_parallelism` (ok),
`cargo test -p tupi-app --test tool_batch_reads` (ok).

## 💰 O Custo

Trait do kernel (+`Effect`, +`effect`), 8 tools com declaração explícita, wrapper
sem parse próprio, `tool_batch` sem lista de nomes. Refatoração média e mecânica;
o parse de argumentos continua existindo, mas como sobre-aproximação declarada e
não como fonte de verdade.

## O que fica declarado como próximo passo

- `sdd`, `state`, `facts`, `feedback`, `archive`, `investigate`, `setup` mantêm
  `mutates() = true` e, portanto, `Opaque` em toda call — correto e conservador,
  mas perde paralelismo nas ações de leitura (`status`, `list`, `resume`).
  Declarar `ReadOnly` por ação é a otimização pendente.
- `mutates()` sobrevive como projeção de compatibilidade (sem argumentos) para não
  quebrar fixtures de teste; a fonte de verdade é `effect`.
