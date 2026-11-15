# The Skeptic's Journal — Critical Disaster Scenarios

## 2026-11-11 - Stale-Green `exec` Cache: The Verification Gate Lies About Freshness
**The Flaw:** `ExecTool::call` (`crates/tupi-tools/src/tools/exec/mod.rs:162-203`) keys the cache on
`args.join(" ") + fingerprint(root)`, and `fingerprint` (`crates/tupi-tools/src/tools/exec/render.rs:32-75`)
only admits extensions `rs|toml|lock`, never dotfiles (`hidden(true)`) and never anything ignored by
`.gitignore` (`standard_filters(true)`), identifying each file as `(path, len, mtime_nanos)` instead of
hashing content. `.cargo/config.toml` (rustflags, linker, target-dir, `[env]`), `.sqlx/`, `.env*` and
every fixture outside `rs/toml/lock` (`.json`, `.sql`, `.proto`, `.yaml`) are invisible to the key.
On a hit the tool prints `status: ok · <original duration>s · em cache`, reconstructing a plausible
fresh run time from the cloned `Output.duration` (`exec/mod.rs:84-87`, `exec/diagnostics.rs:65-150`).
**The Catalyst:** A crate reads `tests/fixtures/*.json` (or `.cargo/config.toml` sets a `--cfg`/linker).
The agent runs `exec kind=test` and gets `ok`. Only the fixture (or the hidden config) changes — via
`write`/`edit` or by the user. Nothing in `rs|toml|lock` changed, so the next `exec kind=test` is a cache
hit and returns the previous `ok`. The failure needs the one case nobody tests: a change outside the
whitelist.
**The Fallout:** The project's only falsifier (AGENTS.md: "nenhuma afirmação entra no contexto sem um
falsificador") certifies broken code with a green `ok` and a fabricated duration. The agent reports
"tests pass", opens the PR, and the pipeline finds the regression after review. Secondary: every `exec`
pays a full `ignore::WalkBuilder` crawl (gitignore matching + metadata + sort) to save one cargo run that
cargo already deduplicates with content hashes — the cache costs CPU on every call and buys no tokens.
Only defense available to the model is `refresh: true` on every call, which nullifies the cache entirely.

## 2026-11-11 - The Prefix-Blind Target & Shrink-Induced Pruning Spiral
**The Flaw:** `App::plan_context` (`crates/tupi-app/src/app.rs:105-122`) subtracts `fixed_tokens` (`tool_tokens + prefix_tokens + warning_tokens`) from `context_budget` to compute `message_budget`, yet passes the raw `context_target` into `plan_amortized_with` capped only by `.min(message_budget)` instead of subtracting `fixed_tokens`. Worse, inside `tupi_domain::context::plan_with` (`crates/tupi-domain/src/context.rs:74-118`), `shrink_old_results` runs unconditionally whenever `total(messages) > ceiling`, mutating old tool outputs into `[resultado anterior omitido...]` pointers even when the final turn unit cannot be dropped (`units(&kept) == 1`). `Plan::trimmed()` returns `true` whenever `shrunk_results > 0`, triggering `session.replace_messages(planned.messages)` in `App::send_message` (`app.rs:459`), which permanently overwrites the in-memory `SessionStore` with pointer text.
**The Catalyst:** With default config (`budget: 32000`, `target: 16000`, `fixed_tokens: ~8000`), `message_budget` is `24000`, while `message_target` remains `16000` (leaving total prompt after pruning at `16000 + 8000 = 24000` instead of `16000`). When `token_scale` rises above `1.33x` (or `fixed_tokens >= 16000`), `message_target == message_budget`, which hits `target >= ceiling` in `plan_amortized_with` and **completely disables amortized pruning**, degrading to step-by-step thrashing that invalidates the provider's KV prefix cache on every single tool step. Simultaneously, if a single multi-step turn exceeds `message_budget`, old tool results are permanently erased from `SessionStore` and replaced with instructions to call the tool again—where `DeliveryLedger` (`crates/tupi-tools/src/ledger.rs`) responds with *"Já entregue nesta sessão — corpo em peek h_xxxx"* because the ledger was never notified that the context pruner destroyed the delivery.
**The Fallout:** Complete breakdown of the core value proposition ("Economia de tokens é requisito"). The system enters an unrecoverable livelock: the model is told by the context pointer to re-call the tool; the tool ledger tells the model it already has the content in context or via a `ResponseStore` handle (which expires after 600s or 8MB LRU eviction); the KV prefix cache is invalidated on every step; and the user pays full uncached input token rates for up to 100 steps of circular confusion.

