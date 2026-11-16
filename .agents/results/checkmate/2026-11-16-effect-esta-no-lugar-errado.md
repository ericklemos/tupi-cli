# ♟️ Checkmate: a mutabilidade declarada por booleano precisa ser demolida

## 🎯 O Alvo

O canal de declaração de mutabilidade de uma call: `Tool::mutates()` +
`Tool::effect()` (`crates/tupi-kernel/src/tool.rs:165-177`), e o único consumidor
de segurança que sobrou dele — `call_is_parallel`
(`crates/tupi-app/src/tool_batch.rs:16-20`).

Alvo único: **como o runner descobre, antes de executar, o conjunto de escrita de
uma call.** Não é estilo; é a premissa que sustenta paralelismo e invalidação.

## 📜 As Premissas

Predicados:

- `W(t,a)` ≡ a call `(t,a)` altera o workspace **ou o estado durável**
  (definição dada pelo próprio trait, `tool.rs:163-165`).
- `D(t,a)` ≡ a call declara isso: `effect(a) ≠ ReadOnly`.
- `P(t,a)` ≡ o runner a executa em paralelo com outras calls da mesma step.

Axiomas da estrutura atual:

- **P1 (contrato).** `∀t,a: W(t,a) → D(t,a)`. Texto do trait
  (`tool.rs:169-177`): "quem muta sem declarar caminhos devolve `Opaque`…
  nunca vazio".
- **P2 (gate).** `P(t,a) ⇔ D(t,a) ∧ mode_mask(t) ⊇ mode`
  (`tool_batch.rs:19-20`). `mutates()` não é consultado por ninguém no caminho
  de execução.
- **P3 (default).** `D` é derivado de um **booleano por tool**:
  `effect(a) = ReadOnly` sempre que a tool não sobrescreve `mutates()`
  (`tool.rs:171-177`, default `false` em `:165-167`).
- **P4 (decorador).** `RepairingTool::mutates() = true` fixo e
  `RepairingTool::effect()` delegado ao inner
  (`crates/tupi-tools/src/support/graph/mod.rs:60-78`).
- **P5 (reparo).** `RepairingTool::call` só repara o cache quando
  `self.effect(&args) ≠ ReadOnly` (`graph/mod.rs:44-52`).
- **P6 (limiar).** `Effect::ReadOnly` significa literalmente "Não altera o
  workspace" (`tool.rs:125-127`).

## 💥 A Refutação

### (a) Contradição dentro de um único impl — `shell`

`shell` é registrada embrulhada: `RepairingTool::new(Arc::new(ShellTool…))`
(`crates/tupi-tools/src/lib.rs:296-303`). E `impl Tool for ShellTool`
(`crates/tupi-tools/src/tools/shell/mod.rs:180-680`) contém **só** `spec`,
`call` e `describe_call`: nenhum `mutates`, nenhum `effect`.

Logo, por P3 ∧ P4, para `a = {action: "call", command: "cargo fmt"}`:

```
RepairingTool::mutates()  → true    (P4: "altera o workspace")
RepairingTool::effect(a)  → ReadOnly (P3: inner não declarou; e P6: "não altera o workspace")
```

Duas asserções contraditórias sobre a mesma call, no mesmo tipo, sobre o mesmo
valor. `cargo fmt` grava `.rs` — `W = true`, `D = false`: **P1 é falsa hoje**, e
a testemunha não é hipotética: é o caminho que o `RepairingTool` existe para
atender. Corolário imediato de P5: aquele `repair` nunca roda para `shell`. O
decorador foi construído e está morto para o único caso que o justifica.

### (b) Enumeração exaustiva do catálogo

Quem **declara**: `edit`, `write`, `write_many`, `move`, `patch` via
`support::effect::writes` (`crates/tupi-tools/src/support/effect.rs:60-67`;
`edit/tool.rs:4-6`, `write/mod.rs:51-53`, `write_many/mod.rs:146-148`,
`relocate/mod.rs:123-125`, `patch/mod.rs:181-183`); `exec` e `test_many` com
`Opaque` explícito (`exec/mod.rs:112-114`, `test_many/mod.rs:115-117`);
`state` (`tools/state/mod.rs:379-381`), `setup` (`tools/setup/mod.rs:232-234`),
`archive` (`tools/archive/mod.rs:161-163`) com `mutates() = true`; `docs`
decidindo por string de ação (`tools/docs/mod.rs:392-400`).

