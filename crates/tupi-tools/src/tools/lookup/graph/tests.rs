use super::*;

fn write(dir: &std::path::Path, relative: &str, content: &str) {
    let path = dir.join(relative);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, content).unwrap();
}

fn fixture() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path(),
        "src/a.rs",
        "pub fn a() { b(); }\npub fn b() { c(); }\npub fn c() {}\n",
    );
    dir
}

fn rendered(items: Vec<Item>) -> String {
    items
        .into_iter()
        .map(|item| format!("{}\n{}", item.section.heading, item.section.body))
        .collect::<Vec<String>>()
        .join("\n")
}

#[test]
fn one_hop_shows_callers_and_callees() {
    let dir = fixture();
    let out = rendered(sections(dir.path(), Some("b"), None, None, 240).unwrap());
    assert!(out.contains("chamadores"), "{out}");
    assert!(out.contains("a (src/a.rs"), "{out}");
    assert!(out.contains("consumidores"), "{out}");
    assert!(out.contains("c (src/a.rs"), "{out}");
}

#[test]
fn two_hops_reach_the_grandparent() {
    let dir = fixture();
    let out = rendered(sections(dir.path(), Some("c"), None, Some(2), 240).unwrap());
    assert!(out.contains("nível 2"), "{out}");
    assert!(out.contains("a (src/a.rs"), "{out}");
}

#[test]
fn a_file_target_uses_its_symbols_as_roots() {
    let dir = fixture();
    let out = rendered(sections(dir.path(), None, Some("src/a.rs"), None, 240).unwrap());
    assert!(out.contains("3 raiz(es)"), "{out}");
}

#[test]
fn an_unknown_target_is_refused() {
    let dir = fixture();
    let error = match sections(dir.path(), Some("nao_existe"), None, None, 240) {
        Ok(_) => panic!("deveria recusar alvo desconhecido"),
        Err(error) => error,
    };
    let message = format!("{error:?}");
    assert!(message.contains("não encontrado no grafo"), "{message}");
}

#[test]
fn the_op_accepts_path_without_symbol() {
    let params: super::super::params::LookupParams = serde_json::from_value(serde_json::json!({
        "ops": [{ "kind": "graph", "path": "src/a.rs" }]
    }))
    .expect("path sozinho deveria bastar no graph");

    match params.single() {
        super::super::params::LookupOp::Graph { symbol, path, .. } => {
            assert!(symbol.is_none());
            assert_eq!(path.as_deref(), Some("src/a.rs"));
        }
        other => panic!("esperava graph, veio {other:?}"),
    }
}
