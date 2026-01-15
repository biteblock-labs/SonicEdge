use alloy::consensus::Transaction as TransactionTrait;
use alloy::network::TransactionResponse;
use alloy::primitives::B256;
use alloy::providers::{DynProvider, Provider};
use anyhow::Result;
use sonic_core::types::MempoolTx;
use sonic_core::utils::now_ms;
use std::time::Duration;

pub struct TxFetcher {
    provider: DynProvider,
    timeout: Duration,
}

impl TxFetcher {
    pub fn new(provider: DynProvider, timeout_ms: u64) -> Self {
        Self {
            provider,
            timeout: Duration::from_millis(timeout_ms),
        }
    }

    pub async fn fetch(&self, hash: B256) -> Result<Option<MempoolTx>> {
        let fut = self.provider.get_transaction_by_hash(hash);
        let tx_opt = tokio::time::timeout(self.timeout, fut).await??;
        Ok(tx_opt.map(|tx| Self::map_tx(tx, now_ms())))
    }

    fn map_tx<T>(tx: T, first_seen_ms: u64) -> MempoolTx
    where
        T: TransactionTrait + TransactionResponse,
    {
        MempoolTx {
            hash: tx.tx_hash(),
            from: tx.from(),
            to: tx.to(),
            input: tx.input().clone(),
            value: tx.value(),
            nonce: tx.nonce(),
            gas_limit: tx.gas_limit(),
            max_fee_per_gas: Some(TransactionTrait::max_fee_per_gas(&tx)),
            max_priority_fee_per_gas: tx.max_priority_fee_per_gas(),
            first_seen_ms,
        }
    }
}
