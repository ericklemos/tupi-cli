use super::{DocsFeedbackStatus, SddPlan, SddSubtaskStatus, SddTaskStatus, status_label_plan};

pub const GENERATED_HEADER: &str =
    "<!-- Gerado automaticamente pelo tupi. Não edite; a fonte é meta.json/tasks.json. -->\n\n";

pub fn render_spec(plan: &SddPlan) -> String {
    let mut out = String::from(GENERATED_HEADER);
    out.push_str(&format!("# {} (`{}`)\n\n", plan.title, plan.slug));
    out.push_str(&format!(
        "**Status:** {}\n\n",
        status_label_plan(plan.status)
    ));
    out.push_str("## Objetivo\n\n");
    out.push_str(plan.goal.trim());
    out.push_str("\n\n");
    if !plan.constraints.is_empty() {
        out.push_str("## Restrições\n\n");
        for item in &plan.constraints {
            out.push_str(&format!("- {item}\n"));
        }
        out.push('\n');
    }
    if !plan.nuances.is_empty() {
        out.push_str("## Nuances\n\n");
        for item in &plan.nuances {
            out.push_str(&format!("- {item}\n"));
        }
        out.push('\n');
    }
    out
}

pub fn render_summary(plan: &SddPlan) -> String {
    let mut out = String::from(GENERATED_HEADER);
    out.push_str(&format!("# {} (`{}`)\n\n", plan.title, plan.slug));
    out.push_str(&format!(
        "**Status:** {}\n\n",
        status_label_plan(plan.status)
    ));

    let tasks_total = plan.tasks.len();
    let tasks_done = plan
        .tasks
        .iter()
        .filter(|task| task.status == SddTaskStatus::Done)
        .count();
    let subtasks: Vec<_> = plan
        .tasks
        .iter()
        .flat_map(|task| task.subtasks.iter())
        .collect();
    let subtasks_done = subtasks
        .iter()
        .filter(|sub| sub.status == SddSubtaskStatus::Done)
        .count();
    out.push_str(&format!(
        "**Progresso:** {tasks_done}/{tasks_total} tasks · {subtasks_done}/{} subtasks\n\n",
        subtasks.len()
    ));

    out.push_str("## Próximas\n\n");
    let mut listed = false;
    for task in &plan.tasks {
        if task.status == SddTaskStatus::Done {
            continue;
        }
        if task.subtasks.is_empty() {
            out.push_str(&format!("- [ ] {} {}\n", task.id, task.title));
            listed = true;
        }
        for sub in &task.subtasks {
            if sub.status != SddSubtaskStatus::Done {
                out.push_str(&format!("- [ ] {} {}\n", sub.id, sub.title));
                listed = true;
            }
        }
    }
    if !listed {
        out.push_str("- nenhuma\n");
    }

    out.push_str("\n## Últimas atividades\n\n");
    if plan.activities.is_empty() {
        out.push_str("- nenhuma\n");
    } else {
        for activity in plan.activities.iter().rev().take(5).rev() {
            out.push_str(&format!(
                "- {} {} {}{}\n",
                activity.at,
                activity.action,
                activity.status,
                activity
                    .subtask
                    .as_deref()
                    .or(activity.task.as_deref())
                    .map(|id| format!(" ({id})"))
                    .unwrap_or_default()
            ));
        }
    }
    out
}

