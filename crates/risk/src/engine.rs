use alloy::primitives::Address;
use alloy::providers::DynProvider;
use anyhow::Result;
use sonic_core::config::RiskConfig;
use tracing::debug;

use crate::types::RiskDecision;

#[derive(Clone)]
pub struct RiskEngine {
    cfg: RiskConfig,
}

pub struct RiskContext<'a> {
    pub provider: &'a DynProvider,
    pub router: Address,
    pub base_token: Address,
    pub token: Address,
}

impl RiskEngine {
    pub fn new(cfg: RiskConfig) -> Self {
        Self { cfg }
    }

    pub async fn assess(&self, _ctx: &RiskContext<'_>) -> Result<RiskDecision> {
        debug!(
            max_tax_bps = self.cfg.max_tax_bps,
            sellability_amount_base = %self.cfg.sellability_amount_base,
            "risk assessment placeholder"
        );
        Ok(RiskDecision::pass())
    }
}