Quem **grava e não declara** (`W ∧ ¬D`, cada um derruba P1 sozinho):

| tool | grava | evidência |
|---|---|---|
| `shell` | `.rs` via `cargo fmt`/`rustfmt` | `shell/mod.rs:180-680` sem `effect`; embrulhada em `RepairingTool` |
| `facts` | `.agents/tupi/facts.jsonl` | `facts/mod.rs:51-203` só `spec`+`call` |
| `feedback` | `.agents/tupi/debug/feedback.jsonl` | `feedback/mod.rs:82-224` tem `mode_mask`, não tem `mutates` |
| `investigate` | dossiê em disco + job no DB | `investigate/mod.rs:140-220` escreve `report::write`; sem declaração |
| `sdd` | plano em `.agents/tupi/sdd` | `impl Tool for SddTool` (`sdd/mod.rs:651-1171`) sem `mutates`/`effect` |

A tabela é o xeque-mate por enumeração: **cinco** tools satisfazem `W` e violam
`D`. Nenhum teste as pega: `sdd_mode_tool_list_excludes_all_workspace_mutators`
(`lib.rs:585-624`) só olha `specs_for_mode`, não `effect`.

### (c) O booleano não pode ser correto — impossibilidade, não esquecimento

`W` é função de `(t, a)`; `D` (P3) é constante por tool. Então:

- `state(action="status")` → `W = false`, `D = false` (correto por acidente).
- `state(action="note")` → `W = true`, `D = false` (falso).
- após o remendo, `mutates() = true` (`state/mod.rs:379-381`): **as duas**
  passam a declarar mutação. O erro muda de lado, não desaparece.

Nenhum valor do booleano satisfaz as duas chamadas: a declaração é
argument-blind e o predicado não é. `∄ b ∈ {true,false}: ∀a, D(t,a) = b` para
`t = state`. A premissa P3 não é "incompleta"; é **insatisfazível**.

### (d) A segunda fonte de verdade re-parseia o mesmo dado

Para as 5 tools que declaram caminhos, `declared_paths`
(`support/effect.rs:15-57`) rededuz o conjunto de escrita do **mesmo `Value`**
que `call()` já vai reinterpretar, por heurística de string: chaves `"path"`,
`"from"`, `"to"`, `writes[].path`, e linhas `+++ `/`--- ` de um `patch` textual.
Duas interpretações independentes de um dado não tipado, sem nada que as
obrigue a concordar: renomear a chave `"writes"` (ou aceitar um alias) mantém
`call` correto e torna `effect` `Opaque`/vazio — divergência silenciosa,
indetectável por tipo e por teste.

Logo, `P1 ∧ P2 → ⊥` nos dados reais (a–b), e P3 é insatisfazível (c). A
estrutura cai.

## 🏗️ A Nova Estrutura

Requisitos derivados da refutação (não escolhidos):

- **R1.** A declaração é por *call*, não por tool.
- **R2.** Declaração e execução nascem do **mesmo parse** do argumento.
- **R3.** Fail-closed: não deve existir forma de executar sem declarar.

**Teorema.** A única estrutura que satisfaz R1–R3 é um método cujo retorno
carrega os dois: a declaração e a execução.

```rust
pub struct Planned {
    pub effect: Effect,
    pub run: Box<dyn FnOnce() -> Result<String, ToolError> + Send>,
}

pub trait Tool: Send + Sync {
    fn spec(&self) -> ToolSpec;
    /// Único método obrigatório: quem quer executar precisa devolver o efeito.
    fn plan(&self, arguments: Value) -> Result<Planned, ToolError>;
    fn describe_call(&self, arguments: &Value) -> String { /* … */ }
    fn mode_mask(&self) -> ModeMask { ModeMask::EXECUTION }
    fn spec_for_mode(&self, _mode: AgentMode) -> ToolSpec { self.spec() }
}
```

- `mutates()` **é apagado**. Não vira default, não vira `read_only()`: qualquer
  booleano por tool reabre (c).
- `effect()` deixa de existir como método: `effect = plan(args)?.effect`. Uma
  interpretação só (R2).
- O runner executa `plan(args)?` **uma vez** e usa `effect` do mesmo valor:
  `call_is_parallel(t, call) = t.plan?→effect.is_read_only() ∧ mode_mask ⊇ mode`.
