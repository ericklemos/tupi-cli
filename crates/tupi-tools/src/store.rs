use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

struct Entry {
    body: String,
    inserted_at: Instant,
    last_used: Instant,
    bytes: usize,
}

/// Store efêmero para corpos que saíram de uma resposta truncada.
///
/// Em memória e limitado: uma resposta truncada é uma segunda chance, não dado
/// durável; não pode crescer sem limite nem sobreviver ao processo.
pub struct ResponseStore {
    entries: Mutex<HashMap<String, Entry>>,
    total_bytes: Mutex<usize>,
    ttl: Duration,
    max_bytes: usize,
}

impl ResponseStore {
    pub fn new(ttl: Duration, max_bytes: usize) -> Self {
        Self {
            entries: Mutex::new(HashMap::new()),
            total_bytes: Mutex::new(0),
            ttl,
            max_bytes,
        }
    }

    /// Guarda `body` e devolve o handle que o recupera.
    pub fn insert(&self, body: String) -> String {
        self.evict_expired();

        let bytes = body.len();
        self.evict_until_fits(bytes);

        let now = Instant::now();
        let id = self.next_id();
        let mut entries = self.entries.lock().unwrap();
        entries.insert(
            id.clone(),
            Entry {
                body,
                inserted_at: now,
                last_used: now,
                bytes,
            },
        );
        *self.total_bytes.lock().unwrap() += bytes;
        id
    }

    /// Devolve o corpo guardado, ou `None` quando o handle é desconhecido ou expirou.
    pub fn get(&self, id: &str) -> Option<String> {
        self.evict_expired();

        let mut entries = self.entries.lock().unwrap();
        let entry = entries.get_mut(id)?;
        entry.last_used = Instant::now();
        Some(entry.body.clone())
    }

    fn next_id(&self) -> String {
        static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let mut hasher = blake3::Hasher::new();
        hasher.update(&n.to_le_bytes());
        hasher.update(
            &std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
                .to_le_bytes(),
        );
        format!("h_{}", &hasher.finalize().to_hex()[..8])
    }

    fn evict_expired(&self) {
        let now = Instant::now();
        let mut entries = self.entries.lock().unwrap();
        let mut total = self.total_bytes.lock().unwrap();

        let expired: Vec<String> = entries
            .iter()
            .filter(|(_, entry)| now.duration_since(entry.inserted_at) > self.ttl)
            .map(|(id, _)| id.clone())
            .collect();

        for id in expired {
            if let Some(entry) = entries.remove(&id) {
                *total -= entry.bytes;
            }
        }
    }

    fn evict_until_fits(&self, incoming_bytes: usize) {
        let mut entries = self.entries.lock().unwrap();
        let mut total = self.total_bytes.lock().unwrap();

        while *total + incoming_bytes > self.max_bytes {
            let oldest = entries
                .iter()
                .min_by_key(|(_, entry)| entry.last_used)
                .map(|(id, _)| id.clone());

            match oldest {
                Some(id) => {
                    if let Some(entry) = entries.remove(&id) {
                        *total -= entry.bytes;
                    }
                }
                None => break,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn stored_body_is_retrievable_by_handle() {
        let store = ResponseStore::new(Duration::from_secs(60), 1024 * 1024);
        let handle = store.insert("hello".to_string());

        assert!(handle.starts_with("h_"));
        assert_eq!(store.get(&handle).as_deref(), Some("hello"));
    }

    #[test]
    fn unknown_handle_returns_none() {
        let store = ResponseStore::new(Duration::from_secs(60), 1024 * 1024);
        assert_eq!(store.get("h_doesnotexist"), None);
    }

    #[test]
    fn expired_handle_returns_none() {
        let store = ResponseStore::new(Duration::from_millis(1), 1024 * 1024);
        let handle = store.insert("short lived".to_string());
        std::thread::sleep(Duration::from_millis(10));

        assert_eq!(store.get(&handle), None);
    }

    #[test]
    fn eviction_at_the_byte_ceiling_drops_least_recently_used() {
        let store = ResponseStore::new(Duration::from_secs(60), 10);
        let first = store.insert("aaaa".to_string());
        let second = store.insert("bbbb".to_string());
        assert_eq!(store.get(&first).as_deref(), Some("aaaa"));
        store.insert("cccc".to_string());

        assert_eq!(store.get(&second), None);
        assert_eq!(store.get(&first).as_deref(), Some("aaaa"));
    }

    #[test]
    fn handles_are_unique_across_inserts() {
        let store = ResponseStore::new(Duration::from_secs(60), 1024 * 1024);
        let a = store.insert("a".to_string());
        let b = store.insert("b".to_string());

        assert_ne!(a, b);
    }
}
