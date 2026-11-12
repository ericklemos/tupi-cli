# 🤨 Skeptic Failure Analysis Report — Wave 2 Audit (`tupi-cli`)

**Date:** 2026-11-11
**Agent:** The Skeptic 🤨 (🦔 Hedgehog)
**Scope:** Post-fix verification of Wave 1 remediations and deep adversarial audit of `tupi-app`, `tupi-domain`, `tupi-tools`, and `tupi-providers`.

---

## Executive Verdict

You patched the surface of the six disasters I reported earlier, and in doing so, you introduced **three new structural failure modes** and left a gaping arbitrary-file-write vulnerability inside your "read-only" git tools.

Here is the surgical breakdown of how your current implementation will fail in production.

---

## 💥 Catastrophe 1: Argument Injection & Arbitrary Host File Overwrite via `git` Flag Smuggling (`DiffTool` & `ReadTool`)

### 🕵️ The Blindspot
In `crates/tupi-tools/src/diff.rs:125-156`, `DiffTool::call` constructs the `git diff` invocation as:
```rust
let mut args = vec![
    "diff".to_string(),
    "--unified=0".to_string(),
    "--no-color".to_string(),
    base.to_string(),
    "--".to_string(),
];
```
And in `crates/tupi-tools/src/read.rs:141-145`, `ReadTool::prepare_rev` runs:
```rust
let output = crate::proc::program(
    "git",
    &["show".to_string(), format!("{rev}:{relative}")],
    &root,
    std::time::Duration::from_secs(30),
)?;
```
Neither `base` nor `rev` is validated to reject leading dashes (`-`), nor is `--end-of-options` passed before positional revision arguments.

### 🔨 Step-by-Step Destruction
1. The model processes untrusted content (via `fetch`, `research`, or a malicious repo file) or hallucinates a flag in `base` or `rev`.
2. It calls `diff({ "base": "--output=/home/user/.bashrc" })`.
3. `git diff` interprets `--output=/home/user/.bashrc` as an option (because it appears **before** `--`), writing the diff output directly to `/home/user/.bashrc` and clobbering the user's shell configuration outside the workspace root.
4. Or it calls `read({ "path": "Cargo.toml", "rev": "--output=/tmp/exfil" })`. `git show` parses `--output=/tmp/exfil:Cargo.toml` as an option flag, writing to disk inside a tool that claims to be strictly read-only.

### 🛡️ The Ultimatum
- Reject any `rev` or `base` starting with `-` (or containing NUL/control characters) in `ReadTool::prepare_rev` and `DiffTool::call`.
- Pass `--end-of-options` before `base` / `rev` in all `git` invocations (`read`, `diff`, `why`).

---

## 💥 Catastrophe 2: `PatchTool` Corrupts the User's Git Index and Breaks After Any `edit`/`write` (`crates/tupi-tools/src/patch.rs:256-267`)

### 🕵️ The Blindspot
Look at `crates/tupi-tools/src/patch.rs:256-267`:
```rust
let check = proc::program("git", &check_args, &root, Duration::from_secs(30))?;
if !check.success {
    return Err(ToolError::Other(failed_output(&check)));
}

if !params.check_only {
    let mut apply_args = vec!["apply".to_string(), "--index".to_string()];
    if params.reverse {
        apply_args.push("-R".to_string());
    }
    apply_args.push(patch_arg);
```
Why is `check_args` running `git apply --check` (against the working tree only), while `apply_args` runs `git apply --index` (against **both** the working tree and `.git/index`)?

### 🔨 Step-by-Step Destruction
1. **Self-Sabotage Across Tools:** In step 1 of a turn, the model uses `edit` or `write` to modify `src/lib.rs`. Neither `edit` nor `write` updates `.git/index`, so the working tree now diverges from the staged index. In step 2, the model uses `patch` to apply a unified diff to `src/lib.rs`. `git apply --check` **succeeds** (because it only checks the working tree), and then `git apply --index` **fails** with `error: src/lib.rs: does not match index`!
2. **Silent Index Pollution:** Even when `src/lib.rs` was clean, `git apply --index` stages all patched files into `.git/index`. If the human developer had carefully staged partial hunks (`git add -p`) before running Tupi, `PatchTool` silently contaminates their staged commit.

### 🛡️ The Ultimatum
- Remove `"--index"` from `apply_args` in `crates/tupi-tools/src/patch.rs:265`. `PatchTool` must operate on the working tree just like `edit`, `write`, and `write_many`.

---

