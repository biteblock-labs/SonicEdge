use crate::pair::{
    contract_exists,
    derive_pair_address_solidly,
    derive_pair_address_v2,
    get_pair_address,
    get_pair_address_solidly,
    get_pair_code_hash,
    get_pair_tokens,
};
use alloy::primitives::{Address, B256};
use alloy::providers::DynProvider;
use anyhow::Result;
use lru::LruCache;
use sonic_core::utils::now_ms;
use std::collections::HashMap;
use std::num::NonZeroUsize;

#[derive(Debug, Clone)]
pub struct PairMetadata {
    pub factory: Address,
    pub pair: Address,
    pub token0: Address,
    pub token1: Address,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct PairKey {
    factory: Address,
    token0: Address,
    token1: Address,
    stable: Option<bool>,
}

impl PairKey {
    fn new(factory: Address, token_a: Address, token_b: Address, stable: Option<bool>) -> Self {
        let (token0, token1) = if token_a < token_b {
            (token_a, token_b)
        } else {
            (token_b, token_a)
        };
        Self {
            factory,
            token0,
            token1,
            stable,
        }
    }
}

#[derive(Debug, Clone)]
struct CacheEntry {
    value: Option<PairMetadata>,
    expires_at_ms: u64,
}

pub struct PairMetadataCache {
    positive_ttl_ms: u64,
    negative_ttl_ms: u64,
    entries: LruCache<PairKey, CacheEntry>,
    pair_code_hashes: HashMap<Address, Option<B256>>,
}

impl PairMetadataCache {
    pub fn new(
        capacity: usize,
        positive_ttl_ms: u64,
        negative_ttl_ms: u64,
        pair_code_hashes: HashMap<Address, B256>,
    ) -> Self {
        let capacity = NonZeroUsize::new(capacity.max(1)).unwrap();
        let pair_code_hashes = pair_code_hashes
            .into_iter()
            .map(|(factory, hash)| (factory, Some(hash)))
            .collect();
        Self {
            positive_ttl_ms,
            negative_ttl_ms,
            entries: LruCache::new(capacity),
            pair_code_hashes,
        }
    }

    pub async fn resolve(
        &mut self,
        provider: &DynProvider,
        factories: &[Address],
        token_a: Address,
        token_b: Address,
        stable: Option<bool>,
    ) -> Result<Option<PairMetadata>> {
        let now_ms = now_ms();
        self.resolve_with_now(provider, factories, token_a, token_b, stable, now_ms)
            .await
    }

    async fn resolve_with_now(
        &mut self,
        provider: &DynProvider,
        factories: &[Address],
        token_a: Address,
        token_b: Address,
        stable: Option<bool>,
        now_ms: u64,
    ) -> Result<Option<PairMetadata>> {
        for &factory in factories {
            let key = PairKey::new(factory, token_a, token_b, stable);
            if let Some(cached) = self.get_cached(key, now_ms) {
                if let Some(metadata) = cached {
                    return Ok(Some(metadata));
                }
                continue;
            }

            let pair = match stable {
                Some(stable) => {
                    get_pair_address_solidly(provider, factory, token_a, token_b, stable).await?
                }
                None => get_pair_address(provider, factory, token_a, token_b).await?,
            };
            let pair = match pair {
                Some(pair) => pair,
                None => {
                    if let Some(metadata) = self
                        .resolve_with_create2(provider, factory, token_a, token_b, stable)
                        .await?
                    {
                        self.insert_cached(key, Some(metadata.clone()), now_ms);
                        return Ok(Some(metadata));
                    }
                    self.insert_cached(key, None, now_ms);
                    continue;
                }
            };

            let (token0, token1) = match get_pair_tokens(provider, pair).await? {
                Some(tokens) => tokens,
                None => {
                    self.insert_cached(key, None, now_ms);
                    continue;
                }
            };

            let metadata = PairMetadata {
                factory,
                pair,
                token0,
                token1,
            };
            self.insert_cached(key, Some(metadata.clone()), now_ms);
            return Ok(Some(metadata));
        }

        Ok(None)
    }

    async fn resolve_with_create2(
        &mut self,
        provider: &DynProvider,
        factory: Address,
        token_a: Address,
        token_b: Address,
        stable: Option<bool>,
    ) -> Result<Option<PairMetadata>> {
        let init_code_hash = match self.code_hash_for_factory(provider, factory).await? {
            Some(hash) => hash,
            None => return Ok(None),
        };

        let pair = match stable {
            Some(stable) => {
                derive_pair_address_solidly(factory, token_a, token_b, stable, init_code_hash)
            }
            None => derive_pair_address_v2(factory, token_a, token_b, init_code_hash),
        };

        if !contract_exists(provider, pair).await? {
            return Ok(None);
        }

        let (token0, token1) = match get_pair_tokens(provider, pair).await? {
            Some(tokens) => tokens,
            None => return Ok(None),
        };

        Ok(Some(PairMetadata {
            factory,
            pair,
            token0,
            token1,
        }))
    }

