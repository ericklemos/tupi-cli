# Checkmate — Journal

## 2026-11-16 - `mutates()` fail-open: a invariante de mutação já é falsa em 10 tools

**A Premissa Refutada:** `Tool::mutates()` (default `false`) é a fonte de verdade da mutabilidade, e a segurança do paralelismo depende de toda tool mutante sobrescrever `mutates() -> true`.

**A Prova:** Enumeração exaustiva (`assert exists`) mostrou que `write`, `write_many`, `edit`, `patch`, `move`, `shell`, `exec`, `test_many`, `feedback` e `facts` não sobrescrevem `mutates()` — todas reportam `false` apesar de gravar no workspace. A invariante do contrato (P1) é falsa hoje; a única barreira real é a allowlist de strings `BATCHABLE_READS`, redundante e sem tipo. Redução ao absurdo: o default permissivo torna a omissão indistinguível do caso benigno, logo a segurança é por convenção, não por construção — e o mesmo trait declara *fail-closed* em `mode_mask`, contradizendo `mutates()`.

**A Consequência:** O default precisa ser fail-closed (`mutates()` default `true`, ou `read_only()` default `false`) e a allowlist por nome eliminada: `call_is_parallel(t) = ¬mutates(t) ∧ mode_mask(t) ⊇ mode`.
