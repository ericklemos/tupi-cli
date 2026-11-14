use std::sync::Arc;

use tupi_store::{
    SddPlan as StorePlan, SddPlanStatus as StorePlanStatus, SddSubtask as StoreSubtask,
    SddSubtaskStatus as StoreSubtaskStatus, SddTask as StoreTask, SddTaskStatus as StoreTaskStatus,
    Store,
};
use tupi_tools::{
    SddPlan, SddPlanStatus, SddStore, SddSubtask, SddSubtaskStatus, SddTask, SddTaskStatus,
};

pub struct RedbSddStore {
    store: Arc<Store>,
}

impl RedbSddStore {
    pub fn new(store: Arc<Store>) -> Self {
        Self { store }
    }
}

fn to_store_plan(plan: &SddPlan) -> StorePlan {
    StorePlan {
        slug: plan.slug.clone(),
        repo_key: plan.repo_key.clone(),
        title: plan.title.clone(),
        status: match plan.status {
            SddPlanStatus::Draft => StorePlanStatus::Draft,
            SddPlanStatus::Closed => StorePlanStatus::Closed,
            SddPlanStatus::Completed => StorePlanStatus::Completed,
        },
        goal: plan.goal.clone(),
        constraints: plan.constraints.clone(),
        nuances: plan.nuances.clone(),
        tasks: plan
            .tasks
            .iter()
            .map(|t| StoreTask {
                id: t.id.clone(),
                title: t.title.clone(),
                details: t.details.clone(),
                nuances: t.nuances.clone(),
                parallel: t.parallel,
                status: match t.status {
                    SddTaskStatus::Open => StoreTaskStatus::Open,
                    SddTaskStatus::InProgress => StoreTaskStatus::InProgress,
                    SddTaskStatus::Done => StoreTaskStatus::Done,
                },
                subtasks: t
                    .subtasks
                    .iter()
                    .map(|s| StoreSubtask {
                        id: s.id.clone(),
                        title: s.title.clone(),
                        acceptance: s.acceptance.clone(),
                        touches: s.touches.clone(),
                        acceptance_test: s.acceptance_test.as_ref().map(|t| {
                            tupi_store::AcceptanceTest {
                                package: t.package.clone(),
                                filter: t.filter.clone(),
                            }
                        }),
                        status: match s.status {
                            SddSubtaskStatus::Open => StoreSubtaskStatus::Open,
                            SddSubtaskStatus::Done => StoreSubtaskStatus::Done,
                        },
                    })
                    .collect(),
            })
            .collect(),
        created_at: plan.created_at,
        updated_at: plan.updated_at,
        closed_at: plan.closed_at,
    }
}

fn from_store_plan(plan: StorePlan) -> SddPlan {
    SddPlan {
        slug: plan.slug,
        repo_key: plan.repo_key,
        title: plan.title,
        status: match plan.status {
            StorePlanStatus::Draft => SddPlanStatus::Draft,
            StorePlanStatus::Closed => SddPlanStatus::Closed,
            StorePlanStatus::Completed => SddPlanStatus::Completed,
        },
        goal: plan.goal,
        constraints: plan.constraints,
        nuances: plan.nuances,
        tasks: plan
            .tasks
            .into_iter()
            .map(|t| SddTask {
                id: t.id,
                title: t.title,
                details: t.details,
                nuances: t.nuances,
                parallel: t.parallel,
                status: match t.status {
                    StoreTaskStatus::Open => SddTaskStatus::Open,
                    StoreTaskStatus::InProgress => SddTaskStatus::InProgress,
                    StoreTaskStatus::Done => SddTaskStatus::Done,
                },
                subtasks: t
                    .subtasks
                    .into_iter()
                    .map(|s| SddSubtask {
                        id: s.id,
                        title: s.title,
                        acceptance: s.acceptance,
                        touches: s.touches,
                        acceptance_test: s.acceptance_test.map(|t| {
                            tupi_tools::sdd::AcceptanceTest {
                                package: t.package,
                                filter: t.filter,
                            }
                        }),
                        status: match s.status {
                            StoreSubtaskStatus::Open => SddSubtaskStatus::Open,
                            StoreSubtaskStatus::Done => SddSubtaskStatus::Done,
                        },
                    })
                    .collect(),
            })
            .collect(),
        created_at: plan.created_at,
        updated_at: plan.updated_at,
        closed_at: plan.closed_at,
    }
}

impl SddStore for RedbSddStore {
    fn save(&self, plan: &SddPlan) -> Result<(), String> {
        self.store
            .save_sdd_plan(&to_store_plan(plan))
            .map_err(|error| error.to_string())
    }

    fn load(&self, repo_key: &str, slug: &str) -> Result<Option<SddPlan>, String> {
        self.store
            .load_sdd_plan(repo_key, slug)
            .map(|opt| opt.map(from_store_plan))
            .map_err(|error| error.to_string())
    }

    fn list(&self, repo_key: &str) -> Result<Vec<SddPlan>, String> {
        self.store
            .list_sdd_plans(repo_key)
            .map(|plans| plans.into_iter().map(from_store_plan).collect())
            .map_err(|error| error.to_string())
    }

    fn active_slug(&self, repo_key: &str) -> Result<Option<String>, String> {
        self.store
            .active_sdd_slug(repo_key)
            .map_err(|error| error.to_string())
    }

    fn set_active_slug(&self, repo_key: &str, slug: Option<&str>) -> Result<(), String> {
        self.store
            .set_active_sdd_slug(repo_key, slug)
            .map_err(|error| error.to_string())
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use tupi_store::Store;
    use tupi_tools::{
        SddPlan, SddPlanStatus, SddStore, SddSubtask, SddSubtaskStatus, SddTask, SddTaskStatus,
    };

    use super::*;

    #[test]
    fn redb_sdd_store_round_trips_and_survives_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("tupi.redb");

        let plan = SddPlan {
            slug: "auth".into(),
            repo_key: "repo-x".into(),
            title: "Auth".into(),
            status: SddPlanStatus::Closed,
            goal: "OAuth".into(),
            constraints: vec!["Sem quebra".into()],
            nuances: vec!["55m".into()],
            tasks: vec![SddTask {
                id: "T1".into(),
                title: "Refresh".into(),
                details: "Det".into(),
                nuances: vec![],
                parallel: true,
                status: SddTaskStatus::InProgress,
                subtasks: vec![SddSubtask {
                    id: "T1.1".into(),
                    title: "Lock".into(),
                    acceptance: "Passa".into(),
                    touches: vec!["src/a.rs".into()],
                    acceptance_test: Some(tupi_tools::sdd::AcceptanceTest {
                        package: Some("tupi-app".into()),
                        filter: "sdd_run".into(),
                    }),
                    status: SddSubtaskStatus::Done,
                }],
            }],
            created_at: 10,
            updated_at: 20,
            closed_at: Some(20),
        };

        {
            let store = Arc::new(Store::open(&db_path).unwrap());
            let adapter = RedbSddStore::new(store);
            adapter.save(&plan).unwrap();
            adapter.set_active_slug("repo-x", Some("auth")).unwrap();
        }

        let reopened = Arc::new(Store::open(&db_path).unwrap());
        let adapter = RedbSddStore::new(reopened);
        assert_eq!(adapter.load("repo-x", "auth").unwrap(), Some(plan));
        assert_eq!(
            adapter.active_slug("repo-x").unwrap().as_deref(),
            Some("auth")
        );
        assert_eq!(adapter.list("repo-x").unwrap().len(), 1);
    }
}
