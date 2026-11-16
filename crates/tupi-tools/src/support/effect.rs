//! Conjunto de escrita declarado pelas tools.
//!
//! A extração das chaves de argumento é a mesma para todas: `path`, `from`,
//! `to`, `writes[].path` e os cabeçalhos `+++`/`---` de `patch`. Tool que grava
//! por fora desse vocabulário (`shell`, `exec`, `test_many`) declara
//! [`Effect::Opaque`] — nunca um conjunto vazio, porque "muta e não declara
//! nada" não é distinguível de "não mudou nada".

use std::path::PathBuf;

use serde_json::Value;
use tupi_kernel::Effect;

/// Caminhos que a call declara alterar, no vocabulário acima.
pub(crate) fn declared_paths(arguments: &Value) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    let mut push = |path: &str| {
        let candidate = PathBuf::from(path);
        if !candidate.as_os_str().is_empty() && !paths.contains(&candidate) {
            paths.push(candidate);
        }
    };

    for key in ["path", "from", "to"] {
        if let Some(path) = arguments.get(key).and_then(Value::as_str) {
            push(path);
        }
    }

    if let Some(writes) = arguments.get("writes").and_then(Value::as_array) {
        for write in writes {
            if let Some(path) = write.get("path").and_then(Value::as_str) {
                push(path);
            }
        }
    }

    if let Some(patch) = arguments.get("patch").and_then(Value::as_str) {
        for line in patch.lines() {
            if let Some(rest) = line
                .strip_prefix("+++ ")
                .or_else(|| line.strip_prefix("--- "))
            {
                let raw = rest.split('\t').next().unwrap_or(rest).trim();
                if raw != "/dev/null" && !raw.is_empty() {
                    let relative = raw
                        .strip_prefix("b/")
                        .or_else(|| raw.strip_prefix("a/"))
                        .unwrap_or(raw);
                    push(relative);
                }
            }
        }
    }

    paths
}

/// Escrita declarada, ou [`Effect::Opaque`] quando a call não declara caminho.
pub(crate) fn writes(arguments: &Value) -> Effect {
    let paths = declared_paths(arguments);
    if paths.is_empty() {
        Effect::Opaque
    } else {
        Effect::Writes(paths)
    }
}

/// `true` quando `arguments.action` é uma das ações que só leem.
pub(crate) fn is_read_action(arguments: &Value, read: &[&str]) -> bool {
    arguments
        .get("action")
        .and_then(Value::as_str)
        .is_some_and(|action| read.contains(&action))
}
