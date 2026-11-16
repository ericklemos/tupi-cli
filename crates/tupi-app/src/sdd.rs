use crate::sdd_render::{list_block, render};
use tupi_domain::AgentMode;
use tupi_tools::sdd::{SddPlan, SddSubtaskStatus, SddTaskStatus};

/// Fatos da task/subtask, sem as regras do papel de quem vai executar.
/// Compartilhado entre o brief de execução e o de retomada.
fn push_next_step(
    out: &mut String,
    task: &tupi_tools::sdd::SddTask,
    subtask: Option<&tupi_tools::sdd::SddSubtask>,
) {
    out.push_str(&format!("## Task {} — {}\n", task.id, task.title));
    if !task.details.trim().is_empty() {
        out.push_str(&format!("Detalhes: {}\n", task.details.trim()));
    }
    for nuance in &task.nuances {
        out.push_str(&format!("Nuance: {nuance}\n"));
    }
    if let Some(sub) = subtask {
        out.push_str(&format!("\n## Subtarefa: {} — {}\n", sub.id, sub.title));
        if !sub.acceptance.trim().is_empty() {
            out.push_str(&format!("Aceite: {}\n", sub.acceptance.trim()));
        }
        if !sub.touches.is_empty() {
            out.push_str(&format!("Arquivos: {}\n", sub.touches.join(", ")));
        }
    }
}

/// Brief enxuto de uma subtask para o agente de implementação.
pub fn format_subtask_brief(
    plan: &tupi_tools::sdd::SddPlan,
    task: &tupi_tools::sdd::SddTask,
    subtask: Option<&tupi_tools::sdd::SddSubtask>,
) -> String {
    let mut next_step = String::new();
    push_next_step(&mut next_step, task, subtask);
    render(
        tupi_kernel::sdd_config().execution.subtask_brief,
        &[
            (
                "subtask_id",
                subtask.map(|sub| sub.id.as_str()).unwrap_or(task.id.as_str()),
            ),
            ("plan_title", plan.title.as_str()),
            ("plan_goal", plan.goal.as_str()),
            ("constraints", &list_block("Restrições", &plan.constraints)),
            ("nuances", &list_block("Nuances", &plan.nuances)),
            ("next_step", &format!("\n{next_step}")),
        ],
    )
}

/// Relatório final do runner: quantas tasks ou subtasks fecharam e o resultado.
pub fn format_run_report(plan: &tupi_tools::sdd::SddPlan, lines: &[String]) -> String {
    let subtasks: Vec<&tupi_tools::sdd::SddSubtask> = plan
        .tasks
        .iter()
        .flat_map(|task| task.subtasks.iter())
        .collect();
    let (done, total, unit) = if subtasks.is_empty() {
        let done_tasks = plan
            .tasks
            .iter()
            .filter(|task| task.status == tupi_tools::sdd::SddTaskStatus::Done)
            .count();
        (done_tasks, plan.tasks.len(), "tasks")
    } else {
        let done_subtasks = subtasks
            .iter()
            .filter(|sub| sub.status == tupi_tools::sdd::SddSubtaskStatus::Done)
            .count();
        (done_subtasks, subtasks.len(), "subtasks")
    };
    let mut lines_block = String::new();
    for line in lines {
        lines_block.push_str(&format!("- {line}\n"));
    }
    render(
        tupi_kernel::sdd_config().execution.run_report,
        &[
            ("done", &done.to_string()),
            ("total", &total.to_string()),
            ("unit", unit),
            ("lines", &lines_block),
        ],
    )
}

/// Handoff entregue ao agente quando o plano fechado vira execução.
pub fn format_execution_handoff(plan: &SddPlan) -> String {
    let report = std::env::current_dir()
        .ok()
        .and_then(|cwd| {
            std::fs::read_to_string(
                cwd.join(".tupi")
                    .join("sdd")
                    .join(&plan.slug)
                    .join("result.md"),
            )
            .ok()
        })
        .filter(|report| !report.trim().is_empty());
    let mut tasks = String::new();
    for task in &plan.tasks {
        let status = match task.status {
            SddTaskStatus::Done => "done",
            SddTaskStatus::InProgress => "in_progress",
            SddTaskStatus::Open => "open",
        };
        tasks.push_str(&format!("- [{}] {} — {}\n", status, task.id, task.title));
        if !task.details.trim().is_empty() {
            tasks.push_str(&format!("  Detalhes: {}\n", task.details.trim()));
        }
        for nuance in &task.nuances {
            tasks.push_str(&format!("  Nuance: {nuance}\n"));
        }
        for sub in &task.subtasks {
            let smark = if sub.status == SddSubtaskStatus::Done {
                "x"
            } else {
                " "
            };
            tasks.push_str(&format!("  - [{smark}] {} — {}", sub.id, sub.title));
            if !sub.acceptance.trim().is_empty() {
                tasks.push_str(&format!(" (aceite: {})", sub.acceptance.trim()));
            }
            tasks.push('\n');
        }
    }
    let config = tupi_kernel::sdd_config();
    let template = match &report {
        Some(_) => config.execution.handoff_done,
        None => config.execution.handoff_open,
    };
    render(
        template,
        &[
            ("slug", plan.slug.as_str()),
            ("title", plan.title.as_str()),
            ("goal", plan.goal.as_str()),
            ("constraints", &list_block("Restrições", &plan.constraints)),
            ("nuances", &list_block("Nuances", &plan.nuances)),
            ("tasks", &tasks),
            ("report", report.as_deref().unwrap_or("")),
        ],
    )
}

