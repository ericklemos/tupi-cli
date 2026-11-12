# 🤨 Skeptic: [Logical Flaw in Context Pruning, Tool Cancellation, Exec Caching, and Patch Validation]

You built an entire architecture around a single commandment in `AGENTS.md`: *"Economia de tokens é requisito, não otimização. O histórico é dívida."*
And yet the very machinery designed to enforce token economy contains a mathematical contradiction that destroys provider prefix caching and traps the model in an unrecoverable amnesia loop.

Below is the surgical dissection of the four most catastrophic failure modes currently sitting in `HEAD`.

---

## Target #1: The Prefix-Blind Target & Shrink-Induced Amnesia Spiral
**Locations:** `crates/tupi-app/src/app.rs:105-122, 439-461`, `crates/tupi-domain/src/context.rs:74-138, 284-340`, `crates/tupi-tools/src/ledger.rs:130-185`

### 💥 The Catastrophe: What Will Break
1. **Amortized pruning silently disables itself under normal token scaling**, causing the context pruner to trim on *every single tool step*, shifting the message array head and **100% invalidating the LLM provider's KV prefix cache on every request**.
2. **The Context Pruner and the `DeliveryLedger` are completely blind to each other.** When `shrink_old_results` erases a tool output from `SessionStore` and replaces it with a pointer instructing the model to call the tool again, `DeliveryLedger` still believes the model has the full text in context and refuses to return it—creating an unrecoverable livelock of wasted tool steps.

### 🕵️ The Blindspot: Step-by-Step Mathematical Proof

#### Flaw A: The `context_target` Math Bug (`crates/tupi-app/src/app.rs:105-122`)
Look at how `plan_context` computes `message_budget` vs `message_target`:
```rust
let message_budget = context_budget.saturating_sub(tool_tokens).max(1);
let message_target = match context_target {
    0 => message_budget.saturating_mul(2) / 5,
    target => target.min(message_budget),
};
```
Here `tool_tokens` is actually `fixed_tokens = request_tool_tokens + prefix_tokens + warning_tokens` (`app.rs:442-444`).
In `configs/context.ron`, `budget = 32000` and `target = 16000`.
With 26 registered tools + system prompt + workflow catalog, `fixed_tokens` is roughly **8,000–10,000 tokens**.
- `message_budget` becomes `32,000 - 10,000 = 22,000`.
- `message_target` is **not** reduced by `fixed_tokens`; it is merely `16,000.min(22,000) = 16,000`.
- Now suppose the provider charges 1.4x your heuristic estimate (`token_scale = 1,400,000` ppm):
  - `scaled_budget(32000, 1.4) = 22,857`.
  - `scaled_budget(16000, 1.4) = 11,428`.
  - Subtract `fixed_tokens` (`11,500`): `message_budget = 11,357`.
  - `message_target = 11,428.min(11,357) = 11,357`.
- Now look at `plan_amortized_with` (`crates/tupi-domain/src/context.rs:133-138`):
```rust
if ceiling == 0 || total(messages) <= ceiling || target >= ceiling {
    return plan_with(messages, ceiling, recent_results);
}
```
Because `message_target == message_budget` (`target >= ceiling`), **amortized pruning is completely bypassed!** Instead of cutting deep once and letting the cached prefix survive for the next 15 steps, `plan_with` cuts to `ceiling` on step N, exceeds `ceiling` again on step N+1, drops another unit from the front of the history on step N+1, and **invalidates the entire prompt prefix cache on every single step**.

#### Flaw B: The Context Pruner vs. `DeliveryLedger` Deadlock
Trace this exact sequence during a single complex turn (`units(&kept) == 1`):
1. Step 1–4: The model calls `read` on `SymbolA`. `DeliveryLedger` records `(path, "SymbolA", file_hash, complete: true)`.
2. Step 5–8: The model calls 4 more tools. `total(messages)` crosses `message_budget`.
3. Step 9: `plan_context` runs `plan_with`. Inside `plan_with`, `shrink_old_results` runs **before** checking if any turn units can be dropped.
4. `SymbolA`'s tool result (from Step 1, now older than `recent_results: 3`) is mutated into:
   `[resultado anterior omitido na compactação de contexto: ~1200 tokens. Chame a tool de novo se precisar do conteúdo — o ledger devolve ponteiro ou diff.]`
5. In `App::send_message` (`app.rs:455-460`):
   `planned.trimmed()` is `true` because `shrunk_results > 0`.
   `session.replace_messages(planned.messages.clone())` **permanently overwrites `SessionStore`**, destroying the original tool output!
6. Step 10: The model needs `SymbolA` to write an edit. It obeys the pointer and calls `read(path, "SymbolA")` (without `repeat: "allow"`, because the pointer told it *"o ledger devolve ponteiro ou diff"*).
7. `ReadTool::symbol_sections` (`crates/tupi-tools/src/read.rs:328`) queries `DeliveryLedger::check`.
8. `DeliveryLedger` has **no idea** that `context.rs` deleted `SymbolA` from the conversation history! It returns `Verdict::Seen`:
   `> **Já entregue nesta sessão** (read · blake3 ...) — corpo em peek h_12345678.`
