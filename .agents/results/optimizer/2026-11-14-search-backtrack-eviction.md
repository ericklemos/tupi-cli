# ⚡ Optimizer — 2026-11-14 · `search` / `BacktrackGraph` eviction

## Resultado: nenhuma otimização aplicada (candidata medida e revertida)

O workflow prevê parar sem PR quando nenhuma otimização adequada se sustenta na
medição. Este é o caso: a hipótese inicial foi derrubada e a correção
teoricamente correta não coube nas ferramentas de edição desta sessão.

## 💡 Onde o tempo vai

`SearchTool::search` (`crates/tupi-tools/src/tools/search/query.rs`) marca **cada
resultado cortado** como `Frontier`:

```rust
for &(index, _) in ranked.iter().skip(max_results) {
    self.backtrack.record_visit(relative, &snippets[index].qualified_name, file_hash,
        VisitLevel::Frontier, Some(&cut_label));
}
```

Cada `record_visit` (`crates/tupi-tools/src/support/backtrack.rs:170`) paga:
`Mutex::lock` + `NodeId::new` (2 alocações de `String`) + `via.to_string()` + um
`HashMap` insert e, por fim, `State::evict_if_needed`. Quando `nodes` chega ao
teto (`DEFAULT_MAX_NODES = 1024`), `evict_if_needed` fazia um
`min_by_key(last_touched)` sobre o mapa **inteiro por inserção** → $O(k \cdot n)$.

## 📊 Medições (debug, `cargo test -p tupi-tools --lib`)

Workspace sintético: 160 arquivos × 100 funções = 16.000 snippets candidatos.

| configuração | tempo |
|---|---|
| A — teto 1024, sort por backtrack ativo | **719 ms** |
| B — teto 1024, `repeat=allow` (sem comparador) | 709 ms |
| C — teto 1M (sem evicção), sort ativo | **455 ms** |
| D — teto 1M, sem comparador | 451 ms |

- **Evicção: 264 ms = 37%** da busca (A − C).
- **Comparador do `sort_by` (2 × `visit_level` por comparação): ~4-10 ms (<1.5%)**
  — a otimização "óbvia" (hoistar o lock para fora do comparador) é irrelevante.
- `nodes` saturou exatamente em **1024** → `DEFAULT_MAX_NODES` confirmado.
- Escala: 960 candidatos → 31 ms; 16.000 → 719 ms (16,7× candidatos, 23× tempo).

TUI, para comparação de magnitude: `transcript_layout_at` custa **1,51 ms/frame**
com 1000 entradas (5501 linhas), linear em linhas totais e não em linhas visíveis.

## 🔬 A tentativa que falhou

Trocar a evicção por nó por uma **passada única** (`collect last_touched` +
`select_nth_unstable` + `retain`) — matematicamente $O(n)$ por chamada em vez de
$O(n)$ por nó removido:

| | tempo @16.000 candidatos |
|---|---|
| antes | 719 ms |
| depois | **1253 ms (+74%)** |

**Por que piorou:** a evicção é chamada para remover **um** nó por vez. Uma
passada de `HashMap::iter().min_by_key()` com exatamente um `NodeId::clone()`
ganha de três varreduras sequenciais (`values()`, `select_nth_unstable`,
`retain`) mais uma `Vec<u64>` alocada por chamada. Evicção em lote só compensa
quando muitos nós saem por chamada.

## 📐 Correção identificada (próxima sessão)

Remover a evicção por item: registrar o lote de cortes de uma vez.

```rust
pub struct VisitRequest<'a> {
    pub relative: &'a str,
    pub symbol: &'a str,
    pub file_hash: Hash,
    pub level: VisitLevel,
    pub via: Option<&'a str>,
}

pub fn record_visits(&self, visits: &[VisitRequest<'_>]) {
    // um lock, N inserts, uma evicção
}
// record_visit(v) => record_visits(&[v])
```

`crates/tupi-tools/src/tools/search/query.rs` monta o `Vec<VisitRequest>` do laço
de cortes e chama `record_visits` uma vez. Complexidade: $O(k \cdot n)$ →
$O(k + n)$. Alvo: 719 ms → ~460 ms (~1.55×), e é o que faz a busca de um repo
grande deixar de degradar.

**Por que não foi aplicado:** adicionar o `VisitRequest` (ou um alias de tupla)
exige inserir texto fora do corpo de um símbolo existente. `patch` recusou
repetidamente os hunks de inserção em `backtrack.rs`/`run.rs` (contexto não
casava), `edit` só substitui corpos, e `write` exigiria reescrever o arquivo
inteiro (586 linhas) sem tê-lo lido por completo.

## ✅ Verificação

- `cargo test -p tupi-tools --lib`: **376 passed, 0 failed**.
- `crates/tupi-tools/src/support/backtrack.rs` → blake3 `fb5acfc4fde5` (original).
- `crates/tupi-tools/src/tools/search/tests.rs` → blake3 `1ea44e4f925f` (original).
- `crates/tupi-tui/src/run.rs` → blake3 `bee22bcf9fac` (original).
- Nenhum arquivo de produção alterado; nenhum PR criado.
