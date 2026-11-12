# The Skeptic — Compact Disaster Registry (`tupi-cli`)

## Resolved Systemic Risks (2026-11-11)

1. **[RESOLVED] Context Pruner / DeliveryLedger Desynchronization, Target Cliff & Re-Shrink Loop (`tupi-app/src/app.rs`, `tupi-domain/src/context.rs`)**
   - `plan_context` now scales `message_target` proportionally across the entire domain (`message_budget * target / context_budget`), eliminating the discontinuity cliff at `target == message_budget`.
   - `shrink_old_results` pointer text explicitly instructs `repeat="allow"`, and `shrunk_count` only increments on newly shrunk items (ignoring existing `is_pointer` / `is_shrunk_arg` tombstones).

2. **[RESOLVED] Zombie Blocking Threads on Stream Cancel (`tupi-app/src/app.rs`, `tupi-kernel/src/tool.rs`, `tupi-tools/src/proc.rs`)**
   - Thread-local cancellation propagation (`with_tool_cancel` / `is_tool_cancelled`) wired into `proc::run`'s 20ms poll loop (`kill_tree`) and an in-flight tool barrier (`Notify` + `AtomicUsize`) awaited by `App::cancel` before releasing `self.active`.

3. **[RESOLVED] `git` Flag Smuggling, Symlink & Quoted-Path Escape (`tupi-tools/src/patch.rs`, `tupi-tools/src/diff.rs`, `tupi-tools/src/read.rs`, `tupi-tools/src/relocate.rs`)**
   - `PatchTool::validate_paths` rejects symlink modes (`120000`), rejects quoted/escaped header paths, and canonicalizes every touched path via `paths::resolve_for_write`.
   - `DiffTool` and `ReadTool::prepare_rev` reject leading `-` flags and control/whitespace characters, validate `path` inside root, and pass `--end-of-options` to `git`.
   - `MoveTool::copy_dir` rejects symlinks inside copied/moved directories.

4. **[RESOLVED] Fake Incremental Graph Repair & Missing Path Extraction (`tupi-tools/src/graph/cache.rs`, `tupi-tools/src/graph/mod.rs`)**
   - `GraphCache::repair(paths)` verifies `file_hashes` against disk and invalidates lazily.
   - `RepairingTool::paths` extracts `"path"`, `"from"`, `"to"`, `"writes"`, and `"patch"` headers.

5. **[RESOLVED] Silent Fingerprint Truncation in `ExecTool` (`tupi-tools/src/exec.rs`)**
   - `fingerprint()` returns `None` when the workspace exceeds `MAX_FINGERPRINT_FILES` (5,000), bypassing the cache instead of hashing a truncated pre-sort slice.

6. **[RESOLVED] SSRF, DNS Rebinding & Redirect Exposure in `FetchTool` & `ResearchTool` (`tupi-tools/src/web.rs`)**
   - `web::curl` resolves DNS via `ToSocketAddrs`, blocks loopback/private/link-local/CGNAT/IPv4-mapped IPv6 addresses, pins resolved IPs via `curl --resolve`, and validates every HTTP redirect hop in Rust.

## Architectural Assumptions Under Watch
- `ResponseStore` TTL is 600s and max size is 8MB (`crates/tupi-tools/src/lib.rs:119`); any session pausing >10m loses all `h_xxxxxxxx` peek handles.
- `EventBus` capacity is 256 (`crates/tupi/src/main.rs:25`).
