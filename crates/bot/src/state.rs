use alloy::primitives::{B256, U256};

#[derive(Debug, Clone, Copy)]
pub enum BotState {
    Detecting,
    Qualifying,
    Executing,
    Managing,
}

#[derive(Debug, Clone)]
pub struct FillRecord {
    pub tx_hash: B256,
    pub block_number: u64,
    pub amount_out: U256,
}