## 2026-11-11 - Unaborted `spawn_blocking` Zombie Tool Execution After Stream Cancellation
**The Flaw:** `App::cancel` (`crates/tupi-app/src/app.rs:797-840`) aborts the outer Tokio task (`task.abort()`) and immediately resets `self.active` to `0`, allowing the next user turn to start. However, `run_tool` (`crates/tupi-app/src/app.rs:20-31`) dispatches synchronous tool execution inside `tokio::task::spawn_blocking(move || tool.call(arguments))`. In Tokio, aborting a future awaiting a `JoinHandle` from `spawn_blocking` detaches the OS thread; it **cannot** cancel or kill the running blocking closure.
**The Catalyst:** A user presses `Esc` while `exec` (`cargo test`, up to 300s timeout), `test_many` (up to 10 × 600s = 6000s), `patch`, or `write_many` is running, and immediately submits a follow-up message (or the TUI automatically drains its message queue via `drain_queue` at `crates/tupi-tui/src/run.rs:146`).
**The Fallout:** Split-brain workspace corruption and Cargo lock contention. The zombie OS thread from the cancelled turn continues mutating files (`write_many`, `patch`, `edit`, `move`), poisoning `DeliveryLedger` and `GraphCache`, or holding the Cargo `target/` build lock in the background while the newly started turn executes conflicting reads and writes on the exact same repository.

## 2026-11-11 - `git apply` Symlink Escape & Quoted-Path Bypass in `PatchTool`
**The Flaw:** `PatchTool::validate_paths` (`crates/tupi-tools/src/patch.rs:50-82`) only performs lexical string checks (`outside()`: `starts_with('/')` or `..`) on diff headers before invoking `git apply`. Unlike `paths::resolve_for_write`, it never canonicalizes intermediate path components on disk, does not strip git C-style quotes (`"../outside"`), and does not reject patches that create symlinks (`new file mode 120000`) and write through them in a subsequent hunk.
**The Catalyst:** A model (or prompt-injected diff content from `fetch`/`research`) passes a unified diff targeting an existing symlink inside the workspace, a quoted traversal path, or a two-hunk symlink creation + file write.
**The Fallout:** Arbitrary file overwrite outside the workspace root via `git apply`.

## 2026-11-11 - Pre-Sort 5,000-File Truncation in `ExecTool::fingerprint` & Double Full-Graph Rebuild Storm
**The Flaw:** `ExecTool::fingerprint` (`crates/tupi-tools/src/exec.rs:391-434`) breaks out of the directory walk at `entries.len() >= 5_000` *before* sorting `entries`. In any workspace with >5,000 `.rs`/`.toml`/`.lock` files, edits to files visited after the first 5,000 produce an identical fingerprint and return a stale cached `cargo check/test` result. Meanwhile, `GraphCache::repair(&self, _paths: &[PathBuf])` (`crates/tupi-tools/src/graph/cache.rs:65-71`) completely ignores `_paths` and runs a full `O(Calls × Symbols)` rebuild synchronously inside `RepairingTool::call` and a second time 150ms later inside `GraphWatcher`.
**The Catalyst:** Running Tupi on a repository with >5,000 source files or executing rapid `edit`/`write` calls.
**The Fallout:** False-positive green test/check cache hits on broken code, plus quadratic CPU burn and multi-second tool latency on every single mutation.

## 2026-11-11 - Permanent Prefix-Cache Thrashing & Silent Context-Pointer Re-Shrinking
**The Flaw:** Even after the proportional target patch in `App::plan_context` (`crates/tupi-app/src/app.rs:143-151`), `message_target` only scales when `target >= message_budget`. Under the default configuration (`budget: 32_000`, `target: 16_000`) or whenever `target < message_budget`, `fixed_tokens` (`tool_tokens + prefix_tokens + warning_tokens`, typically `6_000–14_000` tokens) is subtracted from `context_budget` to form `message_budget`, while `message_target` is passed **raw and unreduced** (`16_000`). Worse, inside `tupi_domain::context::shrink_old_results` (`crates/tupi-domain/src/context.rs:302-335`), already-shrunk tool outputs (`is_pointer(&message.content)`) and already-shrunk tool call arguments (`is_shrunk_arg(&call.arguments)`) increment `shrunk_count += 1` on every subsequent call, even though zero bytes were mutated!
**The Catalyst:**
1. As `token_scale` adjusts or `fixed_tokens` grows (e.g., `message_budget = 16_500`, `target = 16_000`), the amortization runway (`message_budget - message_target`) collapses from `16_000` tokens down to `500` tokens—triggering a full history trim and breaking the provider's KV prefix cache on almost every single tool step.
2. In any multi-step turn whose single surviving unit exceeds `message_budget` (or whenever `total(messages) > ceiling` after a previous shrink), `shrink_old_results` sees the existing pointers from the previous step and returns `shrunk_results > 0`. `Plan::trimmed()` returns `true`, causing `App::send_message` (`app.rs:502-529`) to overwrite `SessionStore`, emit bogus `Event::ContextTrimmed` notifications to the TUI, and record fake `TelemetryEvent::ContextTrimmed` events on every single step—even when the message list is 100% unchanged.
**The Fallout:** Unbounded KV cache invalidation and telemetry/UI corruption. Instead of cutting total prompt tokens to `context_target` (`16_000`), pruning leaves total prompt tokens at `target + fixed_tokens` (`24_000–31_999`), exhausting the remaining budget in 1–2 tool calls and forcing continuous re-tokenization of the entire conversation history at full uncached provider rates.

