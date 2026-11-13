# ⚡ Optimizer: Index `GraphBuilder::build` lookups and eliminate redundant file parses

## 💡 What
Eliminated quadratic full-graph scans, redundant `FileScope` clones, repeated line splits, and duplicate tree-sitter parses / `blake3` hashes in `GraphBuilder::build` (`crates/tupi-tools/src/graph/builder.rs`), supported by zero-overhead entrypoints `AstChunker::snippets_from_tree` (`crates/tupi-tools/src/ast.rs`) and `ParseCache::parse_with_digest` (`crates/tupi-tools/src/graph/parse_cache.rs`).

## 📐 Approach
1. **Single parse & hash per file**: Compute `blake3::hash` once, parse once via `parse_tree(..., file_hash)`, and pass the resulting `Tree` to `AstChunker::snippets_from_tree`. Split `content.lines()` lazily at most once per file for `has_test_attribute`.
2. **$O(N)$ `BelongsTo` indexing**: Replaced per-member linear scan of all containers ($O(M \cdot K)$) and string clones with a borrowed `containers_by_name: HashMap<&str, Vec<NodeIndex>>` index.
3. **$O(N)$ caller & definition indexing**: Built `callers_by_file` and language-partitioned `defs_by_name: [HashMap<&str, Vec<NodeIndex>>; 2]` once in $O(N)$, returning borrowed `Cow<'_, [NodeIndex]>` slices in $O(1)$ per call site.
4. **Loop-invariant hoisting & lazy work**: Borrowed `&FileScope` once per file instead of cloning per call; skipped files with no callables or no calls; computed `collect_bindings` lazily only when a call has a non-`self` receiver; checked `self` calls directly over incoming `BelongsTo` edges without allocating a temporary `Vec`.

## 📊 Impact
- **Time Complexity**: $O(C \cdot N + M \cdot K + F \cdot N) \rightarrow O(C + N + F)$ (where $N$ = symbols, $M$ = members, $K$ = containers, $F$ = files, $C$ = call sites).
- **Space Complexity**: $O(N)$ temporary borrowed index maps during build; eliminated $O(C)$ `FileScope` + `Vec<NodeIndex>` heap allocations and $O(\text{snippets} \cdot \text{lines})$ line-vector allocations.
- **Measured Benchmark** (`tupi-cli` workspace: 2,280 nodes, 6,335 edges): **2.135s → 0.803s** (**2.66x speedup**, -62.4% wall-clock time).

## 🔬 Verification
- `cargo fmt --check` passed.
- `cargo clippy -p tupi-tools --all-targets` passed with 0 errors/warnings.
- `cargo test -p tupi-tools --all-targets` passed (all existing unit/integration tests plus new tests for `self` container resolution and `ParseCache` reuse).
