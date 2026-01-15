use alloy::primitives::Address;
use alloy::providers::{DynProvider, Provider};
use anyhow::Result;
use std::sync::atomic::{AtomicU64, Ordering};

pub struct NonceManager {
    next: AtomicU64,
}

impl NonceManager {
    pub fn new(start: u64) -> Self {
        Self {
            next: AtomicU64::new(start),
        }
    }

    pub fn next_nonce(&self) -> u64 {
        self.next.fetch_add(1, Ordering::SeqCst)
    }

    pub fn set(&self, value: u64) {
        self.next.store(value, Ordering::SeqCst);
    }

    pub async fn sync(&self, provider: &DynProvider, address: Address) -> Result<u64> {
        let nonce = provider.get_transaction_count(address).await?;
        self.set(nonce);
        Ok(nonce)
    }
}
