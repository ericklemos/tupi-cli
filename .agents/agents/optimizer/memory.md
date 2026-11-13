# Optimizer Memory

## Known Hot Paths & Optimized Areas

- **`crates/tupi-tools/src/graph/builder.rs` (`GraphBuilder::build`)** — *Optimized 2026-11-13*
  - Replaced $O(C \cdot N)$ full-graph node scans, $O(M \cdot K)$ container scans, $O(F \cdot N)$ caller scans, per-call `FileScope::clone()`, duplicate uncached tree-sitter parses, and repeated `blake3` hashes / line splits with $O(N)$ indices (`containers_by_name`, `callers_by_file`, `defs_by_name`) and single-pass parsing/hashing.
  - Baseline on `tupi-cli` workspace (2280 nodes, 6335 edges): **2.135s → 0.803s** (**2.66x speedup**, -62.4%).

## Candidate Areas for Future Sessions

- `crates/tupi-tools/src/graph/scope.rs` (`resolve_within`) — `definitions.to_vec()` + `sort_unstable` + `dedup` when resolving ambiguous calls.
- `crates/tupi-tui/src/run.rs` (`transcript_layout_at`, `draw_transcript`) — `cache.rows(...).to_vec()` clones all cached `Line<'static>` on every redraw.
- `crates/tupi-tools/src/search.rs` (`SearchTool::search`) — clones `content` and `AstSnippet` per candidate before ranking/truncating.
