# The Skeptic — Compact Disaster Registry (`tupi-cli`)

## Resolved Systemic Risks

1. **[RESOLVED 2026-11-11] Context Pruner / DeliveryLedger Desynchronization, Target Cliff & Re-Shrink Loop (`tupi-app/src/app.rs`, `tupi-domain/src/context.rs`)**
   - `plan_context` scales `message_target` proportionally across the whole domain (`message_budget * target / context_budget`), eliminating the cliff at `target == message_budget`.
   - `shrink_old_results` pointer text instructs `repeat="allow"`, and `shrunk_count` only increments on newly shrunk items (ignoring `is_pointer` / `is_shrunk_arg` tombstones).

2. **[RESOLVED 2026-11-11] Zombie Blocking Threads on Stream Cancel (`tupi-app/src/app.rs`, `tupi-kernel/src/tool.rs`, `tupi-tools/src/proc.rs`)**
   - Thread-local cancellation propagation wired into `proc::run`'s 20ms poll loop (`kill_tree`) plus an in-flight tool barrier (`Notify` + `AtomicUsize`) awaited by `App::cancel`.

3. **[RESOLVED 2026-11-11] `git` Flag Smuggling, Symlink & Quoted-Path Escape (`tupi-tools/src/patch.rs`, `diff.rs`, `read.rs`, `relocate.rs`)**
   - `PatchTool::validate_paths` rejects symlink modes and quoted/escaped header paths and canonicalizes every touched path; `DiffTool`/`ReadTool::prepare_rev` reject leading `-` and pass `--end-of-options`; `MoveTool::copy_dir` rejects symlinks.

4. **[RESOLVED 2026-11-11] Fake Incremental Graph Repair & Missing Path Extraction (`tupi-tools/src/graph/cache.rs`, `graph/mod.rs`)** — `GraphCache::repair(paths)` verifies `file_hashes` against disk; `RepairingTool::paths` extracts `path`/`from`/`to`/`writes`/`patch`.

5. **[RESOLVED 2026-11-11] Silent Fingerprint Truncation in `ExecTool`** — superseded by #7: `fingerprint` fails closed on both file count and total bytes.

6. **[RESOLVED 2026-11-11] SSRF, DNS Rebinding & Redirect Exposure (`tupi-tools/src/web.rs`)** — DNS resolved and filtered, IPs pinned via `--resolve`, every redirect hop validated.

7. **[FIXED 2026-11-11] Stale-Green `exec` Cache (`tupi-tools/src/tools/exec/`)** — two rounds:
   - Round 1: key became a `blake3` of **content** (path + length + bytes) instead of `(path, len, mtime)`; `hidden(false)` brought `.cargo/config.toml` and `.sqlx/` into the key; the extension whitelist died; a hit prints `em cache há 2m5s · execução original: 3.4s` instead of impersonating a fresh run (`Cached.at`, `sections_for` takes `Option<Duration>`).
   - Round 2: gitignore filtering removed (`.ignore/.git_ignore/.git_global/.git_exclude/.parents` all false) so ignored fixtures count; `follow_links(true)` so dir symlinks count; env inputs (`RUSTFLAGS`, `CARGO_ENCODED_RUSTFLAGS`, `CARGO_BUILD_TARGET`, `CARGO_BUILD_RUSTFLAGS`, `RUSTUP_TOOLCHAIN`) hashed before the files, injected via `fingerprint_with(root, limit, environment)`.
   - Only exclusions left, each named: `target/`, `.git/`, `.brain/`, `.agents/tupi/`.
   - Guards: `exec/tests.rs` — same-len + restored mtime, `.cargo/config.toml`, `.json` fixture, gitignored `.json`, dir symlink, agent bookkeeping, env injection, cache-age label (20 tests green).
   - Reports: `.agents/results/skeptic/2026-11-11-exec-cache-stale-green.md`, `2026-11-11-exec-cache-fix.md`.

