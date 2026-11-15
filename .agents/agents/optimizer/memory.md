# Optimizer Memory

## Known Hot Paths & Optimized Areas

- **`crates/tupi-tools/src/support/graph/builder.rs` (`GraphBuilder::build`)** — *Optimized 2026-11-13*
  - Replaced $O(C \cdot N)$ full-graph node scans, $O(M \cdot K)$ container scans, $O(F \cdot N)$ caller scans, per-call `FileScope::clone()`, duplicate uncached tree-sitter parses, and repeated `blake3` hashes / line splits with $O(N)$ indices (`containers_by_name`, `callers_by_file`, `defs_by_name`) and single-pass parsing/hashing.
  - Baseline on `tupi-cli` workspace (2280 nodes, 6335 edges): **2.135s → 0.803s** (**2.66x speedup**, -62.4%).

## Profiled, Not Yet Optimized

- **`SearchTool::search` cut-result backtrack recording** — *measured 2026-11-14, no change landed*
  - `crates/tupi-tools/src/tools/search/query.rs` records a `Frontier` visit per cut result; `crates/tupi-tools/src/support/backtrack.rs` pays a `Mutex` lock + `NodeId` (2 `String` allocs) + `via` `String` + `evict_if_needed` **per item**.
  - At capacity (`DEFAULT_MAX_NODES = 1024` in `backtrack.rs`) `evict_if_needed` does an $O(n)$ `min_by_key` scan per insert → $O(k \cdot n)$.
  - Baseline @16,000 candidate snippets (debug): **719ms** (cap 1024) vs **455ms** (cap 1M) → eviction = **264ms / 37%**. @960 snippets: 31ms total.
  - The `ranked.sort_by` comparator (2 × `visit_level` per comparison) is only **4-10ms** — do **not** bother hoisting it.
  - Next step: batched `record_visits` (one lock + one evict per search). **Do not** swap the per-item eviction for a `select_nth_unstable` + `retain` single pass — measured **+74%** (719ms → 1253ms).

- **`crates/tupi-tui/src/run.rs` (`transcript_layout_at` / `draw_transcript`)** — *measured 2026-11-14*
  - Rebuilds the whole transcript every frame: `cache.rows(...).to_vec()` + `indent_line` per message, plus `format!`/`wrap_text`/`String::repeat` per tool row, plus the `owners` vector (which is `#[allow(dead_code)]`).
  - Measured with synthetic sessions (`cargo test -p tupi-tui --lib`, debug): **161µs/frame** (100 entries / 551 rows), **612µs** (400 / 2201), **1.51ms** (1000 / 5501). Linear in total rows, not in visible rows (~40).
  - Only `rows.len()` + `rows[start..end]` are needed by `draw_transcript`; a counting pass + windowed build would cut it to ~O(visible).

## Candidate Areas for Future Sessions

- `crates/tupi-tools/src/support/graph/scope.rs` (`resolve_within`) — `definitions.to_vec()` + `sort_unstable` + `dedup` on every call site, plus a `from_globs` `Vec` and an `in_crate` `Vec` collected per call site even when `scope.globs` is empty.
- `crates/tupi-tui/src/run.rs` — windowed transcript layout (see above); drop the dead `owners` vector.
- `crates/tupi-tools/src/tools/search/query.rs` — `Candidate { content: content.clone(), snippet: snippet.clone() }` per rendered result clones the whole file body (bounded by `max_results`, so small).
