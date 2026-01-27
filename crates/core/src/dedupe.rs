use lru::LruCache;
use std::hash::Hash;
use std::num::NonZeroUsize;

pub struct DedupeCache<K> {
    ttl_ms: u64,
    cache: LruCache<K, u64>,
}

impl<K> DedupeCache<K>
where
    K: Hash + Eq,
{
    pub fn new(capacity: usize, ttl_ms: u64) -> Self {
        let capacity = NonZeroUsize::new(capacity.max(1)).unwrap();
        Self {
            ttl_ms,
            cache: LruCache::new(capacity),
        }
    }

    pub fn check_and_update(&mut self, key: K, now_ms: u64) -> bool {
        if let Some(expires_at) = self.cache.get_mut(&key) {
            if now_ms <= *expires_at {
                *expires_at = now_ms.saturating_add(self.ttl_ms);
                return false;
            }
        }

        let expires_at = now_ms.saturating_add(self.ttl_ms);
        self.cache.put(key, expires_at);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::DedupeCache;

    #[test]
    fn dedupe_blocks_within_ttl() {
        let mut cache = DedupeCache::new(4, 100);
        assert!(cache.check_and_update(42u64, 1_000));
        assert!(!cache.check_and_update(42u64, 1_050));
    }

    #[test]
    fn dedupe_expires_after_ttl() {
        let mut cache = DedupeCache::new(4, 100);
        assert!(cache.check_and_update(7u64, 1_000));
        assert!(cache.check_and_update(7u64, 1_200));
    }

    #[test]
    fn dedupe_refreshes_ttl_on_hit() {
        let mut cache = DedupeCache::new(4, 100);
        assert!(cache.check_and_update(9u64, 1_000));
        assert!(!cache.check_and_update(9u64, 1_050));
        assert!(!cache.check_and_update(9u64, 1_120));
        assert!(cache.check_and_update(9u64, 1_300));
    }
}
