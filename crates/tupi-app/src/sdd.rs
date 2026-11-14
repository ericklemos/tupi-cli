use tupi_domain::AgentMode;
use tupi_store::{SddPlan, SddSubtaskStatus, SddTaskStatus};

pub const SDD_POSTURE_PROMPT: &str = r#"## Modo SDD (Planejamento)

Você está em modo SDD (somente leitura; edições e comandos estão bloqueados).
1. Investigue o necessário com as tools de leitura/busca e faça perguntas objetivas de esclarecimento antes de propor soluções.
2. Use a tool `sdd` (`upsert_spec`, `set_tasks`) para estruturar spec, restrições, nuances destiladas, tasks e subtasks autocontidas.
3. Quando tiver contexto suficiente, avise em linguagem natural e pergunte se o usuário deseja continuar o debate ou fechar a spec.
4. Só chame `sdd(action="close")` após confirmação explícita do usuário."#;

/// Brief enxuto de uma subtask para o agente de implementação.
pub fn format_subtask_brief(
    plan: &tupi_tools::sdd::SddPlan,
    task: &tupi_tools::sdd::SddTask,
    subtask: Option<&tupi_tools::sdd::SddSubtask>,
) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "Você é o agente de implementação da subtarefa {}.\n\n# {}\nObjetivo: {}\n",
        subtask
            .map(|sub| sub.id.as_str())
            .unwrap_or(task.id.as_str()),
        plan.title,
        plan.goal
    ));
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
    out.push_str(&format!("\n## Task {} — {}\n", task.id, task.title));
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
    out.push_str(
        "\nRegras:\n- Implemente apenas esta subtarefa; não toque no que pertence a outras.\n- Outros agentes podem estar trabalhando em paralelo em outras partes do repositório.\n- Não edite o plano nem chame sdd.\n- Ao terminar, responda em até 3 linhas com o que mudou e como verificou.\n",
    );
    out
}

/// Relatório curto da execução para voltar ao agente principal.
pub fn format_run_report(plan: &tupi_tools::sdd::SddPlan, lines: &[String]) -> String {
    let subtasks: Vec<&tupi_tools::sdd::SddSubtask> = plan
        .tasks
        .iter()
        .flat_map(|task| task.subtasks.iter())
        .collect();
    let done = subtasks
        .iter()
        .filter(|sub| sub.status == tupi_tools::sdd::SddSubtaskStatus::Done)
        .count();
    let mut out = if subtasks.is_empty() {
        let done_tasks = plan
            .tasks
            .iter()
            .filter(|task| task.status == tupi_tools::sdd::SddTaskStatus::Done)
            .count();
        format!("Execução SDD: {done_tasks}/{} tasks ok\n", plan.tasks.len())
    } else {
        format!("Execução SDD: {done}/{} subtasks ok\n", subtasks.len())
    };
    for line in lines {
        out.push_str(&format!("- {line}\n"));
    }
    out
}

pub fn format_execution_handoff(plan: &SddPlan) -> String {
    let report = std::env::current_dir()
        .ok()
        .and_then(|cwd| {
            std::fs::read_to_string(
                cwd.join(".agents/tupi/sdd")
                    .join(format!("{}.execution.md", plan.slug)),
            )
            .ok()
        })
        .filter(|report| !report.trim().is_empty());
    let mut out = String::new();
    match &report {
        Some(report) => {
            out.push_str("SDD já executado com agentes. Use o relatório abaixo; não reexecute subtasks done.\n\n");
            out.push_str(report);
            out.push_str("\n\n");
        }
        None => out.push_str("SDD fechado. Inicie a execução seguindo exclusivamente o plano destilado abaixo (não reabra documentação já resolvida).\n\n"),
    }
    out.push_str(&format!(
        "# {} (`{}`)\nObjetivo: {}\n",
        plan.title, plan.slug, plan.goal
    ));

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
