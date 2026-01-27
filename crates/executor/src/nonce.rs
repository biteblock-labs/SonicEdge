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
        let pending = provider.get_transaction_count(address).pending().await?;
        let current = self.next.load(Ordering::SeqCst);
        let next = pending.max(current);
        self.set(next);
        Ok(next)
    }

    pub async fn sync_allow_decrease(
        &self,
        provider: &DynProvider,
        address: Address,
    ) -> Result<u64> {
        let pending = provider.get_transaction_count(address).pending().await?;
        self.set(pending);
        Ok(pending)
    }
}

#[cfg(test)]
mod tests {
    use super::NonceManager;
    use alloy::primitives::address;
    use alloy::providers::{Provider, ProviderBuilder};
    use alloy::transports::mock::Asserter;

    #[tokio::test]
    async fn sync_sets_next_nonce() {
        let asserter = Asserter::new();
        let provider = ProviderBuilder::new()
            .connect_mocked_client(asserter.clone())
            .erased();
        asserter.push_success(&7u64);

        let manager = NonceManager::new(0);
        let nonce = manager
            .sync(
                &provider,
                address!("0x1000000000000000000000000000000000000001"),
            )
            .await
            .unwrap();

        assert_eq!(nonce, 7);
        assert_eq!(manager.next_nonce(), 7);
        assert_eq!(manager.next_nonce(), 8);
        assert!(asserter.read_q().is_empty());
    }

    #[tokio::test]
    async fn sync_does_not_decrease_nonce() {
        let asserter = Asserter::new();
        let provider = ProviderBuilder::new()
            .connect_mocked_client(asserter.clone())
            .erased();
        asserter.push_success(&5u64);

        let manager = NonceManager::new(0);
        manager.set(9);
        let nonce = manager
            .sync(
                &provider,
                address!("0x1000000000000000000000000000000000000001"),
            )
            .await
            .unwrap();

        assert_eq!(nonce, 9);
        assert_eq!(manager.next_nonce(), 9);
        assert!(asserter.read_q().is_empty());
    }

    #[tokio::test]
    async fn sync_allow_decrease_can_lower_nonce() {
        let asserter = Asserter::new();
        let provider = ProviderBuilder::new()
            .connect_mocked_client(asserter.clone())
            .erased();
        asserter.push_success(&3u64);

        let manager = NonceManager::new(0);
        manager.set(7);
        let nonce = manager
            .sync_allow_decrease(
                &provider,
                address!("0x1000000000000000000000000000000000000001"),
            )
            .await
            .unwrap();

        assert_eq!(nonce, 3);
        assert_eq!(manager.next_nonce(), 3);
        assert!(asserter.read_q().is_empty());
    }
}