9. The model now has to spend *another* step calling `peek(h_12345678)`—or if it calls `read` again, it burns 3 full LLM round-trips before `MAX_OMISSIONS` finally forces `Verdict::Full`.

### 💸 The Cost
- **Direct financial burn:** Busting DeepSeek's KV cache on a 25k-token context across a 50-step turn multiplies input token costs by up to **10x** (cache miss vs cache hit pricing) while adding 2–4 extra round-trips whenever a pruned result is needed again.

### 🛡️ The Ultimatum
1. `plan_context` must either scale `message_target` proportionally to `message_budget` (`message_budget * context_target / context_budget`) or subtract `fixed_tokens` from `context_target` so `target < ceiling` holds invariant.
2. When `shrink_old_results` evicts a tool result from the active context, either:
   - The pointer message must instruct the model to pass `repeat: "allow"`, OR
   - `DeliveryLedger` must be notified to evict/downgrade entries whose tool outputs were pruned from `SessionStore`.

---

## Target #2: Zombie `spawn_blocking` Threads After Turn Cancellation
**Locations:** `crates/tupi-app/src/app.rs:20-31, 797-840`, `crates/tupi-tui/src/run.rs:135-149`

### 💥 The Catastrophe: What Will Break
Look at the comment in `App::cancel` (`crates/tupi-app/src/app.rs:812-816`):
```rust
// Espera a task parar de verdade: depois disso ela não escreve mais no
// histórico, e um novo turno pode começar com o histórico consistente.
if let Some(task) = task {
    task.abort();
    let _ = task.await;
}
```
**That assumption breaks the moment a tool is executing.**
`task.abort()` only drops the async state machine at its current `.await` point. When a tool is running, that `.await` point is inside `run_tool` (`crates/tupi-app/src/app.rs:27`):
```rust
tokio::task::spawn_blocking(move || tool.call(arguments)).await
```
In Tokio, aborting a task that is awaiting a `spawn_blocking` `JoinHandle` **does not stop the blocking OS thread**. The thread continues executing `tool.call(arguments)` to completion in the background!

### 🕵️ The Blindspot: The Race Condition Sequence
1. Turn 1 starts a heavy or mutating tool call: `exec` (`cargo test`), `test_many`, `patch`, `write_many`, or a `pipeline` step running 4 parallel tools.
2. The user types a follow-up message while Turn 1 is running (which the TUI queues in `state.enqueue(&input)` at `crates/tupi-tui/src/run.rs:95`), and then presses `Esc` to cancel Turn 1.
3. `App::cancel` aborts the outer Tokio task, awaits the `JoinError::Cancelled` (which returns in microseconds while the `spawn_blocking` thread is still alive!), sets `self.active = 0`, and returns `CommandOutcome::Cancelled`.
4. Immediately on the next line (`crates/tupi-tui/src/run.rs:146`), the TUI executes:
   `request = drain_queue(&buses.commands, &mut state).await;`
5. **Turn 2 starts immediately** while Turn 1's `spawn_blocking` thread (and its child `cargo` / `git apply` / `write_many` process) is still actively mutating the filesystem, writing to `DeliveryLedger`, and rebuilding `GraphCache`!

### 💸 The Cost
- Concurrent `write` / `edit` / `patch` operations corrupting workspace files or failing hash checks unpredictably.
- Concurrent `cargo` invocations deadlocking on `target/.cargo-lock` until the 300-second timeout expires.

### 🛡️ The Ultimatum
1. Pass a cancellation token (`Arc<AtomicBool>`) into `Tool::call` / `proc::run` so `proc::run`'s 20ms poll loop (`crates/tupi-tools/src/proc.rs:76`) immediately invokes `kill_tree(&mut child)` when the turn is cancelled.
2. Track active `spawn_blocking` tool handles in `App` and await their actual termination in `App::cancel` before releasing `self.active` and before allowing `drain_queue` to start the next turn.

---

## Target #3: Silent Cache Poisoning in `ExecTool::fingerprint` & Double Graph Rebuilds
**Locations:** `crates/tupi-tools/src/exec.rs:391-434`, `crates/tupi-tools/src/graph/cache.rs:65-71`, `crates/tupi-tools/src/graph/mod.rs:55-66`, `crates/tupi-tools/src/graph/watcher.rs:76`

### 💥 The Catastrophe: What Will Break
1. **`ExecTool` returns stale "build/test succeeded" results on broken code** in any repository with more than 5,000 files.
2. **`GraphCache::repair` is a fake incremental update** that rebuilds the entire repository graph twice on every single file edit.

### 🕵️ The Blindspot: The Math Doesn't Hold at Scale

