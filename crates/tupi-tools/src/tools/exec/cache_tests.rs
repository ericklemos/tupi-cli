//! Testes da chave do cache de `exec`: exclusões, ambiente, lookup e resumo.
//!
//! Separados de `tests.rs` porque travam o *acerto* e as exclusões, não o
//! formato dos argumentos do cargo.

use super::*;

fn write_file(root: &Path, relative: &str, body: &str) {
    let path = root.join(relative);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(path, body).unwrap();
}

fn key(root: &Path) -> String {
    fingerprint(root, MAX_FINGERPRINT_FILES).expect("chave")
}

fn check() -> ExecParams {
    ExecParams {
        kind: ExecKind::Check,
        package: None,
        target: None,
        filter: None,
        exact: false,
        task: None,
        refresh: false,
        timeout_secs: None,
        budget_tokens: None,
    }
}

fn raw() -> Output {
    Output {
        stdout: String::new(),
        stderr: String::new(),
        success: true,
        timed_out: false,
        duration: Duration::from_millis(10),
    }
}

#[test]
fn the_agent_session_log_stays_out_of_the_key() {
    let dir = tempfile::tempdir().unwrap();
    write_file(dir.path(), "src/main.rs", "fn main() {}");
    let before = key(dir.path());

    write_file(
        dir.path(),
        ".tupi/sessions/x/log.jsonl",
        "{\"tool\":\"exec\"}\n",
    );

    assert_eq!(key(dir.path()), before);
}

#[test]
fn docs_and_agent_results_stay_in_the_key() {
    let dir = tempfile::tempdir().unwrap();
    write_file(dir.path(), "src/main.rs", "fn main() {}");
    write_file(dir.path(), "docs/index.md", "a");
    write_file(dir.path(), ".agents/results/r.md", "a");
    let before = key(dir.path());

    write_file(dir.path(), "docs/index.md", "b");
    let after_docs = key(dir.path());
    assert_ne!(after_docs, before);

    write_file(dir.path(), ".agents/results/r.md", "b");
    assert_ne!(key(dir.path()), after_docs);
}

#[test]
fn the_key_is_stable_between_two_identical_reads() {
    let dir = tempfile::tempdir().unwrap();
    write_file(dir.path(), "src/main.rs", "fn main() {}");
    write_file(dir.path(), ".cargo/config.toml", "[build]\n");
    assert_eq!(key(dir.path()), key(dir.path()));
}

#[test]
fn cache_lookup_hits_only_on_the_same_key_without_refresh() {
    let cache = Some(Cached {
        key: "k".to_string(),
        output: raw(),
        at: std::time::Instant::now(),
    });

    assert!(cache_lookup(&cache, Some("k"), false).is_some());
    assert!(cache_lookup(&cache, Some("outra"), false).is_none());
    assert!(cache_lookup(&cache, None, false).is_none());
    assert!(cache_lookup(&cache, Some("k"), true).is_none());
    assert!(cache_lookup(&None, Some("k"), false).is_none());
}

#[test]
fn a_tool_with_the_cache_off_never_builds_a_key() {
    let dir = tempfile::tempdir().unwrap();
    write_file(dir.path(), "src/main.rs", "fn main() {}");
    let store = Arc::new(ResponseStore::new(Duration::from_secs(60), 1024 * 1024));
    let tool = ExecTool::new(dir.path().to_path_buf(), store, 1_000).with_cache(false);

    let (key, cost, off) = tool.build_key(&["check".to_string()]);

    assert!(key.is_none());
    assert_eq!(cost, Duration::ZERO);
    assert!(off.is_none());
}

#[test]
fn a_disabled_cache_reports_the_reason() {
    for (off, expected) in [
        (CacheOff::Files(5_000), "mais de 5000 arquivos"),
        (CacheOff::Bytes(64 * 1024 * 1024), "mais de 64 MB"),
    ] {
        let note = CacheNote {
            age: None,
            key_cost: Some(Duration::from_millis(2)),
            off: Some(off),
        };
        let body = sections_for(&build_args(&check()).unwrap(), &raw(), note)
            .first()
            .unwrap()
            .body
            .clone();
        assert!(body.contains("cache desligado"), "{body}");
        assert!(body.contains(expected), "{body}");
    }
}

