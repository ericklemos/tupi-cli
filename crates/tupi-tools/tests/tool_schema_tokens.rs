//! Custo em tokens do schema de tools que viaja em **toda** requisição.
//!
//! `expose_all` (`configs/tools.ron`) manda o payload de todas as tools em cada
//! passo; esse payload é medido aqui pelo mesmo estimador que o planejador de
//! contexto usa (`tupi_domain::tokens::estimate_tokens`, chamado por
//! `tool_spec_tokens` em `crates/tupi-app/src/app.rs:170-174`) e é subtraído do
//! orçamento de mensagens (`plan_context`, `crates/tupi-app/src/app.rs:142-168`).
//!
//! A medição é local e mecânica: nada de API. Números vivos em
//! `docs/perf/turn-latency.md`; o assert trava a deriva do schema (tool nova ou
//! descrição maior muda o número e obriga a atualizar a doc no mesmo commit).

use tupi_kernel::AgentMode;
use tupi_kernel::ToolRegistry;
use tupi_tools::Exposure;

fn tokens_of(exposure: Exposure<'_>, root: &std::path::Path) -> (usize, usize) {
    let mut registry = ToolRegistry::new();
    tupi_tools::register_exposed(root.to_path_buf(), &mut registry, exposure);

    let specs = registry.specs_for_mode(AgentMode::Execution);
    let payload = tupi_kernel::tools_payload(&specs);
    let json = serde_json::to_string(&payload).unwrap_or_default();
    (specs.len(), tupi_domain::tokens::estimate_tokens(&json))
}

/// O schema é custo fixo por passo: ele viaja nas duas exposições e o número que
/// importa é quanto `expose_all` acrescenta sobre o núcleo.
#[test]
fn the_agent_schema_costs_tokens_on_every_step() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .to_path_buf();

        assert_eq!(
            tokens_of(Exposure::Core, &root),
            (11, 4468),
            "núcleo (Exposure::Core) mudou: atualize docs/perf/turn-latency.md"
        );
        assert_eq!(
            tokens_of(Exposure::All, &root),
            (29, 10613),
            "exposição total (Exposure::All) mudou: atualize docs/perf/turn-latency.md"
        );
    }
