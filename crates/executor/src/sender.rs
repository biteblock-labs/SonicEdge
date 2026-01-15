use alloy::primitives::B256;
use alloy::providers::{DynProvider, Provider};
use alloy::rpc::types::TransactionRequest;
use anyhow::Result;
use tracing::info;

#[derive(Clone)]
pub struct TxSender {
    provider: DynProvider,
}

impl TxSender {
    pub fn new(provider: DynProvider) -> Self {
        Self { provider }
    }

    pub async fn send(&self, tx: TransactionRequest) -> Result<B256> {
        let pending = self.provider.send_transaction(tx).await?;
        let hash = *pending.inner().tx_hash();
        info!(%hash, "tx broadcast");
        Ok(hash)
    }
}
