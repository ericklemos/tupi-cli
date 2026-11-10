use tupi_domain::index::Index;

use super::{INDEX, Store, StoreError};

impl Store {
    pub fn save_index(&self, repo_key: &str, index: &Index) -> Result<(), StoreError> {
        let raw =
            serde_json::to_vec(index).map_err(|error| StoreError::Corrupt(error.to_string()))?;

        let tx = self
            .db
            .begin_write()
            .map_err(|error| StoreError::Transaction(error.to_string()))?;
        {
            let mut table = tx
                .open_table(INDEX)
                .map_err(|error| StoreError::Table(error.to_string()))?;
            table
                .insert(repo_key, raw.as_slice())
                .map_err(|error| StoreError::Table(error.to_string()))?;
        }
        tx.commit()
            .map_err(|error| StoreError::Transaction(error.to_string()))?;

        Ok(())
    }

    pub fn load_index(&self, repo_key: &str) -> Result<Option<Index>, StoreError> {
        use redb::ReadableDatabase;

        let tx = self
            .db
            .begin_read()
            .map_err(|error| StoreError::Transaction(error.to_string()))?;
        let table = tx
            .open_table(INDEX)
            .map_err(|error| StoreError::Table(error.to_string()))?;

        let Some(raw) = table
            .get(repo_key)
            .map_err(|error| StoreError::Table(error.to_string()))?
        else {
            return Ok(None);
        };

        Ok(serde_json::from_slice(raw.value()).ok())
    }
}
