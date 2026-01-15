use alloy::primitives::{Address, B256, U256};
use anyhow::anyhow;
use std::str::FromStr;
use std::time::{SystemTime, UNIX_EPOCH};

pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

pub fn parse_address(s: &str) -> anyhow::Result<Address> {
    Address::from_str(s).map_err(|e| anyhow!("invalid address {s}: {e}"))
}

pub fn parse_u256_decimal(s: &str) -> anyhow::Result<U256> {
    if let Some(stripped) = s.strip_prefix("0x") {
        Ok(U256::from_str_radix(stripped, 16)?)
    } else {
        Ok(U256::from_str_radix(s, 10)?)
    }
}

pub fn parse_b256(s: &str) -> anyhow::Result<B256> {
    let stripped = s.trim_start_matches("0x");
    B256::from_str(stripped).map_err(|e| anyhow!("invalid b256 {s}: {e}"))
}
