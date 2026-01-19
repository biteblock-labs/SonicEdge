use alloy::primitives::{B256, U256};
use lru::LruCache;
use sonic_core::types::LiquidityCandidate;
use std::num::NonZeroUsize;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BotState {
    Detecting,
    Qualifying,
    Executing,
    Managing,
    Dropped,
}

#[derive(Debug, Clone)]
pub struct FillRecord {
    pub tx_hash: B256,
    pub block_number: u64,
    pub amount_out: U256,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DropKind {
    Transient,
    Terminal,
}

#[derive(Debug, Clone)]
pub struct CandidateLifecycle {
    pub candidate: LiquidityCandidate,
    pub state: BotState,
    pub first_seen_ms: u64,
    pub last_update_ms: u64,
    pub drop_reason: Option<String>,
    pub drop_kind: Option<DropKind>,
    pub exec_tx_hash: Option<B256>,
}

impl CandidateLifecycle {
    pub fn new(candidate: LiquidityCandidate, state: BotState, now_ms: u64) -> Self {
        Self {
            first_seen_ms: candidate.first_seen_ms,
            candidate,
            state,
            last_update_ms: now_ms,
            drop_reason: None,
            drop_kind: None,
            exec_tx_hash: None,
        }
    }

    fn update(&mut self, candidate: LiquidityCandidate, state: BotState, now_ms: u64) {
        if self.state == BotState::Dropped {
            return;
        }
        self.candidate = candidate;
        self.state = state;
        self.last_update_ms = now_ms;
    }

    fn mark_dropped_with_kind(&mut self, reason: String, kind: DropKind, now_ms: u64) {
        self.state = BotState::Dropped;
        self.drop_reason = Some(reason);
        self.drop_kind = Some(kind);
        self.last_update_ms = now_ms;
    }

    fn mark_executed(&mut self, tx_hash: B256, now_ms: u64) {
        self.state = BotState::Managing;
        self.exec_tx_hash = Some(tx_hash);
        self.last_update_ms = now_ms;
    }
}

pub struct CandidateStore {
    entries: LruCache<B256, CandidateLifecycle>,
    ttl_ms: u64,
}

impl CandidateStore {
    pub fn new(capacity: usize, ttl_ms: u64) -> Self {
        let capacity = NonZeroUsize::new(capacity.max(1)).unwrap();
        Self {
            entries: LruCache::new(capacity),
            ttl_ms,
        }
    }

    pub fn track_detected(&mut self, candidate: LiquidityCandidate, now_ms: u64) {
        let hash = candidate.add_liq_tx_hash;
        if let Some(entry) = self.entries.get_mut(&hash) {
            if entry.state == BotState::Dropped {
                if entry.drop_kind == Some(DropKind::Transient) {
                    *entry = CandidateLifecycle::new(candidate, BotState::Detecting, now_ms);
                }
                return;
            }
            entry.update(candidate, BotState::Detecting, now_ms);
            return;
        }
        self.entries
            .put(hash, CandidateLifecycle::new(candidate, BotState::Detecting, now_ms));
    }

    pub fn set_state(&mut self, candidate: LiquidityCandidate, state: BotState, now_ms: u64) {
        self.upsert(candidate, state, now_ms);
    }

    pub fn drop_terminal(&mut self, hash: B256, reason: impl Into<String>, now_ms: u64) {
        self.drop_with_kind(hash, reason, DropKind::Terminal, now_ms);
    }

    pub fn drop_transient(&mut self, hash: B256, reason: impl Into<String>, now_ms: u64) {
        self.drop_with_kind(hash, reason, DropKind::Transient, now_ms);
    }

    pub fn mark_executed(&mut self, hash: B256, tx_hash: B256, now_ms: u64) {
        if let Some(entry) = self.entries.get_mut(&hash) {
            entry.mark_executed(tx_hash, now_ms);
        }
    }

    pub fn is_terminal(&mut self, hash: B256) -> bool {
        self.entries
            .get(&hash)
            .map(|entry| {
                entry.state == BotState::Dropped && entry.drop_kind == Some(DropKind::Terminal)
            })
            .unwrap_or(false)
    }

    pub fn prune(&mut self, now_ms: u64) {
        if self.ttl_ms == 0 {
            return;
        }
        let mut expired = Vec::new();
        for (hash, entry) in self.entries.iter() {
            if now_ms.saturating_sub(entry.last_update_ms) > self.ttl_ms {
                expired.push(*hash);
            }
        }
        for hash in expired {
            self.entries.pop(&hash);
        }
    }