#[test]
fn a_miss_states_the_key_cost_and_the_total() {
    let note = CacheNote {
        age: None,
        key_cost: Some(Duration::from_millis(5)),
        off: None,
    };
    let body = sections_for(&build_args(&check()).unwrap(), &raw(), note)
        .first()
        .unwrap()
        .body
        .clone();

    assert!(body.contains("cache: miss · chave 5.0ms · total 0.0s"), "{body}");
}

// Medição de custo, não de comportamento: roda só sob demanda
// (`cargo test -p tupi-tools --lib fingerprint_cost -- --ignored --nocapture`).
// É somente leitura: nenhum arquivo do repositório é criado ou alterado — o
// caso de ~500 arquivos nasce num `tempfile::tempdir()`.
#[test]
#[ignore = "mede walk+blake3 do fingerprint; roda sob demanda"]
fn fingerprint_cost() {
    fn measure(label: &str, root: &Path) {
        let walk_started = std::time::Instant::now();
        let entries = fingerprint_entries(root, MAX_FINGERPRINT_FILES);
        let walked = walk_started.elapsed();
        let entries = match entries {
            Ok(entries) => entries,
            Err(off) => {
                println!(
                    "{label}: {} — walk custou {walked:.2?}, cache desligado",
                    off.reason()
                );
                return;
            }
        };
        let bytes: u64 = entries.iter().map(|(_, length)| *length).sum();

        let hash_started = std::time::Instant::now();
        let digest = fingerprint_digest(&entries, "RUSTFLAGS=\n").expect("hash");
        let hashed = hash_started.elapsed();

        println!(
            "{label}: {} arquivos, {:.1} MB — walk {walked:.2?}, hash {hashed:.2?}, total {:.2?} (chave {})",
            entries.len(),
            bytes as f64 / (1024.0 * 1024.0),
            walked + hashed,
            &digest[..8],
        );
    }

    let repo = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("raiz do workspace");
    measure("repo real", &repo);

    let small = tempfile::tempdir().expect("tempdir");
    for index in 0..500u32 {
        let body = vec![b'x'; 4 * 1024];
        std::fs::write(small.path().join(format!("f{index}.rs")), body).expect("fixture");
    }
    measure("repo de ~500 arquivos", small.path());
}
use super::render::{CacheOff, fingerprint_digest, fingerprint_entries};

#[test]
#[ignore = "custa um cargo check real; roda sob demanda"]
fn two_identical_calls_hit_the_cache_and_a_source_change_misses() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("Cargo.toml"),
        "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    std::fs::create_dir_all(dir.path().join("src")).unwrap();
    std::fs::write(dir.path().join("src/lib.rs"), "pub fn a() -> u8 { 1 }\n").unwrap();

    let store = Arc::new(ResponseStore::new(Duration::from_secs(60), 1024 * 1024));
    let tool = ExecTool::new(dir.path().to_path_buf(), store, 4_000);
    let args = serde_json::json!({ "kind": "check" });

    // Aquecimento: o primeiro `cargo check` escreve `Cargo.lock` e `target/`.
    // O `target/` está fora da chave, o `Cargo.lock` não — então a primeira
    // rodada é *miss* por construção e a comparação começa na segunda.
    let _ = tool.call(args.clone()).expect("aquecimento");

    let second = tool.call(args.clone()).expect("segunda chamada");
    assert!(!second.contains("em cache há"), "{second}");
    assert!(second.contains("cache: miss"), "{second}");

    let third = tool.call(args.clone()).expect("terceira chamada");
    assert!(third.contains("em cache há"), "{third}");
    assert!(third.contains("cache: hit"), "{third}");

    std::fs::write(dir.path().join("src/lib.rs"), "pub fn a() -> u8 { 2 }\n").unwrap();
    let fourth = tool.call(args).expect("quarta chamada");
    assert!(!fourth.contains("em cache há"), "{fourth}");
    assert!(fourth.contains("cache: miss"), "{fourth}");
}
