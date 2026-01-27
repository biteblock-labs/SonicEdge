use alloy::primitives::{Address, B256, U256};
use anyhow::Result;
use lru::LruCache;
use serde::{Deserialize, Serialize};
use sonic_core::types::LiquidityCandidate;
use std::collections::HashMap;
use std::fs;
use std::num::NonZeroUsize;
use std::path::Path;

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
    pub exec_attempts: u8,
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
            exec_attempts: 0,
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
        if self.exec_tx_hash.is_none() {
            self.exec_attempts = self.exec_attempts.saturating_add(1);
        }
        self.state = BotState::Managing;
        self.exec_tx_hash = Some(tx_hash);
        self.last_update_ms = now_ms;
    }

    fn mark_execution_failed(&mut self, now_ms: u64) {
        if self.state == BotState::Dropped {
            return;
        }
        self.state = BotState::Qualifying;
        self.exec_tx_hash = None;
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
        self.entries.put(
            hash,
            CandidateLifecycle::new(candidate, BotState::Detecting, now_ms),
        );
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

    pub fn mark_execution_failed(&mut self, hash: B256, now_ms: u64) {
        if let Some(entry) = self.entries.get_mut(&hash) {
            entry.mark_execution_failed(now_ms);
        }
    }

    pub fn exec_attempts(&mut self, hash: B256) -> u8 {
        self.entries
            .get(&hash)
            .map(|entry| entry.exec_attempts)
            .unwrap_or(0)
    }

    pub fn candidate_snapshot(&mut self, hash: B256) -> Option<LiquidityCandidate> {
        self.entries.get(&hash).map(|entry| entry.candidate.clone())
    }

    pub fn has_execution(&mut self, hash: B256) -> bool {
        self.entries
            .get(&hash)
            .map(|entry| entry.exec_tx_hash.is_some() || entry.state == BotState::Managing)
            .unwrap_or(false)
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ExitReason {
    TakeProfit,
    StopLoss,
    MaxHold,
    EmergencyReserveDrop,
    EmergencySellSimFailure,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PositionStatus {
    Open,
    ExitSignaled { reason: ExitReason, decided_ms: u64 },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Position {
    pub add_liq_tx_hash: B256,
    pub entry_tx_hash: B256,
    pub router: Address,
    pub token: Address,
    pub base: Address,
    pub pricing_base: Address,
    #[serde(default)]
    pub token_decimals: Option<u8>,
    pub pair: Option<Address>,
    pub stable: Option<bool>,
    pub entry_base_amount: U256,
    #[serde(default)]
    pub entry_base_spent: Option<U256>,
    pub entry_token_amount: Option<U256>,
    pub entry_price_base_per_token: Option<U256>,
    #[serde(default)]
    pub entry_base_reserve: Option<U256>,
    #[serde(default)]
    pub entry_token_reserve: Option<U256>,
    pub entry_block: Option<u64>,
    pub opened_ms: u64,
    pub last_update_ms: u64,
    pub status: PositionStatus,
    pub exit_tx_hash: Option<B256>,
}

impl Position {
    pub fn is_open(&self) -> bool {
        matches!(self.status, PositionStatus::Open)
    }

    pub fn mark_exit(&mut self, reason: ExitReason, now_ms: u64) {
        self.status = PositionStatus::ExitSignaled {
            reason,
            decided_ms: now_ms,
        };
        self.last_update_ms = now_ms;
    }

    pub fn set_entry_quote(&mut self, token_amount: U256, price: U256, now_ms: u64) {
        let mut updated = false;
        if self.entry_token_amount.is_none() {
            self.entry_token_amount = Some(token_amount);
            updated = true;
        }
        if self.entry_price_base_per_token.is_none() {
            self.entry_price_base_per_token = Some(price);
            updated = true;
        }
        if updated {
            self.last_update_ms = now_ms;
        }
    }

    pub fn set_entry_reserves(
        &mut self,
        base_reserve: U256,
        token_reserve: U256,
        now_ms: u64,
    ) -> bool {
        let mut updated = false;
        if self.entry_base_reserve.is_none() {
            self.entry_base_reserve = Some(base_reserve);
            updated = true;
        }
        if self.entry_token_reserve.is_none() {
            self.entry_token_reserve = Some(token_reserve);
            updated = true;
        }
        if updated {
            self.last_update_ms = now_ms;
        }
        updated
    }
}

pub struct PositionStore {
    entries: HashMap<B256, Position>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PositionStoreSnapshot {
    positions: Vec<Position>,
}

impl PositionStore {
    pub fn new() -> Self {
        Self {
            entries: HashMap::new(),
        }
    }

    pub fn load_from(path: &Path) -> Result<Self> {
        let data = fs::read_to_string(path)?;
        let snapshot: PositionStoreSnapshot = serde_json::from_str(&data)?;
        Ok(Self::from_positions(snapshot.positions))
    }

    pub fn persist_to(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                fs::create_dir_all(parent)?;
            }
        }
        let snapshot = PositionStoreSnapshot {
            positions: self.entries.values().cloned().collect(),
        };
        let mut data = serde_json::to_string_pretty(&snapshot)?;
        data.push('\n');
        let tmp_path = path.with_extension("tmp");
        fs::write(&tmp_path, data)?;
        fs::rename(tmp_path, path)?;
        Ok(())
    }

    pub fn snapshot_positions(&self) -> Vec<Position> {
        self.entries.values().cloned().collect()
    }

    pub fn persist_snapshot(path: &Path, positions: Vec<Position>) -> Result<()> {
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                fs::create_dir_all(parent)?;
            }
        }
        let snapshot = PositionStoreSnapshot { positions };
        let mut data = serde_json::to_string_pretty(&snapshot)?;
        data.push('\n');
        let tmp_path = path.with_extension("tmp");
        fs::write(&tmp_path, data)?;
        fs::rename(tmp_path, path)?;
        Ok(())
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn insert(&mut self, position: Position) -> bool {
        let key = position.add_liq_tx_hash;
        if self.entries.contains_key(&key) {
            return false;
        }
        self.entries.insert(key, position);
        true
    }

    pub fn get(&self, key: B256) -> Option<&Position> {
        self.entries.get(&key)
    }

    pub fn get_mut(&mut self, key: B256) -> Option<&mut Position> {
        self.entries.get_mut(&key)
    }

    pub fn open_snapshot(&self) -> Vec<(B256, Position)> {
        self.entries
            .iter()
            .filter(|(_, position)| position.is_open())
            .map(|(hash, position)| (*hash, position.clone()))
            .collect()
    }

    pub fn snapshot(&self) -> Vec<(B256, Position)> {
        self.entries
            .iter()
            .map(|(hash, position)| (*hash, position.clone()))
            .collect()
    }

    pub fn mark_exit(&mut self, key: B256, reason: ExitReason, now_ms: u64) -> Option<Position> {
        if let Some(position) = self.entries.get_mut(&key) {
            position.mark_exit(reason, now_ms);
            return Some(position.clone());
        }
        None
    }

    pub fn remove(&mut self, key: B256) -> Option<Position> {
        self.entries.remove(&key)
    }

    pub fn set_exit_tx_hash(&mut self, key: B256, tx_hash: B256, now_ms: u64) -> bool {
        if let Some(position) = self.entries.get_mut(&key) {
            position.exit_tx_hash = Some(tx_hash);
            position.last_update_ms = now_ms;
            return true;
        }
        false
    }

    pub fn clear_exit_tx_hash(&mut self, key: B256, now_ms: u64) -> bool {
        if let Some(position) = self.entries.get_mut(&key) {
            position.exit_tx_hash = None;
            position.last_update_ms = now_ms;
            return true;
        }
        false
    }

    pub fn touch(&mut self, key: B256, now_ms: u64) -> bool {
        if let Some(position) = self.entries.get_mut(&key) {
            position.last_update_ms = now_ms;
            return true;
        }
        false
    }

    pub fn prune_exit_signaled(&mut self, now_ms: u64, ttl_ms: u64) -> usize {
        if ttl_ms == 0 {
            return 0;
        }
        let mut removed = 0;
        self.entries.retain(|_, position| match position.status {
            PositionStatus::ExitSignaled { decided_ms, .. } => {
                if position.exit_tx_hash.is_some() {
                    return true;
                }
                let anchor = position.last_update_ms.max(decided_ms);
                if now_ms.saturating_sub(anchor) > ttl_ms {
                    removed += 1;
                    false
                } else {
                    true
                }
            }
            _ => true,
        });
        removed
    }

    pub fn set_entry_quote(
        &mut self,
        key: B256,
        token_amount: U256,
        price: U256,
        now_ms: u64,
    ) -> bool {
        if let Some(position) = self.entries.get_mut(&key) {
            position.set_entry_quote(token_amount, price, now_ms);
            return true;
        }
        false
    }

    pub fn set_entry_reserves(
        &mut self,
        key: B256,
        base_reserve: U256,
        token_reserve: U256,
        now_ms: u64,
    ) -> bool {
        if let Some(position) = self.entries.get_mut(&key) {
            return position.set_entry_reserves(base_reserve, token_reserve, now_ms);
        }
        false
    }

    fn from_positions(positions: Vec<Position>) -> Self {
        let mut entries = HashMap::new();
        for position in positions {
            entries.insert(position.add_liq_tx_hash, position);
        }
        Self { entries }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::primitives::{address, b256};
    use std::time::{SystemTime, UNIX_EPOCH};

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

        let exec_hash = b256!("0x0202020202020202020202020202020202020202020202020202020202020202");
        store.mark_executed(hash, exec_hash, 1_300);
        let entry = store.entries.get(&hash).expect("entry");
        assert_eq!(entry.state, BotState::Managing);
        assert_eq!(entry.exec_tx_hash, Some(exec_hash));
        assert_eq!(entry.exec_attempts, 1);
        assert!(store.has_execution(hash));
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

    fn sample_position(hash: B256) -> Position {
        Position {
            add_liq_tx_hash: hash,
            entry_tx_hash: b256!(
                "0x5555555555555555555555555555555555555555555555555555555555555555"
            ),
            router: address!("0x9999999999999999999999999999999999999999"),
            token: address!("0x6666666666666666666666666666666666666666"),
            base: address!("0x7777777777777777777777777777777777777777"),
            pricing_base: address!("0x7777777777777777777777777777777777777777"),
            token_decimals: Some(18),
            pair: Some(address!("0x8888888888888888888888888888888888888888")),
            stable: None,
            entry_base_amount: U256::from(1_000u64),
            entry_base_spent: None,
            entry_token_amount: Some(U256::from(500u64)),
            entry_price_base_per_token: Some(U256::from(2_000u64)),
            entry_base_reserve: None,
            entry_token_reserve: None,
            entry_block: Some(12),
            opened_ms: 1_000,
            last_update_ms: 1_000,
            status: PositionStatus::Open,
            exit_tx_hash: None,
        }
    }

    #[test]
    fn position_store_marks_exit() {
        let mut store = PositionStore::new();
        let hash = b256!("0x9999999999999999999999999999999999999999999999999999999999999999");
        let position = sample_position(hash);

        assert!(store.insert(position.clone()));
        assert!(store.get(hash).unwrap().is_open());

        let updated = store
            .mark_exit(hash, ExitReason::TakeProfit, 2_000)
            .expect("position");
        match updated.status {
            PositionStatus::ExitSignaled { reason, decided_ms } => {
                assert_eq!(reason, ExitReason::TakeProfit);
                assert_eq!(decided_ms, 2_000);
            }
            _ => panic!("expected exit signal"),
        }
        assert!(store.get(hash).is_some());
    }

    #[test]
    fn position_store_prunes_exit_signaled() {
        let mut store = PositionStore::new();
        let hash = b256!("0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa");
        store.insert(sample_position(hash));
        store.mark_exit(hash, ExitReason::StopLoss, 1_000);

        assert_eq!(store.prune_exit_signaled(1_999, 2_000), 0);
        assert!(store.get(hash).is_some());

        assert_eq!(store.prune_exit_signaled(3_001, 2_000), 1);
        assert!(store.get(hash).is_none());
    }

    #[test]
    fn position_store_prune_uses_last_update_ms() {
        let mut store = PositionStore::new();
        let hash = b256!("0xbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb");
        store.insert(sample_position(hash));
        store.mark_exit(hash, ExitReason::StopLoss, 1_000);
        if let Some(position) = store.get_mut(hash) {
            position.last_update_ms = 2_500;
        }

        assert_eq!(store.prune_exit_signaled(3_001, 2_000), 0);
        assert!(store.get(hash).is_some());
    }

    #[test]
    fn position_store_persists_and_loads() {
        let mut store = PositionStore::new();
        let hash = b256!("0xcccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc");
        store.insert(sample_position(hash));

        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let mut path = std::env::temp_dir();
        path.push(format!(
            "sonic_positions_{}_{}.json",
            std::process::id(),
            nanos
        ));

        store.persist_to(&path).expect("persist");
        let loaded = PositionStore::load_from(&path).expect("load");
        let loaded_position = loaded.get(hash).expect("position");
        assert_eq!(
            loaded_position.entry_tx_hash,
            store.get(hash).unwrap().entry_tx_hash
        );

        let _ = std::fs::remove_file(path);
    }
}
