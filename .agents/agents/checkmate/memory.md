# Checkmate — Memory

## Padrões de prova reutilizáveis

- **Fail-open em propriedade de segurança** = demolição imediata. Se o default de um contrato é o valor *permissivo* e a omissão é silenciosa, a propriedade só vale por convenção, não por construção. Cheque sempre o default contra o princípio declarado no mesmo trait (aqui: `mutates()` fail-open vs. `mode_mask` fail-closed).
- **Duas fontes de verdade sobre a mesma propriedade** (aqui: `mutates()` + allowlist `BATCHABLE_READS`) divergem; a verdade efetiva vira a string frágil, não o tipo.
- **Enumeração exaustiva por `assert exists`** sobre quem sobrescreve um método default revela a violação real (10 tools falsas) em vez de hipótese.

## Estruturas já refutadas

- `Tool::mutates()` default `false` + `BATCHABLE_READS` (paralelismo de tool calls) — refutado em 2026-11-16.