#### Flaw A: Pre-Sort Truncation in `ExecTool::fingerprint` (`crates/tupi-tools/src/exec.rs:419-424`)
```rust
entries.push((path.to_path_buf(), metadata.len(), modified));
if entries.len() >= 5_000 {
    break;
}
...
entries.sort();
```
`WalkBuilder::build()` yields directory entries in filesystem / inode order, and you `break` at 5,000 files **before** sorting!
- If a repository has 5,200 `.rs`/`.toml` files, 200 files are **completely invisible** to `fingerprint()`.
- When the model edits one of those 200 files and runs `exec(kind: "check")` or `exec(kind: "test")`, `fingerprint(&root)` produces the **exact same blake3 hash** as before the edit.
- `ExecTool::call` (`crates/tupi-tools/src/exec.rs:503-511`) hits its cache and returns `cached: true` with the previous run's output—lying to the model that its new code compiled and passed tests without ever running `cargo`!

#### Flaw B: The Double Full-Rebuild in `GraphCache`
Look at `GraphCache::repair` (`crates/tupi-tools/src/graph/cache.rs:65-71`):
```rust
pub fn repair(&self, _paths: &[PathBuf]) {
    let rebuilt = Arc::new(self.build());
    *self.graph.write().unwrap_or_else(|error| error.into_inner()) = Some(rebuilt);
}
```
1. `_paths` is unused! Every call to `repair` walks the entire repository and runs `GraphBuilder::build`.
2. Inside `GraphBuilder::build` (`crates/tupi-tools/src/graph/builder.rs:146-214`), for every file and every call site, it iterates linearly over `graph.graph.node_indices()` to find matching symbol definitions (`O(Calls * Symbols)`).
3. When `EditTool` or `WriteTool` finishes, `RepairingTool::call` (`crates/tupi-tools/src/graph/mod.rs:61`) calls `self.cache.repair(&paths)` synchronously on the tool thread.
4. 100–150ms later, the background `GraphWatcher` (`crates/tupi-tools/src/graph/watcher.rs:76`) wakes up from the OS `notify` event for that exact same write and calls `cache.repair(&paths)` **a second time**!

### 💸 The Cost
- False-positive green test suites in large workspaces (corrupting agent verification).
- Multi-second latency added to every single `edit`/`write` call as the repo scales.

### 🛡️ The Ultimatum
1. In `ExecTool::fingerprint`, if the walk hits the 5,000-file cap, do **not** cache the `ExecTool` result.
2. In `GraphCache`, either implement true file-scoped incremental repair in `repair(paths)` or mark the graph dirty (`invalidate()`) on mutation and rebuild lazily on the next `map`/`context_graph`/`impact_check` query instead of rebuilding eagerly on every edit + watcher tick.

---

## Target #4: `PatchTool` Symlink/Quote Escape & `web::curl` Redirect SSRF
**Locations:** `crates/tupi-tools/src/patch.rs:46-82, 193-225`, `crates/tupi-tools/src/web.rs:19-47`

### 💥 The Catastrophe: What Will Break
- **Arbitrary file write outside the repository root** via `patch`.
- **Local network / cloud metadata / `file://` redirect SSRF** via `fetch` and `research`.

### 🕵️ The Blindspot
1. `WriteTool`, `EditTool`, and `MoveTool` all use `paths::resolve_for_write` (`crates/tupi-tools/src/paths.rs:53-101`), which canonicalizes existing parent directories to ensure a symlink inside the workspace cannot point outside `root`.
2. `PatchTool::validate_paths` (`crates/tupi-tools/src/patch.rs:50-82`) **bypasses `paths::resolve_for_write` completely**. It only runs:
   ```rust
   fn outside(path: &str) -> bool {
       path.starts_with('/') || path.split('/').any(|component| component == "..")
   }
   ```
   - **Symlink traversal:** If `sub/link` is an existing symlink inside the repo pointing outside `root` (or if Hunk 1 creates `new file mode 120000` pointing to `/outside` and Hunk 2 writes through it), `outside("sub/link/file")` returns `false`, and `git apply` writes outside the root.
   - **Quoted path bypass:** `git apply` supports C-quoted paths in headers (e.g., `--- "a/../outside"` or `+++ "/tmp/pwn"`). Because `validate_paths` does not strip leading quotes before calling `outside(path)`, `"/tmp/pwn".starts_with('/')` is **`false`**, and `"".."` is not `".."`!
3. In `crates/tupi-tools/src/web.rs:19-47`, `curl` is invoked with `-sSL` on any string starting with `http://` or `https://`.
   - It allows `http://127.0.0.1:...`, `http://localhost:...`, `http://169.254.169.254/latest/meta-data/`, and `http://[::1]/`.
   - Because `-L` follows redirects without `--proto-redir =http,https`, a remote HTTP server can 302-redirect `curl` to `file:///etc/passwd` or internal services!

### 🛡️ The Ultimatum
1. In `PatchTool::validate_paths`, reject any diff containing quoted paths or `120000` symlink modes, strip `a/` and `b/` prefixes, and pass every touched path through `paths::resolve_for_write(&root, path)` before calling `git apply`.
2. In `web::curl`, pass `--proto =http,https --proto-redir =http,https` to `curl` and reject loopback, link-local, and private IP literals/hosts before spawning `curl`.