use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use crate::Provider;

#[derive(Clone, Default)]
pub struct ProviderRegistry {
    providers: Arc<RwLock<HashMap<String, Arc<dyn Provider>>>>,
}

impl ProviderRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(&self, provider: Arc<dyn Provider>) {
        self.providers
            .write()
            .unwrap()
            .insert(provider.id().to_string(), provider);
    }

    pub fn get(&self, id: &str) -> Option<Arc<dyn Provider>> {
        self.providers.read().unwrap().get(id).cloned()
    }

    pub fn ids(&self) -> Vec<String> {
        let mut ids: Vec<String> = self.providers.read().unwrap().keys().cloned().collect();
        ids.sort();
        ids
    }
}
