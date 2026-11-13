# Optimizer Journal

## 2026-11-13 - Quadratic Graph Resolution in GraphBuilder::build
**Bottleneck:** Building the code graph for `tupi-cli` (~90 files, 2,280 nodes, 6,335 edges) took **2.135s** in debug mode because `GraphBuilder::build` scanned all `graph.graph.node_indices()` inside the per-call-site loop ($O(C \cdot N)$), linearly searched all containers per member ($O(M \cdot K)$), scanned all nodes per file for callers ($O(F \cdot N)$), cloned `FileScope` on every call site, and parsed/hashed each file multiple times.
**Learning:** Whenever `GraphCache` invalidates on file edits, `GraphBuilder::build` rebuilds the whole graph from scratch, so $O(C \cdot N)$ call resolution dominates latency even on medium-sized repos.
**Optimization:** Indexed containers by name, callers by relative path, and definitions by `(Language, name)` once in $O(N)$ using `Cow<'_, [NodeIndex]>` borrows, borrowed `&FileScope` once per file, deferred `collect_bindings` until a non-`self` receiver call needs it, and parsed/hashed each file once via `AstChunker::snippets_from_tree` + `ParseCache::parse_with_digest`. Runtime dropped to **803ms** (2.66x faster).
