use crate::pair::{
    contract_exists, derive_pair_address_solidly, derive_pair_address_v2, get_pair_address,
    get_pair_address_solidly, get_pair_code_hash, get_pair_tokens,
};
use alloy::primitives::{Address, B256};
use alloy::providers::DynProvider;
use anyhow::Result;
use lru::LruCache;
use sonic_core::utils::now_ms;
use std::collections::HashMap;
use std::num::NonZeroUsize;
use tracing::warn;

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

#[derive(Debug, Clone)]
struct CodeHashEntry {
    value: Option<B256>,
    expires_at_ms: u64,
}

const CODE_HASH_NEGATIVE_TTL_MS: u64 = 30_000;
const CODE_HASH_POSITIVE_TTL_MS: u64 = 600_000;

pub struct PairMetadataCache {
    positive_ttl_ms: u64,
    negative_ttl_ms: u64,
    entries: LruCache<PairKey, CacheEntry>,
    pair_code_hashes: HashMap<Address, CodeHashEntry>,
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
            .map(|(factory, hash)| {
                (
                    factory,
                    CodeHashEntry {
                        value: Some(hash),
                        expires_at_ms: u64::MAX,
                    },
                )
            })
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

    pub fn invalidate(
        &mut self,
        factory: Address,
        token_a: Address,
        token_b: Address,
        stable: Option<bool>,
    ) {
        let key = PairKey::new(factory, token_a, token_b, stable);
        self.entries.pop(&key);
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
                    match get_pair_address_solidly(provider, factory, token_a, token_b, stable)
                        .await
                    {
                        Ok(pair) => pair,
                        Err(err) => {
                            warn!(?err, factory = %factory, "getPair failed; falling back to CREATE2");
                            None
                        }
                    }
                }
                None => match get_pair_address(provider, factory, token_a, token_b).await {
                    Ok(pair) => pair,
                    Err(err) => {
                        warn!(?err, factory = %factory, "getPair failed; falling back to CREATE2");
                        None
                    }
                },
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

