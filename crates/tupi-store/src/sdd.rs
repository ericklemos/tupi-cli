use serde::{Deserialize, Serialize};

use crate::{SETTINGS, Store, StoreError};

pub(crate) const SDD_PLANS: redb::TableDefinition<&'static str, &'static [u8]> =
    redb::TableDefinition::new("sdd_plans");

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SddPlanStatus {
    Draft,
    Closed,
    Completed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SddTaskStatus {
    Open,
    InProgress,
    Done,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SddSubtaskStatus {
    Open,
    Done,
}

/// Teste cargo que prova o aceite de uma subtask.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AcceptanceTest {
    #[serde(default)]
    pub package: Option<String>,
    pub filter: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SddSubtask {
    pub id: String,
    pub title: String,
    pub acceptance: String,
    #[serde(default)]
    pub touches: Vec<String>,
    #[serde(default)]
    pub acceptance_test: Option<AcceptanceTest>,
    pub status: SddSubtaskStatus,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SddTask {
    pub id: String,
    pub title: String,
    pub details: String,
    #[serde(default)]
    pub nuances: Vec<String>,
    #[serde(default)]
    pub parallel: bool,
    pub status: SddTaskStatus,
    #[serde(default)]
    pub subtasks: Vec<SddSubtask>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SddPlan {
    pub slug: String,
    pub repo_key: String,
    pub title: String,
    pub status: SddPlanStatus,
    pub goal: String,
    #[serde(default)]
    pub constraints: Vec<String>,
    #[serde(default)]
    pub nuances: Vec<String>,
    #[serde(default)]
    pub tasks: Vec<SddTask>,
    pub created_at: u64,
    pub updated_at: u64,
    #[serde(default)]
    pub closed_at: Option<u64>,
}

fn plan_key(repo_key: &str, slug: &str) -> String {
    format!("{repo_key}:{slug}")
}

fn active_setting_key(repo_key: &str) -> String {
    format!("sdd:active:{repo_key}")
}

impl Store {
    pub fn save_sdd_plan(&self, plan: &SddPlan) -> Result<(), StoreError> {
        let key = plan_key(&plan.repo_key, &plan.slug);
        let raw =
            serde_json::to_vec(plan).map_err(|error| StoreError::Corrupt(error.to_string()))?;

        self.with_db(|db| {
            let tx = db
                .begin_write()
                .map_err(|error| StoreError::Transaction(error.to_string()))?;
            {
                let mut table = tx
                    .open_table(SDD_PLANS)
                    .map_err(|error| StoreError::Table(error.to_string()))?;
                table
                    .insert(key.as_str(), raw.as_slice())
                    .map_err(|error| StoreError::Table(error.to_string()))?;
            }
            tx.commit()
                .map_err(|error| StoreError::Transaction(error.to_string()))?;
            Ok(())
        })
    }

    pub fn load_sdd_plan(&self, repo_key: &str, slug: &str) -> Result<Option<SddPlan>, StoreError> {
        use redb::ReadableDatabase;

        let key = plan_key(repo_key, slug);
        self.with_db(|db| {
            let tx = db
                .begin_read()
                .map_err(|error| StoreError::Transaction(error.to_string()))?;
            let table = tx
                .open_table(SDD_PLANS)
                .map_err(|error| StoreError::Table(error.to_string()))?;
            let Some(raw) = table
                .get(key.as_str())
                .map_err(|error| StoreError::Table(error.to_string()))?
            else {
                return Ok(None);
            };
            let plan = serde_json::from_slice::<SddPlan>(raw.value())
                .map_err(|error| StoreError::Corrupt(error.to_string()))?;
            Ok(Some(plan))
        })
    }

    pub fn list_sdd_plans(&self, repo_key: &str) -> Result<Vec<SddPlan>, StoreError> {
        use redb::{ReadableDatabase, ReadableTable};

        let prefix = format!("{repo_key}:");
        self.with_db(|db| {
            let tx = db
                .begin_read()
                .map_err(|error| StoreError::Transaction(error.to_string()))?;
            let table = tx
                .open_table(SDD_PLANS)
                .map_err(|error| StoreError::Table(error.to_string()))?;
            let mut plans = Vec::new();
            for item in table
                .iter()
                .map_err(|error| StoreError::Table(error.to_string()))?
            {
                let (k, v) = item.map_err(|error| StoreError::Table(error.to_string()))?;
                if k.value().starts_with(&prefix) {
                    let plan = serde_json::from_slice::<SddPlan>(v.value())
                        .map_err(|error| StoreError::Corrupt(error.to_string()))?;
                    plans.push(plan);
                }
            }
            plans.sort_by(|a, b| b.updated_at.cmp(&a.updated_at).then(a.slug.cmp(&b.slug)));
            Ok(plans)
        })
    }

    pub fn active_sdd_slug(&self, repo_key: &str) -> Result<Option<String>, StoreError> {
        self.setting(&active_setting_key(repo_key))
    }

    pub fn set_active_sdd_slug(
        &self,
        repo_key: &str,
        slug: Option<&str>,
    ) -> Result<(), StoreError> {
        let key = active_setting_key(repo_key);
        match slug {
            Some(value) => self.save_setting(&key, value),
            None => self.with_db(|db| {
                let tx = db
                    .begin_write()
                    .map_err(|error| StoreError::Transaction(error.to_string()))?;
                {
                    let mut table = tx
                        .open_table(SETTINGS)
                        .map_err(|error| StoreError::Table(error.to_string()))?;
                    table
                        .remove(key.as_str())
                        .map_err(|error| StoreError::Table(error.to_string()))?;
                }
                tx.commit()
                    .map_err(|error| StoreError::Transaction(error.to_string()))?;
                Ok(())
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use super::*;
    use crate::SCHEMA_VERSION;

    fn temp_path(tag: &str) -> PathBuf {
        let path =
            std::env::temp_dir().join(format!("tupi-store-{tag}-{}.redb", std::process::id()));
        let _ = std::fs::remove_file(&path);
        path
    }

    fn read_schema_version_with_redb(path: &Path) -> u64 {
        use redb::ReadableDatabase;

        let db = redb::Database::create(path).unwrap();
        let tx = db.begin_read().unwrap();
        let table = tx.open_table(super::super::META).unwrap();
        let raw = table.get("schema_version").unwrap().unwrap();
        serde_json::from_slice(raw.value()).unwrap()
    }

    fn sample_plan(repo_key: &str, slug: &str, status: SddPlanStatus) -> SddPlan {
        SddPlan {
            slug: slug.to_string(),
            repo_key: repo_key.to_string(),
            title: format!("Plano {slug}"),
            status,
            goal: "Objetivo".to_string(),
            constraints: vec!["Restrição 1".to_string()],
            nuances: vec!["Nuance 1".to_string()],
            tasks: vec![SddTask {
                id: "T1".to_string(),
                title: "Task 1".to_string(),
                details: "Detalhes".to_string(),
                nuances: vec![],
                parallel: true,
                status: SddTaskStatus::Open,
                subtasks: vec![SddSubtask {
                    id: "T1.1".to_string(),
                    title: "Subtask 1".to_string(),
                    acceptance: "Passa teste".to_string(),
                    touches: vec!["src/a.rs".to_string()],
                    acceptance_test: Some(AcceptanceTest {
                        package: Some("tupi-app".to_string()),
                        filter: "sdd_run".to_string(),
                    }),
                    status: SddSubtaskStatus::Open,
                }],
            }],
            created_at: 100,
            updated_at: 100,
            closed_at: None,
        }
    }

    #[test]
    fn sdd_plans_and_active_pointer_are_isolated_by_repo_key() {
        let path = temp_path("sdd-iso");
        let store = Store::open(&path).unwrap();
        let plan_a = sample_plan("repo-a", "auth", SddPlanStatus::Draft);
        let plan_b = sample_plan("repo-b", "auth", SddPlanStatus::Closed);

        store.save_sdd_plan(&plan_a).unwrap();
        store.set_active_sdd_slug("repo-a", Some("auth")).unwrap();
        store.save_sdd_plan(&plan_b).unwrap();
        store.set_active_sdd_slug("repo-b", Some("other")).unwrap();

        assert_eq!(
            store
                .load_sdd_plan("repo-a", "auth")
                .unwrap()
                .unwrap()
                .status,
            SddPlanStatus::Draft
        );
        assert_eq!(
            store
                .load_sdd_plan("repo-b", "auth")
                .unwrap()
                .unwrap()
                .status,
            SddPlanStatus::Closed
        );
        assert_eq!(
            store.active_sdd_slug("repo-a").unwrap().as_deref(),
            Some("auth")
        );
        assert_eq!(
            store.active_sdd_slug("repo-b").unwrap().as_deref(),
            Some("other")
        );
        assert_eq!(store.list_sdd_plans("repo-a").unwrap().len(), 1);
    }

    #[test]
    fn older_schema_v1_upgrades_and_opens_sdd_table() {
        let path = temp_path("sdd-upgrade");
        {
            let db = redb::Database::create(&path).unwrap();
            let tx = db.begin_write().unwrap();
            {
                let mut meta = tx.open_table(super::super::META).unwrap();
                let raw = serde_json::to_vec(&1u64).unwrap();
                meta.insert("schema_version", raw.as_slice()).unwrap();
            }
            tx.commit().unwrap();
        }
        let store = Store::open(&path).unwrap();
        assert_eq!(read_schema_version_with_redb(&path), SCHEMA_VERSION);
        assert!(store.list_sdd_plans("repo-a").unwrap().is_empty());
    }

    #[test]
    fn corrupt_sdd_plan_returns_store_error() {
        let path = temp_path("sdd-corrupt");
        let store = Store::open(&path).unwrap();
        store
            .with_db(|db| {
                let tx = db.begin_write().unwrap();
                {
                    let mut table = tx.open_table(SDD_PLANS).unwrap();
                    table.insert("repo-a:auth", b"not-json".as_slice()).unwrap();
                }
                tx.commit().unwrap();
                Ok(())
            })
            .unwrap();

        let err = store.load_sdd_plan("repo-a", "auth").unwrap_err();
        assert!(matches!(err, StoreError::Corrupt(_)));
    }
}