- `RepairingTool` passa a embrulhar `plan`: devolve o `Planned` do inner com o
  `run` composto (reparar `effect.paths()`/tudo depois). Some a invariante
  tácita de "encaminhar todo método novo do trait" — o decorador compõe o único
  ponto de passagem.
- Ferramentas que gravam de forma opaca (`shell`, `exec`, `test_many`,
  `facts`, `feedback`, `investigate`, `sdd`) devolvem `Effect::Opaque` uma vez,
  no próprio `plan`, onde o efeito e a escrita são a mesma decisão.

## ✅ A Prova de que a nova satisfaz o que a antiga viola

1. **R1/R2 ⇒ P1 vale por construção.** `D` e `W` vêm do mesmo `Value` e da
   mesma decisão de código; para divergirem seria preciso que o mesmo match
   gerasse dois resultados. A linha de `shell`/`facts`/`feedback` que hoje
   mente **não compila** em forma mentirosa: para devolver `Planned` é preciso
   escrever `effect`.
2. **R3 ⇒ fail-closed por tipo.** Não há default para `plan`; esquecer a
   declaração é erro de compilação, não bug de concorrência descoberto em
   produção. O default permissivo (P3) deixa de ser representável.
3. **P2 reescrito** consome o mesmo `effect` que o reparo consome
   (P5) — as duas propriedades de segurança passam a ter **uma** fonte de
   verdade; divergir exige divergir de si mesmo.
4. **Minimalidade.** Quatro campos (`call`, `mutates`, `effect`, e o reparo
   externo) colapsam em um método que devolve dois valores já necessários. Toda
   estrutura com dois pontos de entrada admite um par divergente — construção
   dada em (a)/(d). Logo `plan` é mínima e única sob R1–R3.

## 💰 O Custo

- `plan` em **34 tools** registradas em `register_with`/`register_agent`
  (`crates/tupi-tools/src/lib.rs:167-423`, `agent_tools.rs:27-243`): mudança
  mecânica, `call` vira o corpo do `run`.
- Apagar `mutates`/`effect` do trait e reescrever `RepairingTool`
  (`support/graph/mod.rs:40-78`), `call_is_parallel`
  (`crates/tupi-app/src/tool_batch.rs:16-20`), `agent_register` e o teste
  `sdd_mode_tool_list_excludes_all_workspace_mutators`.
- **Muda comportamento:** `shell`/`facts`/`feedback`/`investigate`/`sdd` deixam
  de rodar em paralelo e passam a invalidar o cache de grafo. `state`/`setup`/
  `archive` continuam serializados (agora por `Opaque`, decidido no `plan`).
  Nada de formato de saída muda.
- Custo declarado: uma migração ampla e mecânica. É consequência da prova, não
  escolha — e o remendo oposto (mais um `mutates()` por tool) já foi tentado:
  a tabela (b) é o resultado.

## 🚧 Execução — estado e ordem (retomada)

**Estado:** nada de código foi alterado. A trait foi editada para exigir
`effect` (sem default, `mutates` removido) e **revertida** ao texto original
(`crates/tupi-kernel/src/tool.rs`, blake3 `bc4d32cda909`); `cargo check -p
tupi-kernel --all-targets` passa. O motivo da reversão é mecânico, não de
projeto: `effect` sem default quebra **todos** os `impl Tool` do workspace de
uma vez (~35 tools em `crates/tupi-tools/src/tools/*`, mais os dublês de teste
em `crates/tupi-kernel/src/tool.rs:243,331,369`,
`crates/tupi-kernel/tests/mode_mask.rs:8,24`, `crates/tupi-app/src/app.rs:3771`,
`crates/tupi-app/src/sdd_run.rs:988`). São ~45 inserções de uma linha; a árvore
só compila no fim do sweep, então ele precisa ser um passo único, não fatiado.

**Ordem de execução obrigatória:**

1. Sweep único de `effect` em todos os `impl Tool` (uma linha por impl: o
   `Effect` correto da tool; `ReadOnly` só para quem comprovadamente não grava).
   Só então `cargo check --workspace` volta a fechar.
2. Remover o default de `effect` e apagar `mutates` da trait.
3. Migrar as 5 tools que enumeram caminhos para decidir o efeito do **mesmo
   parse** que executa (`support/effect.rs:15-57` deixa de ser heurística).
