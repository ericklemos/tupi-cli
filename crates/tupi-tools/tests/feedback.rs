//! O sink de atrito do modelo (`.agents/tupi/debug/feedback.jsonl`) visto pelo
//! perfil que o agente principal usa de verdade: `Exposure::Core`.
//!
//! O arquivo é log local do mantenedor: nada sai da máquina e nada dele entra
//! no índice de espaço, no grafo ou no prompt.

use std::sync::Arc;

use tupi_domain::memory;
use tupi_kernel::{AgentMode, Tool, ToolRegistry};

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

fn feedback(registry: &ToolRegistry) -> Arc<dyn Tool> {
    registry
        .get("feedback")
        .expect("feedback ausente do perfil Core")
}

#[test]
fn core_exposes_feedback_in_execution_and_sdd() {
    let dir = tempfile::tempdir().unwrap();
    let state = tempfile::tempdir().unwrap();
    let registry = core_registry(dir.path(), state.path());

    // O perfil mínimo é o padrão: sem feedback no núcleo o atrito do modelo
    // não teria para onde ir.
    assert!(registry.has("feedback"), "perfil Core sem feedback");
    for mode in [AgentMode::Execution, AgentMode::Sdd] {
        let specs = registry.specs_for_mode(mode);
        assert!(
            specs.iter().any(|spec| spec.name == "feedback"),
            "modo {mode:?} sem feedback: {:?}",
            specs.iter().map(|spec| &spec.name).collect::<Vec<_>>()
        );
    }
}

#[test]
fn record_list_and_dedupe_go_through_the_core_registry() {
    let dir = tempfile::tempdir().unwrap();
    let state = tempfile::tempdir().unwrap();
    let registry = core_registry(dir.path(), state.path());
    let tool = feedback(&registry);

    // Uma sessão aberta com a dívida de feedback pendente, como o begin_turn faz.
    let session = "s1";
    let pending = format!("{session}:{}", memory::FEEDBACK_ID);
    let mut queue = memory::pendings(dir.path());
    memory::open_pending(&mut queue, &pending, memory::FEEDBACK_KIND, session);
    memory::save_pendings(dir.path(), &queue).unwrap();

    let first = tool
        .call(serde_json::json!({
            "action": "record",
            "text": "shell recusou grep",
            "kind": "refusal",
            "tool": "shell",
            "workaround": "search"
        }))
        .unwrap();
    assert!(first.contains("feedback — registrado"), "{first}");
    assert!(first.contains("(tool: shell)"), "{first}");
    assert!(first.contains("→ search"), "{first}");

    let path = memory::feedback_path(dir.path());
    assert!(
        path.ends_with(".agents/tupi/debug/feedback.jsonl"),
        "{}",
        path.display()
    );
    assert!(path.exists(), "sink não criado em {}", path.display());

    // A pendência da sessão foi paga: o registro é a resposta ao atrito.
    assert!(!memory::is_open(&memory::pendings(dir.path()), &pending));

    let list = tool
        .call(serde_json::json!({ "action": "list" }))
        .unwrap();
    assert!(list.contains("shell recusou grep"), "{list}");
    assert!(list.contains("[refusal]"), "{list}");

    // O mesmo atrito repetido não engorda o arquivo.
    let again = tool
        .call(serde_json::json!({
            "action": "record",
            "text": "shell recusou grep",
            "kind": "refusal"
        }))
        .unwrap();
    assert!(again.contains("já registrado"), "{again}");
    assert_eq!(memory::feedback_records(dir.path()).len(), 1);

    // Sink separado e aditivo: nada aqui vira memória injetada.
    assert!(
        memory::facts(dir.path()).is_empty(),
        "feedback não pode virar fato"
    );
}

#[test]
fn record_requires_text_and_a_closed_kind() {
    let dir = tempfile::tempdir().unwrap();
    let state = tempfile::tempdir().unwrap();
    let registry = core_registry(dir.path(), state.path());
    let tool = feedback(&registry);

    assert!(
        tool.call(serde_json::json!({ "action": "record" }))
            .is_err(),
        "record sem text deveria recusar"
    );
    assert!(
        tool.call(serde_json::json!({
            "action": "record",
            "text": "mensagem estranha",
            "kind": "inventado"
        }))
        .is_err(),
        "kind fora do enum deveria recusar"
    );
    assert!(memory::feedback_records(dir.path()).is_empty());
}