12. **[RESOLVED 2026-11-16] "The `exec` cache may never hit" (old #10) — the key includes Tupi's own session log**
   - `fingerprint_with` excludes only `target/`, `.git/`, `.brain/` and `.agents/tupi/` (`crates/tupi-tools/src/tools/exec/render.rs:88`); `.tupi/sessions/<id>/log.jsonl` is appended on every tool interaction (measured 139,534 -> 141,168 bytes in 4s), so the key dies between any two `exec` calls. Four identical `exec kind=check package=tupi-kernel` in one session: zero hits.
   - Not the process lifetime: one `ExecTool` per registry (`crates/tupi-tools/src/lib.rs:293`).
   - Also dead: `ToolsOptions::{exec_cache, web_cache}` claim a "cache persistente" (`lib.rs:57-60`), default `true` (`:89-90`), and are read nowhere.
   - Report: `.agents/results/skeptic/2026-11-16-exec-cache-never-hits.md`.

## Open Systemic Risks (unfalsified — do not trust)

8. **[OPEN] Universal `repeat="allow"` pruning pointer invites non-idempotent re-execution (`tupi-domain/src/context.rs:284-338`)**
   - Emitted for every `Role::Tool` message regardless of tool name, without recording which tool ran or with which arguments; for `patch`, `write_many`, `relocate` and `exec` it invites repeating a side effect, and for tools without a `repeat` parameter it is a schema lie.

9. **[OPEN] `tools::workflow::tests` (6 tests) fail on this tree** — the catalog returns zero `WorkflowSource::Native` entries while `every_native_workflow_is_valid_ron_in_configs` passes. Not caused by the exec change (different module); **not verified as pre-existing** (no git in session). Falsifier: `cargo test -p tupi-tools --lib tools::workflow` on a clean checkout.

10. **[OPEN] Closing the `exec` cache miss activates a stale-green: the key ignores everything outside the root**
    - `fingerprint` hashes only the `RUSTUP_TOOLCHAIN` *name* (`crates/tupi-tools/src/tools/exec/render.rs:39-45`), never the resolved toolchain: `rustup update` swaps the compiler without touching a byte in the key.
    - `~/.cargo/config.toml`, `CARGO_HOME`, `CARGO_NET_OFFLINE`, `SQLX_OFFLINE`, `DATABASE_URL`, `CC`/`CFLAGS` and `PATH` are absent from the key; a cached `ok` outlives all of them.
    - The hit path has no end-to-end test: `exec/tests.rs:177` hands an age to `sections_for`; nothing calls `call` twice and asserts `em cache há`.
    - Falsifier: exclude `.tupi/`, run the same `exec` twice (hit), then `rustup update` + repeat — a hit across a different compiler proves the stale-green.

11. **[OPEN] `.agents/tupi/` is the one exclusion without proof** — it is the tool's own state (`facts.jsonl`, `pending.jsonl`, `debug/`); no lexical hit shows a test reading it as input, but if one ever does, the exclusion becomes a silent hole and nothing fails. Guard today only pins the exclusion (`fingerprint_ignores_the_agents_own_bookkeeping`).

## Memory Integrity Notes
- Older entries cited `crates/tupi-app/src/context.rs`, which does not exist; the pruning code lives in `crates/tupi-domain/src/context.rs`. Every path here must be verified by `read` or `search` in the session that writes it.

## Architectural Assumptions Under Watch
- `ResponseStore` TTL is 600s and max size is 8MB (`crates/tupi-tools/src/lib.rs:119`); any session pausing >10m loses every `h_xxxxxxxx` peek handle.
- `EventBus` capacity is 256 (`crates/tupi/src/main.rs:25`).
- `crates/tupi-tools/Cargo.toml` now depends on `tupi-infer` with feature `local-models`; a half-refactored `tupi-infer` breaks `cargo check -p tupi-tools` even though nothing in `tupi-tools` uses it yet.
