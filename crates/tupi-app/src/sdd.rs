use tupi_domain::AgentMode;
use tupi_store::{SddPlan, SddSubtaskStatus, SddTaskStatus};

pub const SDD_POSTURE_PROMPT: &str = r#"## Modo SDD (Planejamento)

Você está em modo SDD (somente leitura; edições e comandos estão bloqueados).
1. Investigue o necessário com as tools de leitura/busca e faça perguntas objetivas de esclarecimento antes de propor soluções.
2. Use a tool `sdd` (`upsert_spec`, `set_tasks`) para estruturar spec, restrições, nuances destiladas, tasks e subtasks autocontidas.
3. Quando tiver contexto suficiente, avise em linguagem natural e pergunte se o usuário deseja continuar o debate ou fechar a spec.
4. Só chame `sdd(action="close")` após confirmação explícita do usuário."#;

pub fn format_execution_handoff(plan: &SddPlan) -> String {
    let mut out = format!(
        "SDD fechado. Inicie a execução seguindo exclusivamente o plano destilado abaixo (não reabra documentação já resolvida).\n\n# {} (`{}`)\nObjetivo: {}\n",
        plan.title, plan.slug, plan.goal
    );

    if !plan.constraints.is_empty() {
        out.push_str("\nRestrições:\n");
        for item in &plan.constraints {
            out.push_str(&format!("- {item}\n"));
        }
    }

    if !plan.nuances.is_empty() {
        out.push_str("\nNuances:\n");
        for item in &plan.nuances {
            out.push_str(&format!("- {item}\n"));
        }
    }

    out.push_str("\nTasks:\n");
    for task in &plan.tasks {
        let status = match task.status {
            SddTaskStatus::Done => "done",
            SddTaskStatus::InProgress => "in_progress",
            SddTaskStatus::Open => "open",
        };
        out.push_str(&format!("- [{}] {} — {}\n", status, task.id, task.title));
        if !task.details.trim().is_empty() {
            out.push_str(&format!("  Detalhes: {}\n", task.details.trim()));
        }
        for nuance in &task.nuances {
            out.push_str(&format!("  Nuance: {nuance}\n"));
        }
        for sub in &task.subtasks {
            let smark = if sub.status == SddSubtaskStatus::Done {
                "x"
            } else {
                " "
            };
            out.push_str(&format!("  - [{smark}] {} — {}", sub.id, sub.title));
            if !sub.acceptance.trim().is_empty() {
                out.push_str(&format!(" (aceite: {})", sub.acceptance.trim()));
            }
            out.push('\n');
        }
    }

    out.push_str("\nAtualize o progresso das tasks/subtasks via `sdd(action=\"progress\")` conforme concluir.");
    out
}

pub fn compose_prefix(base: &str, mode: AgentMode) -> String {
    match mode {
        AgentMode::Execution => base.to_string(),
        AgentMode::Sdd => {
            if base.trim().is_empty() {
                SDD_POSTURE_PROMPT.to_string()
            } else {
                format!("{base}\n\n{SDD_POSTURE_PROMPT}")
            }
        }
    }
}
