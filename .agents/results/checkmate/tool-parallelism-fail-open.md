# ♟️ Checkmate: o paralelismo de tool calls deve ser demolido — `Tool::mutates` é fail-open e a invariante de mutação já é falsa em 10 tools

## 🎯 O Alvo

O mecanismo que decide se uma tool call roda em paralelo com outra da mesma step. Duas peças:

- `crates/tupi-kernel/src/tool.rs:136-138` — `fn mutates(&self) -> bool { false }` (o contrato de mutabilidade do trait `Tool`).
- `crates/tupi-app/src/tool_batch.rs:15,18-25` — `BATCHABLE_READS` e `call_is_parallel` (o consumidor).

## 📜 As Premissas

O que a estrutura atual assume para funcionar:

- **P1 (contrato)** — `mutates(t) == true ⟺ t altera o workspace/estado durável`. É o que o doc em `tool.rs:135` declara: "A call altera o workspace (ou o estado durável): o runner não a executa em paralelo com outra call da mesma step."
- **P2 (default)** — o valor padrão de `mutates()` é `false` (`tool.rs:138`).
- **P3 (consumo)** — `call_is_parallel(t) = (name(t) ∈ BATCHABLE_READS) ∧ ¬mutates(t) ∧ mode_mask(t) ⊇ mode` (`tool_batch.rs:18-25`).
- **P4 (fonte paralela)** — `BATCHABLE_READS` é uma lista hardcoded de 6 strings (`read, read_many, peek, search, docs, assert`), independente de `mutates()` (`tool_batch.rs:15`).
- **P5 (invariante de segurança)** — **I** = "nenhuma call mutante executa em paralelo com outra call da mesma step".

## 💥 A Refutação

**1. Predicado falso nos dados reais.** P1 afirma que `mutates()` espelha a mutação. Enumeração exaustiva (`assert exists`) mostra que 10 tools que **gravam no workspace** não sobrescrevem `mutates()` e, portanto, reportam `false`:

`write`, `write_many`, `edit`, `patch`, `move`, `shell`, `exec`, `test_many`, `feedback`, `facts`.

Logo `mutates(write) == false ∧ write muta o workspace` — contradição direta com P1. P1 é falsa no código atual, não em cenário hipotético.

**2. Inconsistência interna do design.** O próprio trait declara o princípio *fail-closed* em `mode_mask` (`tool.rs:141`: "Fail-closed: o padrão é só execução"), mas `mutates()` é *fail-open* (default `false`). Para uma propriedade de **segurança** (não paralelizar mutação), o default permissivo é o inverso do princípio que o mesmo trait enuncia.

**3. Contrapositiva / redução ao absurdo.** A segurança I depende de P1 ∧ P2. Assuma que I é garantida **por construção**. Então toda tool mutante tem `mutates() == true`. Mas P2 torna `mutates() == false` o caso **por omissão**, e a omissão de um método default não gera erro de compilação nem lint. Logo existe estado alcançável — e de fato já existente (as 10 tools) — em que `mutates() == false` para tool mutante sem nenhuma falha detectável. Esse estado é **indistinguível, por tipo**, do estado "tool de leitura". Segue-se que I não é garantida por construção: é garantida por convenção, e a convenção já está violada no disco.

**4. Enumeração exaustiva — o xeque-mate.** Quem impede I hoje?

- Fonte `mutates()`: violada em 10 tools → não impede I.
- Fonte `BATCHABLE_READS` (string hardcoded): impede **por acaso** (os 10 nomes mutantes não estão na lista), mas (a) é redundante com `mutates()`, (b) é uma string sem vínculo de tipo, (c) nada impede a inclusão futura de um nome mutante na lista com `mutates() == false`. O caso `docs` demonstra: `docs` está na allowlist e é **condicionalmente** mutante (`DocsTool::mutates` devolve `self.writable`, `docs/mod.rs:392-394`) — só não falha porque alguém lembrou de sobrescrever manualmente.

Todo lance do defensor exige uma ação manual que o tipo não obriga. Todo lance leva à mesma conclusão: **a estrutura delega I à convenção, e a convenção já falhou.**

## 🏗️ A Nova Estrutura

Propriedade exigida: **I garantida por construção, com uma única fonte de verdade.** Segue-se que:

1. `mutates()` torna-se **fail-closed**: default `true` (assume mutação). A omissão passa a produzir **serialização** (seguro). O paralelismo — uma otimização — passa a exigir declaração explícita.
2. `BATCHABLE_READS` é **eliminada**: `call_is_parallel(t) = ¬mutates(t) ∧ mode_mask(t) ⊇ mode`. Com default `true`, `¬mutates()` é uma declaração explícita do autor da tool ("esta tool não grava"), não uma omissão.
3. (Legibilidade) introduzir `fn read_only(&self) -> bool { false }` como a face positiva, mantendo a semântica fail-closed.

**Unicidade/minimalidade.** A propriedade a preservar é de segurança. O único default que a garante por construção é o restritivo (`true` / `read_only() == false`). Qualquer default permissivo reintroduz P2 e a dependência de convenção. A allowlist por nome é eliminável porque `mutates()` fail-closed expressa **exatamente** o predicado que a allowlist tentava codificar como string.

## ✅ A Prova

- Com default `true`, toda tool mutante que **omitir** a sobrescrita fica serializada → I vale por omissão.
- As 10 tools hoje falsas passam a ser corretas **por construção** (default `true` = mutam = serializadas), sem exigir edição imediata para segurança.
- As tools de leitura (`read`, `read_many`, `peek`, `search`, `assert`, `docs`) declaram explicitamente `read_only() == true`, tornando o paralelismo uma decisão auditável.
- Uma única fonte de verdade: o contrato decide; não há mais lista de nomes a sincronizar.

**Preservado vs. mudado:**
- *Preservado* — o paralelismo de leitura das tools hoje beneficiadas (desde que declarem `read_only`).
- *Mudado* — `docs` deixa de depender da coincidência de `self.writable` (serializa por default); `write`/`edit`/`patch`/`shell`/`exec`/`test_many`/`feedback`/`facts` passam a ser reconhecidos como mutantes, corrigindo a verdade do contrato.

## 💰 O Custo

- `crates/tupi-kernel/src/tool.rs`: 1 linha (default `false` → `true`, ou novo `read_only()` default `false`).
- `crates/tupi-app/src/tool_batch.rs`: remover `BATCHABLE_READS` (:15) e simplificar `call_is_parallel` para `¬mutates(t) ∧ mode_mask(t) ⊇ mode`.
- ~6 tools de leitura: adicionar `fn read_only(&self) -> bool { true }` (edição explícita e pequena).
- Testes: `crates/tupi-app/tests/tool_parallelism.rs` e o teste do default em `crates/tupi-kernel/src/tool.rs` refletem o novo default.

O custo é consequência da prova, não escolha: inverter o default é a única mudança que restaura I sem manter duas fontes de verdade.