## 💥 Catastrophe 3: The Unscaled `message_target` Cliff & Infinite `shrunk_count` Re-Trimming Loop

### 🕵️ The Blindspot
1. **The Cliff in `App::plan_context` (`crates/tupi-app/src/app.rs:143-151`):**
```rust
let message_budget = context_budget.saturating_sub(tool_tokens).max(1);
let message_target = match context_target {
    0 => message_budget.saturating_mul(2) / 5,
    target if target < message_budget => target,
    target => {
        ((message_budget as u128 * target.min(context_budget) as u128) / context_budget as u128)
            as usize
    }
};
```
Look at the discontinuity at `target == message_budget`:
- If `context_budget = 32_000`, `context_target = 16_000`, and `fixed_tokens = 15_999`:
  `message_budget = 16_001`. Because `target (16_000) < message_budget (16_001)`, `message_target` is `16_000`! The amortization margin (`message_budget - message_target`) is **1 single token**. One tool call later, it prunes again, destroying the KV prefix cache on every step.
- Add 2 tokens to `fixed_tokens` (`16_001`), so `message_budget = 15_999`: suddenly `message_target` jumps from `16_000` to `7_999`!
- Why is `context_target` (a target for the **entire context**, or proportional to `context_budget`) only scaled when `target >= message_budget` instead of always scaling proportionally (`message_budget * target / context_budget`) or subtracting `tool_tokens`?

2. **Non-Idempotent `shrunk_count` in `shrink_old_results` (`crates/tupi-domain/src/context.rs:302-335`):**
```rust
for &index in tool_indices.iter().take(shrinkable) {
    let message = &mut kept[index];
    if is_pointer(&message.content) {
        shrunk_count += 1;
        continue;
    }
```
And similarly for `is_shrunk_arg(&call.arguments)`:
```rust
    if is_shrunk_arg(&call.arguments) {
        shrunk_count += 1;
        continue;
    }
```
When a message is **already** a pointer from an earlier trim, `shrink_old_results` does **not** modify it, yet it increments `shrunk_count += 1`!
Consequently, whenever `total(messages) > ceiling` and no whole unit can be dropped (e.g., a long multi-step single turn where `units(&kept) == 1`), `plan_with` returns `shrunk_results > 0` and `omitted_units == 0`. `Plan::trimmed()` returns `true` even though `planned.messages == session.messages()`!
In `App::send_message` (`app.rs:502-538`):
- `session.replace_messages(planned.messages.clone())` runs on every step.
- `Event::ContextTrimmed` and `TelemetryEvent::ContextTrimmed` fire on every single step for the rest of the turn, falsely claiming `shrunk_results` pointers were just shrunk when nothing changed.

### 🛡️ The Ultimatum
- In `shrink_old_results`, only increment `shrunk_count` when a message or argument is **actually mutated** during that call (do not increment inside `if is_pointer` or `if is_shrunk_arg`).
- In `App::plan_context`, scale `context_target` proportionally across the entire domain (`((message_budget as u128 * target.min(context_budget) as u128) / context_budget as u128) as usize`), so the amortization runway is always proportional to `message_budget` without a 2x cliff at `target == message_budget`.

---

## 💥 Catastrophe 4: SSRF Redirect & DNS Rebinding Bypass in `web::curl` (`crates/tupi-tools/src/web.rs:19-85`)

### 🕵️ The Blindspot
In `crates/tupi-tools/src/web.rs:19-85`, you added string-level checks on the initial URL's host before invoking `curl -sSL`:
1. **Redirect Bypass (`-L`):** An attacker-controlled URL `https://evil.example/redirect` passes your Rust host check (`host == "evil.example"`). `curl -L` fetches it, receives `HTTP 302 Found -> Location: http://169.254.169.254/latest/meta-data/iam/security-credentials/` or `http://127.0.0.1:8080/admin`, and **`curl` follows the redirect directly to localhost/metadata** without Rust ever seeing the redirect target!
2. **DNS Rebinding / nip.io Bypass:** `http://127.0.0.1.nip.io/` or `http://localtest.me/` passes your string check (`!host.ends_with(".localhost")`, not an IP literal) and resolves in `curl` directly to `127.0.0.1`.

### 🛡️ The Ultimatum
- Resolve the hostname via `std::net::ToSocketAddrs` in Rust and verify that **every** resolved `IpAddr` is global/non-private before invoking `curl`, **and** either pin the resolved IP via `curl --resolve host:port:ip` with redirects disabled (or handle redirects in Rust so every hop's DNS resolution is validated).