fn plan_status_label(status: tupi_tools::sdd::SddPlanStatus) -> &'static str {
    match status {
        tupi_tools::sdd::SddPlanStatus::Draft => "draft",
        tupi_tools::sdd::SddPlanStatus::Closed => "closed",
        tupi_tools::sdd::SddPlanStatus::Completed => "completed",
        tupi_tools::sdd::SddPlanStatus::Failed => "failed",
    }
}

/// Cabeçalho comum aos briefs derivados do plano: título, objetivo,
/// restrições e nuances.
fn push_plan_heading(out: &mut String, plan: &SddPlan) {
    out.push_str(&format!(
        "# {} (`{}`)\nObjetivo: {}\n",
        plan.title, plan.slug, plan.goal
    ));
    out.push_str(&list_block("Restrições", &plan.constraints));
    out.push_str(&list_block("Nuances", &plan.nuances));
}

/// Brief injetado no histórico por `/sdd <slug>`.
///
/// Não é um turno: a primeira linha proíbe responder e pede que o agente
/// aguarde o próximo pedido (por exemplo, "continue"). O restante reaproveita
/// o cabeçalho do handoff de execução e o brief da próxima subtarefa.
pub fn format_resume_brief(plan: &SddPlan) -> String {
    let mut out = String::from(
        "Contexto de retomada do plano SDD, injetado pelo comando /sdd. \
         Não responda a esta mensagem e não comece a executar por conta própria: \
         aguarde o próximo pedido do usuário (por exemplo, \"continue\").\n\n",
    );
    out.push_str(&format!("Status: {}\n", plan_status_label(plan.status)));
    push_plan_heading(&mut out, plan);

    let open_tasks: Vec<&tupi_tools::sdd::SddTask> = plan
        .tasks
        .iter()
        .filter(|task| task.status != SddTaskStatus::Done)
        .collect();
    if plan.tasks.is_empty() {
        out.push_str("\nO plano ainda não tem tasks definidas.\n");
    } else if open_tasks.is_empty() {
        out.push_str("\nTodas as tasks já estão concluídas.\n");
    } else {
        out.push_str("\nTasks abertas:\n");
        for task in &open_tasks {
            let status = match task.status {
                SddTaskStatus::Done => "done",
                SddTaskStatus::InProgress => "in_progress",
                SddTaskStatus::Open => "open",
            };
            out.push_str(&format!("- [{}] {} — {}\n", status, task.id, task.title));
            if let Some(sub) = task
                .subtasks
                .iter()
                .find(|sub| sub.status != SddSubtaskStatus::Done)
            {
                out.push_str(&format!(
                    "  Próxima subtarefa: {} — {}\n",
                    sub.id, sub.title
                ));
            }
        }
    }

    if let Some((task, subtask)) = open_tasks.iter().find_map(|task| {
        let next = task
            .subtasks
            .iter()
            .find(|sub| sub.status != SddSubtaskStatus::Done);
        Some((*task, next))
    }) {
        out.push_str("\n## Próximo passo\n\n");
        push_next_step(&mut out, task, subtask);
    }

    out
}

pub fn compose_prefix(base: &str, mode: AgentMode) -> String {
    match mode {
        AgentMode::Execution => base.to_string(),
        AgentMode::Sdd => {
            let posture = tupi_kernel::sdd_config().planning.posture;
            if posture.trim().is_empty() {
                return base.to_string();
            }
            if base.trim().is_empty() {
                posture.to_string()
            } else {
                format!("{base}\n\n{posture}")
            }
        }
        AgentMode::Docs => crate::docs::compose_docs_prefix(base),
    }
}

/// Prefixo do agente de implementação: o prompt base do projeto (Descoberta,
/// Convergência, AGENTS.md) mais as regras do papel de implementador.
pub fn compose_agent_prefix(base: &str) -> String {
    let role = tupi_kernel::sdd_config().execution.agent_prefix;
    if role.trim().is_empty() {
        return base.to_string();
    }
    if base.trim().is_empty() {
        role.to_string()
    } else {
        format!("{base}\n\n{role}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_prefix_carries_the_project_prompt_and_the_role() {
        let composed = compose_agent_prefix("PROMPT DO PROJETO");
        let role = tupi_kernel::sdd_config().execution.agent_prefix;
        assert!(composed.starts_with("PROMPT DO PROJETO"));
        assert!(composed.ends_with(role));
    }

    #[test]
    fn agent_prefix_falls_back_to_the_role_when_the_project_prompt_is_empty() {
        let role = tupi_kernel::sdd_config().execution.agent_prefix;
        assert_eq!(compose_agent_prefix("   "), role);
    }

    #[test]
    fn sdd_prefix_composes_the_project_prompt_with_the_configured_posture() {
        let posture = tupi_kernel::sdd_config().planning.posture;
        assert_eq!(compose_prefix("", AgentMode::Sdd), posture);
        assert_eq!(compose_prefix("BASE", AgentMode::Sdd), format!("BASE\n\n{posture}"));
        assert_eq!(compose_prefix("BASE", AgentMode::Execution), "BASE");
    }
}