    async fn code_hash_for_factory(
        &mut self,
        provider: &DynProvider,
        factory: Address,
    ) -> Result<Option<B256>> {
        if let Some(entry) = self.pair_code_hashes.get(&factory) {
            return Ok(*entry);
        }

        let fetched = match get_pair_code_hash(provider, factory).await {
            Ok(hash) => hash,
            Err(_) => None,
        };
        self.pair_code_hashes.insert(factory, fetched);
        Ok(fetched)
    }

    fn get_cached(&mut self, key: PairKey, now_ms: u64) -> Option<Option<PairMetadata>> {
        if let Some(entry) = self.entries.get(&key) {
            if now_ms <= entry.expires_at_ms {
                return Some(entry.value.clone());
            }
        }
        self.entries.pop(&key);
        None
    }

    fn insert_cached(&mut self, key: PairKey, value: Option<PairMetadata>, now_ms: u64) {
        let ttl_ms = if value.is_some() {
            self.positive_ttl_ms
        } else {
            self.negative_ttl_ms
        };
        let expires_at_ms = now_ms.saturating_add(ttl_ms);
        self.entries.put(
            key,
            CacheEntry {
                value,
                expires_at_ms,
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::primitives::address;
    use std::collections::HashMap;

    #[test]
    fn cache_hits_for_swapped_tokens() {
        let factory = address!("0x0000000000000000000000000000000000000001");
        let token_a = address!("0x0000000000000000000000000000000000000002");
        let token_b = address!("0x0000000000000000000000000000000000000003");
        let pair = address!("0x0000000000000000000000000000000000000004");
        let metadata = PairMetadata {
            factory,
            pair,
            token0: token_a,
            token1: token_b,
        };

        let mut cache = PairMetadataCache::new(2, 1000, 200, HashMap::new());
        cache.insert_cached(
            PairKey::new(factory, token_a, token_b, None),
            Some(metadata.clone()),
            1_000,
        );

        let cached = cache
            .get_cached(PairKey::new(factory, token_b, token_a, None), 1_500)
            .expect("cache hit")
            .expect("metadata");

        assert_eq!(cached.pair, metadata.pair);
        assert_eq!(cached.token0, metadata.token0);
        assert_eq!(cached.token1, metadata.token1);
    }

    #[test]
    fn cache_expires_after_ttl() {
        let factory = address!("0x0000000000000000000000000000000000000001");
        let token_a = address!("0x0000000000000000000000000000000000000002");
        let token_b = address!("0x0000000000000000000000000000000000000003");
        let pair = address!("0x0000000000000000000000000000000000000004");
        let metadata = PairMetadata {
            factory,
            pair,
            token0: token_a,
            token1: token_b,
        };

        let mut cache = PairMetadataCache::new(2, 50, 10, HashMap::new());
        cache.insert_cached(
            PairKey::new(factory, token_a, token_b, None),
            Some(metadata),
            1_000,
        );

        let cached = cache.get_cached(PairKey::new(factory, token_a, token_b, None), 1_051);
        assert!(cached.is_none());
    }

    #[test]
    fn cache_can_store_negative_results() {
        let factory = address!("0x0000000000000000000000000000000000000001");
        let token_a = address!("0x0000000000000000000000000000000000000002");
        let token_b = address!("0x0000000000000000000000000000000000000003");

        let mut cache = PairMetadataCache::new(2, 1000, 1000, HashMap::new());
        cache.insert_cached(PairKey::new(factory, token_a, token_b, None), None, 1_000);

        let cached = cache
            .get_cached(PairKey::new(factory, token_a, token_b, None), 1_500)
            .expect("cache hit");
        assert!(cached.is_none());
    }

    #[test]
    fn negative_ttl_expires_sooner() {
        let factory = address!("0x0000000000000000000000000000000000000001");
        let token_a = address!("0x0000000000000000000000000000000000000002");
        let token_b = address!("0x0000000000000000000000000000000000000003");

        let mut cache = PairMetadataCache::new(2, 1000, 100, HashMap::new());
        cache.insert_cached(PairKey::new(factory, token_a, token_b, None), None, 1_000);

        let cached = cache.get_cached(PairKey::new(factory, token_a, token_b, None), 1_101);
        assert!(cached.is_none());
    }

    #[test]
    fn stable_and_volatile_keys_do_not_collide() {
        let factory = address!("0x0000000000000000000000000000000000000001");
        let token_a = address!("0x0000000000000000000000000000000000000002");
        let token_b = address!("0x0000000000000000000000000000000000000003");
        let pair = address!("0x0000000000000000000000000000000000000004");
        let metadata = PairMetadata {
            factory,
            pair,
            token0: token_a,
            token1: token_b,
        };

        let mut cache = PairMetadataCache::new(2, 1000, 1000, HashMap::new());
        cache.insert_cached(
            PairKey::new(factory, token_a, token_b, Some(true)),
            Some(metadata.clone()),
            1_000,
        );

        let cached = cache.get_cached(PairKey::new(factory, token_a, token_b, Some(false)), 1_000);
        assert!(cached.is_none());
    }
}
