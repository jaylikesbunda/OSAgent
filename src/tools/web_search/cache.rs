use dashmap::DashMap;
use std::time::{Duration, Instant};

#[derive(Debug)]
struct CacheEntry<V> {
    value: V,
    expires_at: Instant,
}

#[derive(Debug)]
pub struct SearchCache<V> {
    ttl: Duration,
    entries: DashMap<String, CacheEntry<V>>,
}

impl<V: Clone> SearchCache<V> {
    pub fn new(ttl: Duration) -> Self {
        Self {
            ttl,
            entries: DashMap::new(),
        }
    }

    pub fn get(&self, key: &str) -> Option<V> {
        let now = Instant::now();
        let result = self.entries.get(key).and_then(|entry| {
            if entry.expires_at > now {
                Some(entry.value.clone())
            } else {
                None
            }
        });
        if result.is_none() {
            self.entries.remove(key);
        }
        result
    }

    pub fn insert(&self, key: String, value: V) {
        self.entries.insert(
            key,
            CacheEntry {
                value,
                expires_at: Instant::now() + self.ttl,
            },
        );
    }
}
