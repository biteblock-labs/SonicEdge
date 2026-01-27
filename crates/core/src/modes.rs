use anyhow::anyhow;

use crate::error::Result;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LaunchGateMode {
    Strict,
    BestEffort,
}

impl LaunchGateMode {
    pub fn parse(raw: &str) -> Result<Self> {
        let normalized = raw.trim().to_ascii_lowercase();
        match normalized.as_str() {
            "strict" => Ok(Self::Strict),
            "best_effort" | "best-effort" | "besteffort" => Ok(Self::BestEffort),
            _ => Err(anyhow!("unsupported launch_only_liquidity_gate_mode: {raw}").into()),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AutoApproveMode {
    Off,
    Exact,
    Max,
}

impl AutoApproveMode {
    pub fn parse(raw: &str) -> Result<Self> {
        let normalized = raw.trim().to_ascii_lowercase();
        match normalized.as_str() {
            "off" => Ok(Self::Off),
            "exact" => Ok(Self::Exact),
            "max" => Ok(Self::Max),
            _ => Err(anyhow!("unsupported executor.auto_approve_mode: {raw}").into()),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BuyAmountMode {
    Liquidity,
    Fixed,
    WalletPct,
}

impl BuyAmountMode {
    pub fn parse(raw: &str) -> Result<Self> {
        let normalized = raw.trim().to_ascii_lowercase();
        match normalized.as_str() {
            "liquidity" => Ok(Self::Liquidity),
            "fixed" => Ok(Self::Fixed),
            "wallet_pct" | "wallet-pct" | "walletpct" | "wallet" => Ok(Self::WalletPct),
            _ => Err(anyhow!("unsupported strategy.buy_amount_mode: {raw}").into()),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SellSimulationMode {
    Strict,
    BestEffort,
}

impl SellSimulationMode {
    pub fn parse(raw: &str) -> Result<Self> {
        let normalized = raw.trim().to_ascii_lowercase();
        match normalized.as_str() {
            "strict" => Ok(Self::Strict),
            "best_effort" | "best-effort" | "besteffort" => Ok(Self::BestEffort),
            _ => Err(anyhow!("unsupported risk.sell_simulation_mode: {raw}").into()),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SellSimulationOverrideMode {
    Detect,
    SkipAny,
}

impl SellSimulationOverrideMode {
    pub fn parse(raw: &str) -> Result<Self> {
        let normalized = raw.trim().to_ascii_lowercase();
        match normalized.as_str() {
            "detect" => Ok(Self::Detect),
            "skip_any" | "skip-any" | "skipany" => Ok(Self::SkipAny),
            _ => Err(anyhow!("unsupported risk.sell_simulation_override_mode: {raw}").into()),
        }
    }
}