pub fn render_result(plan: &SddPlan) -> String {
    let mut out = String::from(GENERATED_HEADER);
    out.push_str(&format!(
        "# Resultado — {} (`{}`)\n\n",
        plan.title, plan.slug
    ));
    out.push_str(&format!("**Status:** {}\n", status_label_plan(plan.status)));
    out.push_str(&format!("**Início:** {}\n", plan.created_at));
    out.push_str(&format!(
        "**Fim:** {}\n\n",
        plan.closed_at.unwrap_or(plan.updated_at)
    ));

    let tasks_total = plan.tasks.len();
    let tasks_done = plan
        .tasks
        .iter()
        .filter(|task| task.status == SddTaskStatus::Done)
        .count();
    let subtasks: Vec<_> = plan
        .tasks
        .iter()
        .flat_map(|task| task.subtasks.iter())
        .collect();
    let subtasks_done = subtasks
        .iter()
        .filter(|sub| sub.status == SddSubtaskStatus::Done)
        .count();
    let failures = plan
        .activities
        .iter()
        .filter(|activity| activity.status == "failed")
        .count();

    out.push_str("## Resumo\n\n");
    out.push_str(&format!("- tasks: {tasks_done}/{tasks_total}\n"));
    out.push_str(&format!(
        "- subtasks: {subtasks_done}/{} ok\n",
        subtasks.len()
    ));
    out.push_str(&format!("- falhas: {failures}\n"));

    out.push_str("\n## Verificação\n\n");
    let mut verified = false;
    for activity in &plan.activities {
        if activity.action.starts_with("acceptance_") {
            out.push_str(&format!(
                "- {}{}: {}\n",
                activity
                    .subtask
                    .as_deref()
                    .or(activity.task.as_deref())
                    .unwrap_or("-"),
                activity.subtask.as_deref().map(|_| "").unwrap_or(""),
                activity.status
            ));
            verified = true;
        }
    }
    if !verified {
        out.push_str("- nenhum aceite registrado\n");
    }

    out.push_str("\n## Docs\n\n");
    match &plan.docs_feedback {
        Some(feedback) => {
            out.push_str(&format!(
                "- status: {}\n",
                feedback_status_label(feedback.status)
            ));
            if feedback.files.is_empty() {
                out.push_str("- arquivos: nenhum\n");
            } else {
                out.push_str(&format!("- arquivos: {}\n", feedback.files.join(", ")));
            }
            out.push_str(&format!(
                "- reindexado: {}\n",
                if feedback.reindexed { "sim" } else { "não" }
            ));
            if let Some(reason) = &feedback.reason {
                out.push_str(&format!("- motivo: {reason}\n"));
            }
        }
        None => out.push_str("- status: skipped\n"),
    }

    out.push_str("\n## Conclusão\n\n");
    out.push_str(
        plan.conclusion
            .as_deref()
            .unwrap_or("(sem conclusão registrada)"),
    );
    out.push('\n');
    out
}

fn feedback_status_label(status: DocsFeedbackStatus) -> &'static str {
    match status {
        DocsFeedbackStatus::Skipped => "skipped",
        DocsFeedbackStatus::Pending => "pending",
        DocsFeedbackStatus::Updated => "updated",
        DocsFeedbackStatus::Created => "created",
        DocsFeedbackStatus::Noop => "noop",
        DocsFeedbackStatus::Failed => "failed",
    }
}

#[cfg(test)]
mod tests {
    use super::super::meta::{MetaFile, TasksFile};
    use super::super::{DocsFeedback, SddPlanStatus, SddSubtask, SddTask};
    use super::*;

    fn plan_fixture() -> SddPlan {
        let mut plan = SddPlan {
            slug: "auth".into(),
            repo_key: "repo".into(),
            title: "Auth".into(),
            status: SddPlanStatus::Draft,
            goal: "OAuth".into(),
            constraints: vec!["sem quebra".into()],
            nuances: vec!["55m".into()],
            tasks: vec![SddTask {
                id: "T1".into(),
                title: "Refresh".into(),
                details: "Renovar".into(),
                nuances: vec![],
                parallel: false,
                status: SddTaskStatus::Open,
                subtasks: vec![SddSubtask {
                    id: "T1.1".into(),
                    title: "Lock".into(),
                    acceptance: "Passa".into(),
                    touches: vec![],
                    acceptance_test: None,
                    status: SddSubtaskStatus::Open,
                }],
            }],
            created_at: 10,
            updated_at: 20,
            closed_at: None,
            ..Default::default()
        };
        plan.record_activity("runner", "subtask_succeeded", "ok");
        plan
    }

    #[test]
    fn meta_projection_keeps_activities_and_tasks_stay_separate() {
        let plan = plan_fixture();
        let meta = MetaFile::from_plan(&plan);
        let tasks = TasksFile::from_plan(&plan);
        assert_eq!(meta.slug, "auth");
        assert_eq!(meta.activities.len(), 1);
        assert_eq!(tasks.tasks.len(), 1);
        let back = meta.into_plan("repo".into(), tasks.into_tasks());
        assert_eq!(back.slug, plan.slug);
        assert_eq!(back.activities.len(), 1);
        assert_eq!(back.repo_key, "repo");
        assert_eq!(back, plan);
    }

    #[test]
    fn renders_carry_the_generated_header_and_expected_sections() {
        let mut plan = plan_fixture();
        let spec = render_spec(&plan);
        assert!(spec.starts_with(GENERATED_HEADER));
        assert!(spec.contains("## Objetivo"));
        assert!(spec.contains("## Restrições"));
        let summary = render_summary(&plan);
        assert!(summary.contains("## Próximas"));
        assert!(summary.contains("## Últimas atividades"));
        plan.status = SddPlanStatus::Completed;
        plan.conclusion = Some("feito".into());
        plan.docs_feedback = Some(DocsFeedback {
            status: DocsFeedbackStatus::Updated,
            files: vec!["docs/a.md".into()],
            reason: Some("documentado".into()),
            reindexed: true,
        });
        let result = render_result(&plan);
        assert!(result.contains("## Verificação"));
        assert!(result.contains("## Docs"));
        assert!(result.contains("## Conclusão"));
        assert!(result.contains("feito"));
    }
}
