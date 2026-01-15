use alloy::rpc::types::TransactionRequest;

#[derive(Debug, Clone, Copy)]
pub enum GasMode {
    Eip1559,
    Legacy,
}

#[derive(Debug, Clone)]
pub struct FeeStrategy {
    pub gas_mode: GasMode,
    pub max_fee_gwei: u64,
    pub max_priority_gwei: u64,
}

impl FeeStrategy {
    pub fn apply(&self, tx: &mut TransactionRequest) {
        match self.gas_mode {
            GasMode::Eip1559 => {
                tx.max_fee_per_gas = Some(gwei_to_wei(self.max_fee_gwei));
                tx.max_priority_fee_per_gas = Some(gwei_to_wei(self.max_priority_gwei));
            }
            GasMode::Legacy => {
                tx.gas_price = Some(gwei_to_wei(self.max_fee_gwei));
            }
        }
    }
}

fn gwei_to_wei(gwei: u64) -> u128 {
    (gwei as u128) * 1_000_000_000u128
}
