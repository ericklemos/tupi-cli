//! O perfil default (Exposure::Core) é o que roda no modo exec do TUI.
//!
//! Ele precisa conter pelo menos um escritor genérico: edit só troca corpo
//! de símbolo Rust, então sem write/patch arquivos como .md, .toml e .json
//! ficam somente leitura.

use tupi_kernel::{AgentMode, ToolRegistry};

fn core_registry(dir: &std::path::Path, state: &std::path::Path) -> ToolRegistry {
    let mut registry = ToolRegistry::new();
    tupi_tools::register_exposed_with(
        dir.to_path_buf(),
        &mut registry,
        tupi_tools::Exposure::Core,
        tupi_tools::ToolsOptions::hermetic(state.to_path_buf()),
    );
    registry
}

fn names(registry: &ToolRegistry, mode: AgentMode) -> Vec<String> {
    registry
        .specs_for_mode(mode)
        .into_iter()
        .map(|spec| spec.name)
        .collect()
}

#[test]
fn core_exposure_can_write_and_patch_non_rust_files() {
    let dir = tempfile::tempdir().unwrap();
    let state = tempfile::tempdir().unwrap();
    let registry = core_registry(dir.path(), state.path());

    assert!(
        registry.has("clock"),
        "perfil Core sem clock: a data vira palpite"
    );
    assert!(registry.has("edit"), "perfil Core sem edit");
    assert!(
        registry.has("write"),
        "perfil Core sem write: arquivo novo/.md fica impossível"
    );
    assert!(
        registry.has("patch"),
        "perfil Core sem patch: editar trecho de .md vira reescrever tudo"
    );

    let exec = names(&registry, AgentMode::Execution);
    for required in ["edit", "write", "patch"] {
        assert!(
            exec.iter().any(|name| name == required),
            "modo exec sem {required}: {exec:?}"
        );
    }

    // O modo SDD continua fail-closed: mutadores de workspace ficam de fora.
    let sdd = names(&registry, AgentMode::Sdd);
    for forbidden in ["edit", "write", "patch"] {
        assert!(
            !sdd.iter().any(|name| name == forbidden),
            "modo SDD não deveria permitir {forbidden}: {sdd:?}"
        );
    }
}

#[test]
fn core_write_creates_a_markdown_file() {
    let dir = tempfile::tempdir().unwrap();
    let state = tempfile::tempdir().unwrap();
    let registry = core_registry(dir.path(), state.path());

    let write = registry.get("write").expect("write ausente do perfil Core");
    write
        .call(serde_json::json!({
            "path": "nota.md",
            "content": "# nota\n"
        }))
        .unwrap();

    let saved = std::fs::read_to_string(dir.path().join("nota.md")).unwrap();
    assert_eq!(saved, "# nota\n");
}

#[test]
fn core_exposure_exposes_feedback_in_every_mode() {
    let dir = tempfile::tempdir().unwrap();
    let state = tempfile::tempdir().unwrap();
    let registry = core_registry(dir.path(), state.path());

    // SDD e docs sofrem o mesmo atrito da execução: o perfil mínimo precisa
    // carregar o feedback em todos os modos.
    assert!(registry.has("feedback"), "perfil Core sem feedback");
    assert!(
        registry.get("feedback").is_some(),
        "feedback registrado mas não exposto pelo perfil Core"
    );
    for mode in [AgentMode::Execution, AgentMode::Sdd] {
        let list = names(&registry, mode);
        assert!(
            list.iter().any(|name| name == "feedback"),
            "modo {mode:?} sem feedback: {list:?}"
        );
    }
}