## 2026-11-11 - Argument Injection & Arbitrary File Read/Write via `git` Flag Smuggling in `read`, `diff`, and `why`
**The Flaw:** `ReadTool::prepare_rev` (`crates/tupi-tools/src/read.rs:119-149`) passes `format!("{rev}:{relative}")` directly to `git show` without rejecting a leading `-` in `rev` or placing `--` before the revision spec. Similarly, `DiffTool::call` (`crates/tupi-tools/src/diff.rs:125-156`) passes `params.base` directly to `git diff --unified=0 --no-color <base> --` without rejecting a leading `-` or using `--end-of-options`. And `WhyTool::call` (`crates/tupi-tools/src/why.rs:100-112`) interpolates `def.relative` into `git log -L start,end:file` without escaping colons or rejecting malicious filenames.
**The Catalyst:** A model (or prompt-injected instruction from external content) calls:
- `read(path: "Cargo.toml", rev: "--output=/tmp/pwned")` -> `git show --output=/tmp/pwned:Cargo.toml` writes repository contents to an arbitrary filesystem path!
- `diff(base: "--output=/home/user/.bashrc")` -> `git diff --unified=0 --no-color --output=/home/user/.bashrc --` overwrites arbitrary files outside the workspace root, bypassing `ReadTool`/`DiffTool` read-only guarantees!
- `diff(base: "--no-index")` with `path: "/etc/passwd"` -> compares and leaks files outside the repository!
**The Fallout:** A purportedly read-only toolset (`read`, `diff`, `why`) that is not even wrapped in `RepairingTool` or marked `mutates() == true` can overwrite arbitrary files on the host filesystem or exfiltrate sensitive files outside the workspace root.

## 2026-11-11 - Uncancelled Multi-Call/Pipeline Loops & `PatchTool` Staged Index Corruption
**The Flaw:**
1. In `App::send_message` (`crates/tupi-app/src/app.rs:699-793`), when the user cancels a turn (`Esc`), `App::cancel` aborts the outer Tokio task (`task.abort()`) while waiting on `in_flight_count == 0`. However, `pipeline::execute` (`crates/tupi-app/src/pipeline.rs:199-248`) runs inside that task and spawns up to `MAX_PARALLEL = 8` concurrent `run_tool` calls or sequential steps, and never checks `cancel_flag` between steps. Worse, non-process tools (`write_many`, `edit`, `move`, `search`, `map` graph builds) never check `tupi_kernel::is_tool_cancelled()`.
2. In `PatchTool::call` (`crates/tupi-tools/src/patch.rs:256-267`), when `params.check_only == false`, Tupi runs `git apply --check` followed by `git apply --index`.
**The Catalyst:**
1. The model invokes `patch(check_only: false)` on a repository where the user has staged changes in the git index (`git add`), or where a working-tree file differs from `HEAD`/index.
2. `git apply --index` requires the patch to apply cleanly to **both** the working tree and the git index, and stages the applied hunks directly into `.git/index`!
**The Fallout:**
- Every `patch` call silently stages its changes into the user's git index (`.git/index`), polluting any pre-existing staged commit the developer was preparing.
- Worse: if the model first edits a file with `edit` or `write` (which modifies the working tree without updating `.git/index`) and then applies a `patch` to the same file, `git apply --index` **fails** because the index does not match the working tree—breaking `PatchTool` on any file previously touched by Tupi's own `edit` or `write` tools!