            let (token0, token1) = match get_pair_tokens(provider, pair).await {
                Ok(Some(tokens)) => tokens,
                Ok(None) => {
                    self.insert_cached(key, None, now_ms);
                    continue;
                }
                Err(err) => {
                    warn!(?err, factory = %factory, pair = %pair, "pair token query failed");
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

    pub async fn predict_pair_address(
        &mut self,
        provider: &DynProvider,
        factory: Address,
        token_a: Address,
        token_b: Address,
        stable: Option<bool>,
    ) -> Result<Option<Address>> {
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
        Ok(Some(pair))
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

        let (token0, token1) = match get_pair_tokens(provider, pair).await {
            Ok(Some(tokens)) => tokens,
            Ok(None) => return Ok(None),
            Err(err) => {
                warn!(?err, factory = %factory, pair = %pair, "derived pair token query failed");
                return Ok(None);
            }
        };

        if !tokens_match(token_a, token_b, token0, token1) {
            warn!(factory = %factory, pair = %pair, "derived pair tokens mismatch");
            return Ok(None);
        }

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
        let now_ms = now_ms();
        let existing = self.pair_code_hashes.get(&factory).cloned();
        if let Some(entry) = &existing {
            if now_ms <= entry.expires_at_ms {
                return Ok(entry.value);
            }
        }

        let fetched = match get_pair_code_hash(provider, factory).await {
            Ok(hash) => hash,
            Err(err) => {
                warn!(?err, factory = %factory, "pair code hash lookup failed; using cached value if available");
                if let Some(entry) = existing {
                    let refreshed = if entry.value.is_some() {
                        CodeHashEntry {
                            value: entry.value,
                            expires_at_ms: now_ms.saturating_add(CODE_HASH_POSITIVE_TTL_MS),
                        }
                    } else {
                        CodeHashEntry {
                            value: None,
                            expires_at_ms: now_ms.saturating_add(CODE_HASH_NEGATIVE_TTL_MS),
                        }
                    };
                    self.pair_code_hashes.insert(factory, refreshed);
                    return Ok(entry.value);
                }
                self.pair_code_hashes.insert(
                    factory,
                    CodeHashEntry {
                        value: None,
                        expires_at_ms: now_ms.saturating_add(CODE_HASH_NEGATIVE_TTL_MS),
                    },
                );
                return Ok(None);
            }
        };

        let entry = if let Some(hash) = fetched {
            CodeHashEntry {
                value: Some(hash),
                expires_at_ms: now_ms.saturating_add(CODE_HASH_POSITIVE_TTL_MS),
            }
        } else if let Some(existing) = existing {
            warn!(factory = %factory, "pair code hash unavailable; keeping last known value");
            if existing.value.is_some() {
                CodeHashEntry {
                    value: existing.value,
                    expires_at_ms: now_ms.saturating_add(CODE_HASH_POSITIVE_TTL_MS),
                }
            } else {
                CodeHashEntry {
                    value: None,
                    expires_at_ms: now_ms.saturating_add(CODE_HASH_NEGATIVE_TTL_MS),
                }
            }
        } else {
            warn!(factory = %factory, "pair code hash unavailable; CREATE2 fallback disabled temporarily");
            CodeHashEntry {
                value: None,
                expires_at_ms: now_ms.saturating_add(CODE_HASH_NEGATIVE_TTL_MS),
            }
        };
        let value = entry.value;
        self.pair_code_hashes.insert(factory, entry);
        Ok(value)
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

fn tokens_match(token_a: Address, token_b: Address, token0: Address, token1: Address) -> bool {
    let (want0, want1) = sort_tokens(token_a, token_b);
    let (have0, have1) = sort_tokens(token0, token1);
    want0 == have0 && want1 == have1
}

fn sort_tokens(token_a: Address, token_b: Address) -> (Address, Address) {
    if token_a < token_b {
        (token_a, token_b)
    } else {
        (token_b, token_a)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::primitives::{address, b256};
    use alloy::providers::{Provider, ProviderBuilder};
    use alloy::transports::mock::Asserter;
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
    fn invalidate_removes_cached_entry() {
        let factory = address!("0x0000000000000000000000000000000000000001");
        let token_a = address!("0x0000000000000000000000000000000000000002");
        let token_b = address!("0x0000000000000000000000000000000000000003");
        let mut cache = PairMetadataCache::new(2, 1000, 1000, HashMap::new());
        let key = PairKey::new(factory, token_a, token_b, None);

        cache.insert_cached(key, None, 1_000);
        assert!(cache.get_cached(key, 1_010).is_some());

        cache.invalidate(factory, token_a, token_b, None);
        assert!(cache.get_cached(key, 1_010).is_none());
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

    #[test]
    fn tokens_match_accepts_sorted_and_unsorted() {
        let token_a = address!("0x00000000000000000000000000000000000000aa");
        let token_b = address!("0x00000000000000000000000000000000000000bb");
        let token0 = address!("0x00000000000000000000000000000000000000aa");
        let token1 = address!("0x00000000000000000000000000000000000000bb");

        assert!(tokens_match(token_a, token_b, token0, token1));
        assert!(tokens_match(token_b, token_a, token0, token1));
        assert!(tokens_match(token_a, token_b, token1, token0));
    }

    #[test]
    fn tokens_match_rejects_mismatch() {
        let token_a = address!("0x00000000000000000000000000000000000000aa");
        let token_b = address!("0x00000000000000000000000000000000000000bb");
        let token0 = address!("0x00000000000000000000000000000000000000aa");
        let token1 = address!("0x00000000000000000000000000000000000000cc");

        assert!(!tokens_match(token_a, token_b, token0, token1));
    }

    #[tokio::test]
    async fn predict_pair_address_uses_cached_code_hash() {
        let factory = address!("0x0000000000000000000000000000000000000001");
        let token_a = address!("0x00000000000000000000000000000000000000aa");
        let token_b = address!("0x00000000000000000000000000000000000000bb");
        let init_code_hash =
            b256!("0x0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef");
        let mut cache =
            PairMetadataCache::new(2, 1000, 100, HashMap::from([(factory, init_code_hash)]));
        let provider = ProviderBuilder::new()
            .connect_mocked_client(Asserter::new())
            .erased();

        let predicted = cache
            .predict_pair_address(&provider, factory, token_a, token_b, None)
            .await
            .unwrap()
            .expect("pair predicted");
        let expected = derive_pair_address_v2(factory, token_a, token_b, init_code_hash);

        assert_eq!(predicted, expected);
    }
}
