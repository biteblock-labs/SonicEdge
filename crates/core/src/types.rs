use alloy::primitives::{Address, Bytes, B256, U256};

#[derive(Debug, Clone)]
pub struct MempoolTx {
    pub hash: B256,
    pub from: Address,
    pub to: Option<Address>,
    pub input: Bytes,
    pub value: U256,
    pub nonce: u64,
    pub gas_limit: u64,
    pub max_fee_per_gas: Option<u128>,
    pub max_priority_fee_per_gas: Option<u128>,
    pub first_seen_ms: u64,
}

#[derive(Debug, Clone)]
pub struct LiquidityCandidate {
    pub token: Address,
    pub base: Address,
    pub router: Address,
    pub factory: Option<Address>,
    pub pair: Option<Address>,
    pub stable: Option<bool>,
    pub add_liq_tx_hash: B256,
    pub first_seen_ms: u64,
    pub implied_liquidity: U256,
}
