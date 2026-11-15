use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::support::paths;

use super::meta::{MetaFile, SCHEMA, TasksFile};
use super::render;
use super::{SddPlan, SddPlanStatus, SddStore, validate_slug};

pub struct FsSddStore {
    root: PathBuf,
    repo_key: String,
}

impl FsSddStore {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        let root = root.into();
        let repo_key = paths::repo_key(&root);
        Self { root, repo_key }
    }

    pub fn repo_key(&self) -> &str {
        &self.repo_key
    }

    fn plans_dir(&self) -> PathBuf {
        self.root.join(".tupi").join("sdd")
    }

    fn plan_dir(&self, slug: &str) -> PathBuf {
        self.plans_dir().join(slug)
    }

    fn active_path(&self) -> PathBuf {
        self.plans_dir().join("active.json")
    }

    fn lock(&self, slug: &str) -> Result<PlanLock, String> {
        let dir = self.plan_dir(slug);
        fs::create_dir_all(&dir)
            .map_err(|error| format!("falha ao criar {}: {error}", dir.display()))?;
        let path = dir.join(".lock");

        for attempt in 0..2 {
            match OpenOptions::new().write(true).create_new(true).open(&path) {
                Ok(mut file) => {
                    let _ = write!(file, "{}", now());
                    return Ok(PlanLock { path });
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    if attempt == 0 && lock_is_stale(&path) {
                        let _ = fs::remove_file(&path);
                        continue;
                    }
                    return Err(format!("plano '{slug}' em uso por outro processo"));
                }
                Err(error) => {
                    return Err(format!("falha ao criar lock de {slug}: {error}"));
                }
            }
        }

        Err(format!("plano '{slug}' em uso por outro processo"))
    }

    fn write_json<T: serde::Serialize>(&self, path: &Path, value: &T) -> Result<(), String> {
        let body = serde_json::to_vec_pretty(value)
            .map_err(|error| format!("falha ao serializar {}: {error}", path.display()))?;
        paths::atomic_write(path, &body)
            .map_err(|error| format!("falha ao gravar {}: {error}", path.display()))
    }

    fn write_text(&self, path: &Path, text: &str) -> Result<(), String> {
        paths::atomic_write(path, text.as_bytes())
            .map_err(|error| format!("falha ao gravar {}: {error}", path.display()))
    }
}

impl SddStore for FsSddStore {
    fn save(&self, plan: &SddPlan) -> Result<(), String> {
        let slug = validate_slug(&plan.slug).map_err(|error| error.to_string())?;
        let _lock = self.lock(&slug)?;
        let dir = self.plan_dir(&slug);
        fs::create_dir_all(&dir)
            .map_err(|error| format!("falha ao criar {}: {error}", dir.display()))?;

        self.write_json(&dir.join("tasks.json"), &TasksFile::from_plan(plan))?;
        self.write_json(&dir.join("meta.json"), &MetaFile::from_plan(plan))?;
        self.write_text(&dir.join("spec.md"), &render::render_spec(plan))?;
        self.write_text(&dir.join("resumo.md"), &render::render_summary(plan))?;

        if matches!(
            &plan.status,
            SddPlanStatus::Completed | SddPlanStatus::Failed
        ) {
            self.write_text(&dir.join("result.md"), &render::render_result(plan))?;
        }

        Ok(())
    }

    fn load(&self, _repo_key: &str, slug: &str) -> Result<Option<SddPlan>, String> {
        let slug = validate_slug(slug).map_err(|error| error.to_string())?;
        let dir = self.plan_dir(&slug);
        let meta_path = dir.join("meta.json");
        if !meta_path.exists() {
            return Ok(None);
        }

        let meta: MetaFile = read_json(&meta_path)?;
        if meta.schema > SCHEMA {
            return Err(format!(
                "schema {} não suportado em {} (máximo {SCHEMA})",
                meta.schema,
                meta_path.display()
            ));
        }
        if meta.slug != slug {
            return Err(format!(
                "slug '{}' não corresponde à pasta '{}'",
                meta.slug, slug
            ));
        }

        let tasks_path = dir.join("tasks.json");
        let tasks: TasksFile = read_json(&tasks_path)?;
        if tasks.schema > SCHEMA {
            return Err(format!(
                "schema {} não suportado em {} (máximo {SCHEMA})",
                tasks.schema,
                tasks_path.display()
            ));
        }
        if tasks.slug != slug {
            return Err(format!(
                "slug '{}' não corresponde à pasta '{}'",
                tasks.slug, slug
            ));
        }

        Ok(Some(
            meta.into_plan(self.repo_key.clone(), tasks.into_tasks()),
        ))
    }

    fn list(&self, _repo_key: &str) -> Result<Vec<SddPlan>, String> {
        let dir = self.plans_dir();
        let Ok(entries) = fs::read_dir(&dir) else {
            return Ok(Vec::new());
        };

        let mut plans = Vec::new();
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let Some(slug) = path.file_name().and_then(|name| name.to_str()) else {
                continue;
            };
            match self.load(&self.repo_key, slug) {
                Ok(Some(plan)) => plans.push(plan),
                Ok(None) => {}
                Err(error) => eprintln!(
                    "plano SDD inválido em {}: {error}; ignorado",
                    path.display()
                ),
            }
        }

