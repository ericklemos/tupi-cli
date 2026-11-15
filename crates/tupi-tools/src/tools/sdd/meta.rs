use serde::{Deserialize, Serialize};

use super::{DocsFeedback, SddActivity, SddNote, SddPlan, SddPlanStatus, SddTask};

pub(crate) const SCHEMA: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct MetaFile {
    pub schema: u32,
    pub slug: String,
    pub title: String,
    pub status: SddPlanStatus,
    pub goal: String,
    #[serde(default)]
    pub constraints: Vec<String>,
    #[serde(default)]
    pub nuances: Vec<String>,
    pub created_at: u64,
    pub updated_at: u64,
    #[serde(default)]
    pub closed_at: Option<u64>,
    #[serde(default)]
    pub conclusion: Option<String>,
    #[serde(default)]
    pub docs_feedback: Option<DocsFeedback>,
    #[serde(default)]
    pub activities: Vec<SddActivity>,
    #[serde(default)]
    pub notes: Vec<SddNote>,
}

impl MetaFile {
    pub fn from_plan(plan: &SddPlan) -> Self {
        Self {
            schema: SCHEMA,
            slug: plan.slug.clone(),
            title: plan.title.clone(),
            status: plan.status,
            goal: plan.goal.clone(),
            constraints: plan.constraints.clone(),
            nuances: plan.nuances.clone(),
            created_at: plan.created_at,
            updated_at: plan.updated_at,
            closed_at: plan.closed_at,
            conclusion: plan.conclusion.clone(),
            docs_feedback: plan.docs_feedback.clone(),
            activities: plan.activities.clone(),
            notes: plan.notes.clone(),
        }
    }

    pub fn into_plan(self, repo_key: String, tasks: Vec<SddTask>) -> SddPlan {
        SddPlan {
            slug: self.slug,
            repo_key,
            title: self.title,
            status: self.status,
            goal: self.goal,
            constraints: self.constraints,
            nuances: self.nuances,
            tasks,
            created_at: self.created_at,
            updated_at: self.updated_at,
            closed_at: self.closed_at,
            activities: self.activities,
            notes: self.notes,
            conclusion: self.conclusion,
            docs_feedback: self.docs_feedback,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct TasksFile {
    pub schema: u32,
    pub slug: String,
    pub updated_at: u64,
    #[serde(default)]
    pub tasks: Vec<SddTask>,
}

impl TasksFile {
    pub fn from_plan(plan: &SddPlan) -> Self {
        Self {
            schema: SCHEMA,
            slug: plan.slug.clone(),
            updated_at: plan.updated_at,
            tasks: plan.tasks.clone(),
        }
    }

    pub fn into_tasks(self) -> Vec<SddTask> {
        self.tasks
    }
}