    fn upsert(&mut self, candidate: LiquidityCandidate, state: BotState, now_ms: u64) {
        let hash = candidate.add_liq_tx_hash;
        if let Some(entry) = self.entries.get_mut(&hash) {
            entry.update(candidate, state, now_ms);
            return;
        }
        self.entries
            .put(hash, CandidateLifecycle::new(candidate, state, now_ms));
    }

    fn drop_with_kind(
        &mut self,
        hash: B256,
        reason: impl Into<String>,
        kind: DropKind,
        now_ms: u64,
    ) {
        if let Some(entry) = self.entries.get_mut(&hash) {
            entry.mark_dropped_with_kind(reason.into(), kind, now_ms);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::primitives::{address, b256};

    fn sample_candidate(hash: B256) -> LiquidityCandidate {
        LiquidityCandidate {
            token: address!("0x1000000000000000000000000000000000000001"),
            base: address!("0x2000000000000000000000000000000000000002"),
            router: address!("0x3000000000000000000000000000000000000003"),
            factory: None,
            pair: None,
            stable: None,
            add_liq_tx_hash: hash,
            first_seen_ms: 1_000,
            implied_liquidity: U256::from(1_000u64),
        }
    }

    #[test]
    fn candidate_store_tracks_transitions() {
        let mut store = CandidateStore::new(4, 10_000);
        let hash = b256!("0x0101010101010101010101010101010101010101010101010101010101010101");
        let candidate = sample_candidate(hash);

        store.track_detected(candidate.clone(), 1_000);
        let entry = store.entries.get(&hash).expect("entry");
        assert_eq!(entry.state, BotState::Detecting);

        let mut updated = candidate.clone();
        updated.pair = Some(address!("0x4000000000000000000000000000000000000004"));
        store.set_state(updated.clone(), BotState::Qualifying, 1_100);
        let entry = store.entries.get(&hash).expect("entry");
        assert_eq!(entry.state, BotState::Qualifying);
        assert_eq!(entry.candidate.pair, updated.pair);

        store.set_state(updated.clone(), BotState::Executing, 1_200);
        let entry = store.entries.get(&hash).expect("entry");
        assert_eq!(entry.state, BotState::Executing);

        let exec_hash =
            b256!("0x0202020202020202020202020202020202020202020202020202020202020202");
        store.mark_executed(hash, exec_hash, 1_300);
        let entry = store.entries.get(&hash).expect("entry");
        assert_eq!(entry.state, BotState::Managing);
        assert_eq!(entry.exec_tx_hash, Some(exec_hash));
    }

    #[test]
    fn candidate_store_prunes_expired() {
        let mut store = CandidateStore::new(4, 100);
        let hash = b256!("0x1111111111111111111111111111111111111111111111111111111111111111");
        store.track_detected(sample_candidate(hash), 1_000);
        store.prune(1_101);
        assert!(store.entries.get(&hash).is_none());
    }

    #[test]
    fn candidate_store_drops_with_reason() {
        let mut store = CandidateStore::new(4, 10_000);
        let hash = b256!("0x2222222222222222222222222222222222222222222222222222222222222222");
        store.track_detected(sample_candidate(hash), 1_000);
        store.drop_terminal(hash, "risk rejected", 1_050);
        let entry = store.entries.get(&hash).expect("entry");
        assert_eq!(entry.state, BotState::Dropped);
        assert_eq!(entry.drop_reason.as_deref(), Some("risk rejected"));
        assert_eq!(entry.drop_kind, Some(DropKind::Terminal));
    }

    #[test]
    fn candidate_store_reenters_on_transient_drop() {
        let mut store = CandidateStore::new(4, 10_000);
        let hash = b256!("0x3333333333333333333333333333333333333333333333333333333333333333");
        store.track_detected(sample_candidate(hash), 1_000);
        store.drop_transient(hash, "pair unresolved", 1_010);
        store.track_detected(sample_candidate(hash), 1_020);
        let entry = store.entries.get(&hash).expect("entry");
        assert_eq!(entry.state, BotState::Detecting);
        assert!(entry.drop_reason.is_none());
        assert!(entry.drop_kind.is_none());
    }

    #[test]
    fn candidate_store_terminal_drop_blocks_reentry() {
        let mut store = CandidateStore::new(4, 10_000);
        let hash = b256!("0x4444444444444444444444444444444444444444444444444444444444444444");
        store.track_detected(sample_candidate(hash), 1_000);
        store.drop_terminal(hash, "risk rejected", 1_010);
        assert!(store.is_terminal(hash));
        store.track_detected(sample_candidate(hash), 1_020);
        let entry = store.entries.get(&hash).expect("entry");
        assert_eq!(entry.state, BotState::Dropped);
        assert_eq!(entry.drop_reason.as_deref(), Some("risk rejected"));
        assert_eq!(entry.drop_kind, Some(DropKind::Terminal));
    }
}
