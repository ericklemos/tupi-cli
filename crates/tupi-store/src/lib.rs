use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use thiserror::Error;

const SCHEMA_VERSION: u64 = 3;

const META: redb::TableDefinition<&'static str, &'static [u8]> = redb::TableDefinition::new("meta");
const MODELS: redb::TableDefinition<&'static str, &'static [u8]> =
    redb::TableDefinition::new("models");
const SETTINGS: redb::TableDefinition<&'static str, &'static [u8]> =
    redb::TableDefinition::new("settings");
const PROVIDERS: redb::TableDefinition<&'static str, &'static [u8]> =
    redb::TableDefinition::new("providers");

pub const MODELS_TTL_SECS: u64 = 24 * 60 * 60;

#[derive(Debug, Error)]
pub enum StoreError {
    #[error("erro de I/O: {0}")]
    Io(#[from] std::io::Error),
    #[error("erro no banco local: {0}")]
    Database(String),
    #[error("erro de transação: {0}")]
    Transaction(String),
    #[error("erro de tabela: {0}")]
    Table(String),
    #[error("schema {found} é mais novo que o suportado ({supported})")]
    FutureSchema { found: u64, supported: u64 },
    #[error("resposta local fora do formato esperado: {0}")]
    Corrupt(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CachedModels {
    pub models: Vec<String>,
    pub fetched_at_unix: u64,
}

impl CachedModels {
    pub fn is_fresh(&self, now_unix: u64, ttl_secs: u64) -> bool {
        now_unix.saturating_sub(self.fetched_at_unix) < ttl_secs
    }
}

#[derive(Debug)]
pub struct Store {
    path: PathBuf,
    lock: Mutex<()>,
}

impl Store {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, StoreError> {
        let path = path.as_ref().to_path_buf();

        if let Some(parent) = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            std::fs::create_dir_all(parent)?;
        }

        let store = Self {
            path,
            lock: Mutex::new(()),
        };
        store.ensure_schema()?;

        Ok(store)
    }

    fn with_db<T>(
        &self,
        f: impl FnOnce(&redb::Database) -> Result<T, StoreError>,
    ) -> Result<T, StoreError> {
        let _guard = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        let db = self.open_db_locked()?;
        f(&db)
    }

    fn open_db_locked(&self) -> Result<redb::Database, StoreError> {
        const MAX_ATTEMPTS: usize = 100;
        for attempt in 0..MAX_ATTEMPTS {
            match redb::Database::create(&self.path) {
                Ok(db) => return Ok(db),
                Err(error)
                    if attempt + 1 < MAX_ATTEMPTS
                        && (matches!(error, redb::DatabaseError::DatabaseAlreadyOpen)
                            || error
                                .to_string()
                                .to_ascii_lowercase()
                                .contains("already open")) =>
                {
                    std::thread::sleep(Duration::from_millis(10));
                }
                Err(error) => return Err(StoreError::Database(error.to_string())),
            }
        }
        Err(StoreError::Database(
            "banco local ocupado por outro processo".to_string(),
        ))
    }

    pub fn models(&self, provider_id: &str) -> Result<Option<CachedModels>, StoreError> {
        use redb::ReadableDatabase;

        self.with_db(|db| {
            let tx = db
                .begin_read()
                .map_err(|error| StoreError::Transaction(error.to_string()))?;
            let table = tx
                .open_table(MODELS)
                .map_err(|error| StoreError::Table(error.to_string()))?;

            let Some(raw) = table
                .get(provider_id)
                .map_err(|error| StoreError::Table(error.to_string()))?
            else {
                return Ok(None);
            };

            Ok(serde_json::from_slice(raw.value()).ok())
        })
    }

    pub fn save_models(
        &self,
        provider_id: &str,
        models: &[String],
        fetched_at_unix: u64,
    ) -> Result<(), StoreError> {
        let entry = CachedModels {
            models: models.to_vec(),
            fetched_at_unix,
        };
        let raw =
            serde_json::to_vec(&entry).map_err(|error| StoreError::Corrupt(error.to_string()))?;

        self.with_db(|db| {
            let tx = db
                .begin_write()
                .map_err(|error| StoreError::Transaction(error.to_string()))?;
            {
                let mut table = tx
                    .open_table(MODELS)
                    .map_err(|error| StoreError::Table(error.to_string()))?;
                table
                    .insert(provider_id, raw.as_slice())
                    .map_err(|error| StoreError::Table(error.to_string()))?;
            }
            tx.commit()
                .map_err(|error| StoreError::Transaction(error.to_string()))?;

            Ok(())
        })
    }

    pub fn setting(&self, key: &str) -> Result<Option<String>, StoreError> {
        use redb::ReadableDatabase;

        self.with_db(|db| {
            let tx = db
                .begin_read()
                .map_err(|error| StoreError::Transaction(error.to_string()))?;
            let table = tx
                .open_table(SETTINGS)
                .map_err(|error| StoreError::Table(error.to_string()))?;

            let Some(raw) = table
                .get(key)
                .map_err(|error| StoreError::Table(error.to_string()))?
            else {
                return Ok(None);
            };

            let value = std::str::from_utf8(raw.value())
                .map_err(|error| StoreError::Corrupt(error.to_string()))?;
            Ok(Some(value.to_string()))
        })
    }

    pub fn save_setting(&self, key: &str, value: &str) -> Result<(), StoreError> {
        self.with_db(|db| {
            let tx = db
                .begin_write()
                .map_err(|error| StoreError::Transaction(error.to_string()))?;
            {
                let mut table = tx
                    .open_table(SETTINGS)
                    .map_err(|error| StoreError::Table(error.to_string()))?;
                table
                    .insert(key, value.as_bytes())
                    .map_err(|error| StoreError::Table(error.to_string()))?;
            }
            tx.commit()
                .map_err(|error| StoreError::Transaction(error.to_string()))?;

            Ok(())
        })
    }

    pub fn provider_credentials(&self, provider_id: &str) -> Result<Option<String>, StoreError> {
        use redb::ReadableDatabase;

        self.with_db(|db| {
            let tx = db
                .begin_read()
                .map_err(|error| StoreError::Transaction(error.to_string()))?;
            let table = tx
                .open_table(PROVIDERS)
                .map_err(|error| StoreError::Table(error.to_string()))?;

            let Some(raw) = table
                .get(provider_id)
                .map_err(|error| StoreError::Table(error.to_string()))?
            else {
                return Ok(None);
            };

            let value = std::str::from_utf8(raw.value())
                .map_err(|error| StoreError::Corrupt(error.to_string()))?;
            Ok(Some(value.to_string()))
        })
    }

    pub fn save_provider_credentials(
        &self,
        provider_id: &str,
        payload: &str,
    ) -> Result<(), StoreError> {
        self.with_db(|db| {
            let tx = db
                .begin_write()
                .map_err(|error| StoreError::Transaction(error.to_string()))?;
            {
                let mut table = tx
                    .open_table(PROVIDERS)
                    .map_err(|error| StoreError::Table(error.to_string()))?;
                table
                    .insert(provider_id, payload.as_bytes())
                    .map_err(|error| StoreError::Table(error.to_string()))?;
            }
            tx.commit()
                .map_err(|error| StoreError::Transaction(error.to_string()))?;

            Ok(())
        })
    }

    pub fn connected_providers(&self) -> Result<Vec<String>, StoreError> {
        use redb::{ReadableDatabase, ReadableTable};

        self.with_db(|db| {
            let tx = db
                .begin_read()
                .map_err(|error| StoreError::Transaction(error.to_string()))?;
            let table = tx
                .open_table(PROVIDERS)
                .map_err(|error| StoreError::Table(error.to_string()))?;

            let mut providers = Vec::new();
            let iter = table
                .iter()
                .map_err(|error| StoreError::Table(error.to_string()))?;
            for entry in iter {
                let (key, _) = entry.map_err(|error| StoreError::Table(error.to_string()))?;
                providers.push(key.value().to_string());
            }
            Ok(providers)
        })
    }

    fn ensure_schema(&self) -> Result<(), StoreError> {
        use redb::ReadableTable;

        self.with_db(|db| {
            let tx = db
                .begin_write()
                .map_err(|error| StoreError::Transaction(error.to_string()))?;

            {
                let mut meta = tx
                    .open_table(META)
                    .map_err(|error| StoreError::Table(error.to_string()))?;

                let current = match meta
                    .get("schema_version")
                    .map_err(|error| StoreError::Table(error.to_string()))?
                {
                    Some(raw) => Some(
                        serde_json::from_slice::<u64>(raw.value())
                            .map_err(|error| StoreError::Corrupt(error.to_string()))?,
                    ),
                    None => None,
                };

                match current {
                    Some(found) if found > SCHEMA_VERSION => {
                        return Err(StoreError::FutureSchema {
                            found,
                            supported: SCHEMA_VERSION,
                        });
                    }
                    Some(found) if found < SCHEMA_VERSION => {
                        let raw = serde_json::to_vec(&SCHEMA_VERSION)
                            .map_err(|error| StoreError::Corrupt(error.to_string()))?;
                        meta.insert("schema_version", raw.as_slice())
                            .map_err(|error| StoreError::Table(error.to_string()))?;
                    }
                    Some(_) => {}
                    None => {
                        let raw = serde_json::to_vec(&SCHEMA_VERSION)
                            .map_err(|error| StoreError::Corrupt(error.to_string()))?;
                        meta.insert("schema_version", raw.as_slice())
                            .map_err(|error| StoreError::Table(error.to_string()))?;
                    }
                }

                tx.open_table(MODELS)
                    .map_err(|error| StoreError::Table(error.to_string()))?;
                tx.open_table(SETTINGS)
                    .map_err(|error| StoreError::Table(error.to_string()))?;
                tx.open_table(PROVIDERS)
                    .map_err(|error| StoreError::Table(error.to_string()))?;
            }

            tx.commit()
                .map_err(|error| StoreError::Transaction(error.to_string()))?;
            Ok(())
        })
    }
}

mod sdd;

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use super::*;

    const META: redb::TableDefinition<&'static str, &'static [u8]> =
        redb::TableDefinition::new("meta");
    const MODELS_TEST: redb::TableDefinition<&'static str, &'static [u8]> =
        redb::TableDefinition::new("models");

    fn temp_path(tag: &str) -> PathBuf {
        let path =
            std::env::temp_dir().join(format!("tupi-store-{tag}-{}.redb", std::process::id()));
        let _ = std::fs::remove_file(&path);
        path
    }

    fn set_schema_version_with_redb(path: &Path, version: u64) {
        let db = redb::Database::create(path).unwrap();
        let tx = db.begin_write().unwrap();
        {
            let mut table = tx.open_table(META).unwrap();
            let raw = serde_json::to_vec(&version).unwrap();
            table.insert("schema_version", raw.as_slice()).unwrap();
        }
        tx.commit().unwrap();
    }

    fn read_schema_version_with_redb(path: &Path) -> u64 {
        use redb::ReadableDatabase;

        let db = redb::Database::create(path).unwrap();
        let tx = db.begin_read().unwrap();
        let table = tx.open_table(META).unwrap();
        let raw = table.get("schema_version").unwrap().unwrap();
        serde_json::from_slice(raw.value()).unwrap()
    }

    #[test]
    fn is_fresh_bounds() {
        let entry = CachedModels {
            models: vec!["m".into()],
            fetched_at_unix: 1_000,
        };

        assert!(entry.is_fresh(1_000, 10));
        assert!(entry.is_fresh(1_009, 10));
        assert!(!entry.is_fresh(1_010, 10));
        assert!(
            entry.is_fresh(999, 10),
            "relógio para trás conta como fresco"
        );
    }

    #[test]
    fn open_creates_directory_and_file() {
        let path = std::env::temp_dir()
            .join(format!("tupi-store-dir-{}", std::process::id()))
            .join("nested")
            .join("tupi.redb");
        let _ = std::fs::remove_dir_all(path.parent().unwrap().parent().unwrap());

        let store = Store::open(&path).expect("abre");
        drop(store);

        assert!(path.exists());
    }

    #[test]
    fn open_on_garbage_file_is_an_error() {
        let path = temp_path("garbage");
        std::fs::write(&path, b"nao e um redb").unwrap();

        assert!(Store::open(&path).is_err());
    }

    #[test]
    fn future_schema_is_refused_without_writing() {
        let path = temp_path("future");
        Store::open(&path).unwrap();
        set_schema_version_with_redb(&path, 99);

        let error = Store::open(&path).expect_err("recusa");

        assert!(matches!(
            error,
            StoreError::FutureSchema {
                found: 99,
                supported: SCHEMA_VERSION
            }
        ));
        assert_eq!(read_schema_version_with_redb(&path), 99);
    }

    #[test]
    fn older_schema_is_upgraded() {
        let path = temp_path("upgrade");
        Store::open(&path).unwrap();
        set_schema_version_with_redb(&path, 1);

        Store::open(&path).expect("abre e atualiza");

        assert_eq!(read_schema_version_with_redb(&path), SCHEMA_VERSION);
    }

    fn overwrite_models_raw_with_redb(path: &Path, provider_id: &str, raw: &[u8]) {
        let db = redb::Database::create(path).unwrap();
        let tx = db.begin_write().unwrap();
        {
            let mut table = tx.open_table(MODELS_TEST).unwrap();
            table.insert(provider_id, raw).unwrap();
        }
        tx.commit().unwrap();
    }

    #[test]
    fn models_round_trip_per_provider() {
        let store = Store::open(temp_path("round")).unwrap();
        assert!(store.models("deepseek").unwrap().is_none());

        store
            .save_models("deepseek", &["a".into(), "b".into()], 42)
            .unwrap();

        let entry = store.models("deepseek").unwrap().unwrap();
        assert_eq!(entry.models, vec!["a".to_string(), "b".to_string()]);
        assert_eq!(entry.fetched_at_unix, 42);
        assert!(store.models("outro").unwrap().is_none());
    }

    #[test]
    fn corrupt_value_reads_as_absent() {
        let path = temp_path("corrupt");
        let store = Store::open(&path).unwrap();
        store.save_models("deepseek", &["a".into()], 1).unwrap();
        drop(store);

        overwrite_models_raw_with_redb(&path, "deepseek", b"{nao e json");

        let store = Store::open(&path).unwrap();
        assert!(store.models("deepseek").unwrap().is_none());
    }

    #[test]
    fn settings_round_trip_and_survive_reopen() {
        let path = temp_path("settings");
        {
            let store = Store::open(&path).unwrap();
            assert_eq!(store.setting("show_prompts_messages").unwrap(), None);

            store.save_setting("show_prompts_messages", "true").unwrap();
        }

        let store = Store::open(&path).unwrap();
        assert_eq!(
            store.setting("show_prompts_messages").unwrap().as_deref(),
            Some("true")
        );

        store
            .save_setting("show_prompts_messages", "false")
            .unwrap();
        assert_eq!(
            store.setting("show_prompts_messages").unwrap().as_deref(),
            Some("false")
        );
    }

    #[test]
    fn providers_round_trip_and_list_connected() {
        let path = temp_path("providers");
        let store = Store::open(&path).unwrap();
        assert!(store.connected_providers().unwrap().is_empty());
        assert_eq!(store.provider_credentials("antigravity").unwrap(), None);

        store
            .save_provider_credentials("antigravity", r#"{"refresh_token":"1//abc"}"#)
            .unwrap();

        assert_eq!(
            store.connected_providers().unwrap(),
            vec!["antigravity".to_string()]
        );
        assert_eq!(
            store
                .provider_credentials("antigravity")
                .unwrap()
                .as_deref(),
            Some(r#"{"refresh_token":"1//abc"}"#)
        );
    }

    #[test]
    fn two_live_stores_on_same_path_share_writes() {
        let path = temp_path("multi-session");
        let session_one = Store::open(&path).unwrap();
        let session_two = Store::open(&path).unwrap();

        session_one
            .save_provider_credentials("antigravity", r#"{"project_id":"p1"}"#)
            .unwrap();

        assert_eq!(
            session_two
                .provider_credentials("antigravity")
                .unwrap()
                .as_deref(),
            Some(r#"{"project_id":"p1"}"#)
        );
        assert_eq!(
            session_two.connected_providers().unwrap(),
            vec!["antigravity".to_string()]
        );
    }
}