        plans.sort_by(|a, b| {
            b.updated_at
                .cmp(&a.updated_at)
                .then_with(|| a.slug.cmp(&b.slug))
        });
        Ok(plans)
    }

    fn active_slug(&self, _repo_key: &str) -> Result<Option<String>, String> {
        let path = self.active_path();
        if !path.exists() {
            return Ok(None);
        }
        let value: serde_json::Value = read_json(&path)?;
        Ok(value
            .get("slug")
            .and_then(|slug| slug.as_str())
            .map(str::to_string))
    }

    fn set_active_slug(&self, _repo_key: &str, slug: Option<&str>) -> Result<(), String> {
        let path = self.active_path();
        match slug {
            Some(slug) => {
                let slug = validate_slug(slug).map_err(|error| error.to_string())?;
                fs::create_dir_all(self.plans_dir()).map_err(|error| {
                    format!("falha ao criar {}: {error}", self.plans_dir().display())
                })?;
                self.write_json(&path, &serde_json::json!({ "slug": slug }))?;
            }
            None => {
                if path.exists() {
                    fs::remove_file(&path)
                        .map_err(|error| format!("falha ao remover {}: {error}", path.display()))?;
                }
            }
        }
        Ok(())
    }
}

struct PlanLock {
    path: PathBuf,
}

impl Drop for PlanLock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T, String> {
    let content = fs::read_to_string(path)
        .map_err(|error| format!("falha ao ler {}: {error}", path.display()))?;
    serde_json::from_str(&content)
        .map_err(|error| format!("JSON inválido em {}: {error}", path.display()))
}

fn lock_is_stale(path: &Path) -> bool {
    let Ok(content) = fs::read_to_string(path) else {
        return false;
    };
    let Ok(at) = content.trim().parse::<u64>() else {
        return false;
    };
    now().saturating_sub(at) > 3600
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use tempfile::tempdir;

    use super::super::{SddSubtask, SddSubtaskStatus, SddTask, SddTaskStatus};
    use super::*;

    fn plan_fixture() -> SddPlan {
        SddPlan {
            slug: "auth".into(),
            repo_key: "repo".into(),
            title: "Auth".into(),
            status: SddPlanStatus::Draft,
            goal: "OAuth".into(),
            constraints: vec!["sem quebra".into()],
            nuances: vec![],
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
        }
    }

    fn copy_dir(from: &Path, to: &Path) {
        fs::create_dir_all(to).unwrap();
        for entry in fs::read_dir(from).unwrap() {
            let entry = entry.unwrap();
            let target = to.join(entry.file_name());
            if entry.file_type().unwrap().is_dir() {
                copy_dir(&entry.path(), &target);
            } else {
                fs::copy(entry.path(), target).unwrap();
            }
        }
    }

    #[test]
    fn save_writes_all_files_and_load_round_trips() {
        let dir = tempdir().unwrap();
        let store = FsSddStore::new(dir.path());
        let mut plan = plan_fixture();
        plan.repo_key = store.repo_key().to_string();
        store.save(&plan).unwrap();

        for file in ["meta.json", "tasks.json", "spec.md", "resumo.md"] {
            assert!(
                dir.path().join(".tupi/sdd/auth").join(file).exists(),
                "{file}"
            );
        }
        assert_eq!(store.load(&plan.repo_key, "auth").unwrap(), Some(plan));
    }

    #[test]
    fn a_completed_plan_writes_result_md() {
        let dir = tempdir().unwrap();
        let store = FsSddStore::new(dir.path());
        let mut plan = plan_fixture();
        plan.status = SddPlanStatus::Completed;
        store.save(&plan).unwrap();
        assert!(dir.path().join(".tupi/sdd/auth/result.md").exists());
    }

    #[test]
    fn a_future_schema_is_refused() {
        let dir = tempdir().unwrap();
        let store = FsSddStore::new(dir.path());
        store.save(&plan_fixture()).unwrap();
        let path = dir.path().join(".tupi/sdd/auth/meta.json");
        let mut value: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        value["schema"] = serde_json::json!(99);
        fs::write(&path, value.to_string()).unwrap();
        assert!(store.load("repo", "auth").is_err());
    }

    #[test]
    fn a_stale_lock_is_reclaimed() {
        let dir = tempdir().unwrap();
        let store = FsSddStore::new(dir.path());
        let plan = plan_fixture();
        store.save(&plan).unwrap();
        fs::write(dir.path().join(".tupi/sdd/auth/.lock"), "0").unwrap();
        assert!(store.save(&plan).is_ok());
    }

    #[test]
    fn a_plan_copied_to_another_root_loads() {
        let a = tempdir().unwrap();
        let b = tempdir().unwrap();
        FsSddStore::new(a.path()).save(&plan_fixture()).unwrap();
        copy_dir(&a.path().join(".tupi"), &b.path().join(".tupi"));
        let loaded = FsSddStore::new(b.path())
            .load("outro-repo", "auth")
            .unwrap()
            .unwrap();
        assert_eq!(loaded.slug, "auth");
    }

    #[test]
    fn active_slug_round_trips_and_clears() {
        let dir = tempdir().unwrap();
        let store = FsSddStore::new(dir.path());
        store.set_active_slug("repo", Some("auth")).unwrap();
        assert_eq!(store.active_slug("repo").unwrap().as_deref(), Some("auth"));
        store.set_active_slug("repo", None).unwrap();
        assert_eq!(store.active_slug("repo").unwrap(), None);
    }
}
