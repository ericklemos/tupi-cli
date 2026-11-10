use std::path::Path;

use redb::ReadableTable;

use serde::{Deserialize, Serialize};

pub const SCHEMA_VERSION: u64 = 1;
pub const MODELS_TTL_SECS: u64 = 24 * 60 * 60;

const META: redb::TableDefinition<&'static str, &'static [u8]> = redb::TableDefinition::new("meta");
const MODELS: redb::TableDefinition<&'static str, &'static [u8]> =
    redb::TableDefinition::new("models");

#[derive(Debug, thiserror::Error)]
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
    db: redb::Database,
}

impl Store {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, StoreError> {
        let path = path.as_ref();

        if let Some(parent) = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            std::fs::create_dir_all(parent)?;
        }

        let db = redb::Database::create(path)
            .map_err(|error| StoreError::Database(error.to_string()))?;
        let store = Self { db };
        store.ensure_schema()?;

        Ok(store)
    }

    pub fn models(&self, provider_id: &str) -> Result<Option<CachedModels>, StoreError> {
        use redb::ReadableDatabase;

        let tx = self
            .db
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

        let tx = self
            .db
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
    }

    fn ensure_schema(&self) -> Result<(), StoreError> {
        let tx = self
            .db
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
        }

        tx.commit()
            .map_err(|error| StoreError::Transaction(error.to_string()))?;
        Ok(())
    }
}

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
}
