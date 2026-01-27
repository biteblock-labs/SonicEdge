use alloy::primitives::Address;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

use crate::error::Result;
use crate::modes::{
    AutoApproveMode, BuyAmountMode, LaunchGateMode, SellSimulationMode, SellSimulationOverrideMode,
};
use crate::utils::{parse_address, parse_b256, parse_u256_decimal};

macro_rules! bail_config {
    ($($arg:tt)*) => {
        return Err(anyhow::anyhow!($($arg)*).into())
    };
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    pub chain: ChainConfig,
    pub mempool: MempoolConfig,
    pub dex: DexConfig,
    pub risk: RiskConfig,
    pub executor: ExecutorConfig,
    pub strategy: StrategyConfig,
    pub observability: ObservabilityConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChainConfig {
    pub chain_id: u64,
    pub rpc_http: String,
    pub rpc_ws: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MempoolConfig {
    pub mode: String,
    pub txpool_poll_ms: u64,
    pub fetch_concurrency: usize,
    pub tx_fetch_timeout_ms: u64,
    #[serde(default = "default_dedup_capacity")]
    pub dedup_capacity: usize,
    #[serde(default = "default_dedup_ttl_ms")]
    pub dedup_ttl_ms: u64,
    #[serde(default = "default_ws_reconnect_base_ms")]
    pub ws_reconnect_base_ms: u64,
    #[serde(default = "default_ws_reconnect_max_ms")]
    pub ws_reconnect_max_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DexConfig {
    #[serde(default)]
    pub routers: Vec<String>,
    #[serde(default)]
    pub factories: Vec<String>,
    #[serde(default)]
    pub router_factories: Vec<RouterFactoryConfig>,
    #[serde(default)]
    pub factory_pair_code_hashes: Vec<FactoryPairCodeHashConfig>,
    #[serde(default)]
    pub wrapped_native: Option<String>,
    #[serde(default)]
    pub base_tokens: Vec<String>,
    #[serde(default = "default_pair_cache_capacity")]
    pub pair_cache_capacity: usize,
    #[serde(default = "default_pair_cache_ttl_ms")]
    pub pair_cache_ttl_ms: u64,
    #[serde(default = "default_pair_cache_negative_ttl_ms")]
    pub pair_cache_negative_ttl_ms: u64,
    #[serde(default = "default_sellability_recheck_interval_ms")]
    pub sellability_recheck_interval_ms: u64,
    #[serde(default)]
    pub allow_execution_without_pair: bool,
    #[serde(default)]
    pub launch_only_liquidity_gate: bool,
    #[serde(default = "default_launch_only_liquidity_gate_mode")]
    pub launch_only_liquidity_gate_mode: String,
    pub min_base_amount: String,
    pub max_slippage_bps: u32,
    pub deadline_secs: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RiskConfig {
    pub sellability_amount_base: String,
    pub max_tax_bps: u32,
    pub erc20_call_timeout_ms: u64,
    #[serde(default)]
    pub trading_control_check: bool,
    #[serde(default)]
    pub trading_control_fail_closed: bool,
    #[serde(default)]
    pub max_tx_min_supply_bps: u32,
    #[serde(default)]
    pub max_wallet_min_supply_bps: u32,
    #[serde(default)]
    pub max_cooldown_secs: u64,
    #[serde(default = "default_sell_simulation_mode")]
    pub sell_simulation_mode: String,
    #[serde(default = "default_sell_simulation_override_mode")]
    pub sell_simulation_override_mode: String,
    #[serde(default)]
    pub token_override_slots: Vec<TokenOverrideSlotsConfig>,
    #[serde(default)]
    pub min_lp_burn_bps: u32,
    #[serde(default)]
    pub min_lp_lock_bps: u32,
    #[serde(default)]
    pub lp_lockers: Vec<String>,
    #[serde(default = "default_lp_lock_check_mode")]
    pub lp_lock_check_mode: String,
    #[serde(default = "default_lp_lock_burn_mode")]
    pub lp_lock_burn_mode: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenOverrideSlotsConfig {
    pub token: String,
    pub balance_slot: u64,
    pub allowance_slot: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutorConfig {
    pub owner_private_key_env: String,
    pub executor_contract: String,
    pub gas_mode: String,
    #[serde(default = "default_auto_approve_mode")]
    pub auto_approve_mode: String,
    #[serde(default = "default_gas_limit_buffer_bps")]
    pub gas_limit_buffer_bps: u32,
    pub max_fee_gwei: u64,
    pub max_priority_gwei: u64,
    pub bump_pct: u32,
    pub bump_interval_ms: u64,
    #[serde(default = "default_nonce_sync_interval_ms")]
    pub nonce_sync_interval_ms: u64,
    #[serde(default = "default_nonce_retry_delay_ms")]
    pub nonce_retry_delay_ms: u64,
    #[serde(default = "default_nonce_retry_max_retries")]
    pub nonce_retry_max_retries: u8,
    #[serde(default = "default_receipt_poll_interval_ms")]
    pub receipt_poll_interval_ms: u64,
    #[serde(default = "default_receipt_timeout_ms")]
    pub receipt_timeout_ms: u64,
    #[serde(default = "default_max_block_number_delta")]
    pub max_block_number_delta: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StrategyConfig {
    pub take_profit_bps: u32,
    pub stop_loss_bps: u32,
    pub max_hold_secs: u64,
    #[serde(default = "default_exit_signal_ttl_secs")]
    pub exit_signal_ttl_secs: u64,
    #[serde(default = "default_emergency_reserve_drop_bps")]
    pub emergency_reserve_drop_bps: u32,
    #[serde(default = "default_emergency_sell_sim_failures")]
    pub emergency_sell_sim_failures: u8,
    #[serde(default = "default_buy_amount_mode")]
    pub buy_amount_mode: String,
    #[serde(default = "default_buy_amount_fixed")]
    pub buy_amount_fixed: String,
    #[serde(default = "default_buy_amount_wallet_bps")]
    pub buy_amount_wallet_bps: u32,
    #[serde(default = "default_buy_amount_min")]
    pub buy_amount_min: String,
    #[serde(default = "default_buy_amount_max")]
    pub buy_amount_max: String,
    #[serde(default = "default_buy_amount_max_liquidity_bps")]
    pub buy_amount_max_liquidity_bps: u32,
    #[serde(default = "default_buy_amount_unavailable_retry_ttl_ms")]
    pub buy_amount_unavailable_retry_ttl_ms: u64,
    #[serde(default = "default_buy_amount_native_reserve")]
    pub buy_amount_native_reserve: String,
    #[serde(default = "default_position_log_interval_ms")]
    pub position_log_interval_ms: u64,
    #[serde(default)]
    pub position_store_path: Option<String>,
    #[serde(default = "default_wait_for_mine")]
    pub wait_for_mine: bool,
    #[serde(default = "default_same_block_attempt")]
    pub same_block_attempt: bool,
    #[serde(default = "default_same_block_requires_reserves")]
    pub same_block_requires_reserves: bool,
    #[serde(default = "default_wait_for_mine_poll_interval_ms")]
    pub wait_for_mine_poll_interval_ms: u64,
    #[serde(default = "default_wait_for_mine_timeout_ms")]
    pub wait_for_mine_timeout_ms: u64,
    #[serde(default = "default_candidate_cache_capacity")]
    pub candidate_cache_capacity: usize,
    #[serde(default = "default_candidate_ttl_ms")]
    pub candidate_ttl_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ObservabilityConfig {
    pub metrics_enabled: bool,
    pub metrics_bind: String,
    #[serde(default = "default_log_level")]
    pub log_level: String,
    #[serde(default = "default_log_format")]
    pub log_format: String,
    #[serde(default)]
    pub base_usd_price: Option<f64>,
    #[serde(default = "default_base_decimals")]
    pub base_decimals: u8,
}

impl AppConfig {
    pub fn load(path: &str) -> Result<Self> {
        let cfg = config::Config::builder()
            .add_source(config::File::with_name(path))
            .add_source(
                config::Environment::with_prefix("SNIPER")
                    .separator("__")
                    .try_parsing(true),
            )
            .build()?;
        Ok(cfg.try_deserialize()?)
    }

    pub fn validate(&self) -> Result<()> {
        ensure_non_empty("chain.rpc_http", &self.chain.rpc_http)?;
        ensure_non_empty("chain.rpc_ws", &self.chain.rpc_ws)?;
        ensure_non_zero_u64("chain.chain_id", self.chain.chain_id)?;

        ensure_non_empty_list("dex.routers", &self.dex.routers)?;
        ensure_non_empty_list("dex.factories", &self.dex.factories)?;
        ensure_non_empty_list("dex.base_tokens", &self.dex.base_tokens)?;

        let routers = parse_address_list("dex.routers", &self.dex.routers)?;
        let factories = parse_address_list("dex.factories", &self.dex.factories)?;
        let _ = parse_address_list("dex.base_tokens", &self.dex.base_tokens)?;

        if let Some(raw) = self.dex.wrapped_native.as_deref() {
            ensure_non_empty("dex.wrapped_native", raw)?;
            let _ = parse_address(raw)?;
        }

        for entry in &self.dex.router_factories {
            let router = parse_address(entry.router.trim())?;
            let factory = parse_address(entry.factory.trim())?;
            if !routers.contains(&router) {
                bail_config!("dex.router_factories router {router:?} missing from dex.routers");
            }
            if !factories.contains(&factory) {
                bail_config!("dex.router_factories factory {factory:?} missing from dex.factories");
            }
        }

        for entry in &self.dex.factory_pair_code_hashes {
            let factory = parse_address(entry.factory.trim())?;
            if !factories.contains(&factory) {
                bail_config!(
                    "dex.factory_pair_code_hashes factory {factory:?} missing from dex.factories"
                );
            }
            let _ = parse_b256(entry.pair_code_hash.trim())?;
        }

        let _ = parse_u256_decimal(self.dex.min_base_amount.trim())?;
        ensure_max_bps("dex.max_slippage_bps", self.dex.max_slippage_bps)?;
        ensure_non_zero_u64("dex.deadline_secs", self.dex.deadline_secs)?;
        ensure_valid_launch_gate_mode(&self.dex.launch_only_liquidity_gate_mode)?;

        let _ = parse_u256_decimal(self.risk.sellability_amount_base.trim())?;
        ensure_max_bps("risk.max_tax_bps", self.risk.max_tax_bps)?;
        ensure_max_bps(
            "risk.max_tx_min_supply_bps",
            self.risk.max_tx_min_supply_bps,
        )?;
        ensure_max_bps(
            "risk.max_wallet_min_supply_bps",
            self.risk.max_wallet_min_supply_bps,
        )?;
        ensure_valid_sell_simulation_mode(&self.risk.sell_simulation_mode)?;
        ensure_valid_sell_simulation_override_mode(&self.risk.sell_simulation_override_mode)?;
        ensure_max_bps("risk.min_lp_burn_bps", self.risk.min_lp_burn_bps)?;
        ensure_max_bps("risk.min_lp_lock_bps", self.risk.min_lp_lock_bps)?;
        ensure_valid_lp_lock_check_mode(&self.risk.lp_lock_check_mode)?;
        ensure_valid_lp_lock_burn_mode(&self.risk.lp_lock_burn_mode)?;
        let mut override_tokens = HashSet::new();
        for entry in &self.risk.token_override_slots {
            ensure_non_empty("risk.token_override_slots.token", &entry.token)?;
            let token = parse_address(entry.token.trim())?;
            if token == Address::ZERO {
                bail_config!("risk.token_override_slots token must not be zero");
            }
            if !override_tokens.insert(token) {
                bail_config!("risk.token_override_slots token {token:?} appears more than once");
            }
        }
        let mut lockers = HashSet::new();
        for entry in &self.risk.lp_lockers {
            ensure_non_empty("risk.lp_lockers", entry)?;
            let locker = parse_address(entry.trim())?;
            if locker == Address::ZERO {
                bail_config!("risk.lp_lockers must not include zero address");
            }
            if !lockers.insert(locker) {
                bail_config!("risk.lp_lockers address {locker:?} appears more than once");
            }
        }
        if self.risk.min_lp_lock_bps > 0 && self.risk.lp_lockers.is_empty() {
            bail_config!("risk.min_lp_lock_bps set but risk.lp_lockers is empty");
        }

        ensure_non_empty(
            "executor.owner_private_key_env",
            &self.executor.owner_private_key_env,
        )?;
        let _ = parse_address(self.executor.executor_contract.trim())?;
        ensure_valid_gas_mode(&self.executor.gas_mode)?;
        ensure_valid_auto_approve_mode(&self.executor.auto_approve_mode)?;
        ensure_max_bps(
            "executor.gas_limit_buffer_bps",
            self.executor.gas_limit_buffer_bps,
        )?;

        ensure_max_bps("strategy.take_profit_bps", self.strategy.take_profit_bps)?;
        ensure_max_bps("strategy.stop_loss_bps", self.strategy.stop_loss_bps)?;
        ensure_max_bps(
            "strategy.emergency_reserve_drop_bps",
            self.strategy.emergency_reserve_drop_bps,
        )?;
        let buy_amount_fixed = parse_u256_decimal(self.strategy.buy_amount_fixed.trim())?;
        let buy_amount_min = parse_u256_decimal(self.strategy.buy_amount_min.trim())?;
        let buy_amount_max = parse_u256_decimal(self.strategy.buy_amount_max.trim())?;
        let _buy_amount_native_reserve =
            parse_u256_decimal(self.strategy.buy_amount_native_reserve.trim())?;
        ensure_max_bps(
            "strategy.buy_amount_wallet_bps",
            self.strategy.buy_amount_wallet_bps,
        )?;
        ensure_max_bps(
            "strategy.buy_amount_max_liquidity_bps",
            self.strategy.buy_amount_max_liquidity_bps,
        )?;
        if !buy_amount_min.is_zero() && !buy_amount_max.is_zero() && buy_amount_min > buy_amount_max
        {
            bail_config!("strategy.buy_amount_min must be <= strategy.buy_amount_max");
        }
        match BuyAmountMode::parse(&self.strategy.buy_amount_mode)? {
            BuyAmountMode::Liquidity => {}
            BuyAmountMode::Fixed => {
                if buy_amount_fixed.is_zero() {
                    bail_config!(
                        "strategy.buy_amount_fixed must be > 0 when strategy.buy_amount_mode=fixed"
                    );
                }
            }
            BuyAmountMode::WalletPct => {
                if self.strategy.buy_amount_wallet_bps == 0 {
                    bail_config!(
                        "strategy.buy_amount_wallet_bps must be > 0 when strategy.buy_amount_mode=wallet_pct"
                    );
                }
            }
        }
        if let Some(path) = self.strategy.position_store_path.as_deref() {
            ensure_non_empty("strategy.position_store_path", path)?;
        }

        if self.observability.metrics_enabled {
            ensure_non_empty(
                "observability.metrics_bind",
                &self.observability.metrics_bind,
            )?;
        }
        ensure_non_empty("observability.log_level", &self.observability.log_level)?;
        ensure_valid_log_format(&self.observability.log_format)?;
        if let Some(price) = self.observability.base_usd_price {
            if price.is_sign_negative() {
                bail_config!("observability.base_usd_price must be >= 0");
            }
        }
        if self.observability.base_decimals == 0 || self.observability.base_decimals > 36 {
            bail_config!("observability.base_decimals must be between 1 and 36");
        }

        ensure_non_zero_usize("mempool.fetch_concurrency", self.mempool.fetch_concurrency)?;
        ensure_non_zero_u64(
            "mempool.tx_fetch_timeout_ms",
            self.mempool.tx_fetch_timeout_ms,
        )?;
        if self.mempool.mode.contains("txpool") {
            ensure_non_zero_u64("mempool.txpool_poll_ms", self.mempool.txpool_poll_ms)?;
        }

        Ok(())
    }

    pub fn validate_for_run(&self) -> Result<Vec<String>> {
        self.validate()?;
        let mut warnings = Vec::new();
        let env_key = self.executor.owner_private_key_env.trim();
        match std::env::var(env_key) {
            Ok(raw) => {
                let trimmed = raw.trim().trim_start_matches("0x");
                if trimmed.is_empty() {
                    warnings.push(format!(
                        "private key env var {env_key} is empty; executions will be skipped"
                    ));
                } else if trimmed.len() != 64 || !trimmed.chars().all(|c| c.is_ascii_hexdigit()) {
                    warnings.push(format!(
                        "private key env var {env_key} is not a 32-byte hex string; executions will be skipped"
                    ));
                }
            }
            Err(_) => warnings.push(format!(
                "private key env var {env_key} is not set; executions will be skipped"
            )),
        }
        let contract = parse_address(self.executor.executor_contract.trim())?;
        if contract == Address::ZERO {
            warnings
                .push("executor.executor_contract is zero; executions will be skipped".to_string());
        }
        Ok(warnings)
    }
}

fn ensure_non_empty(field: &str, value: &str) -> Result<()> {
    if value.trim().is_empty() {
        bail_config!("{field} must not be empty");
    }
    Ok(())
}

fn ensure_non_empty_list(field: &str, value: &[String]) -> Result<()> {
    if value.is_empty() {
        bail_config!("{field} must not be empty");
    }
    Ok(())
}

fn ensure_non_zero_u64(field: &str, value: u64) -> Result<()> {
    if value == 0 {
        bail_config!("{field} must be greater than zero");
    }
    Ok(())
}

fn ensure_non_zero_usize(field: &str, value: usize) -> Result<()> {
    if value == 0 {
        bail_config!("{field} must be greater than zero");
    }
    Ok(())
}

fn ensure_max_bps(field: &str, value: u32) -> Result<()> {
    if value > 10_000 {
        bail_config!("{field} must be <= 10000 bps");
    }
    Ok(())
}

fn parse_address_list(field: &str, value: &[String]) -> Result<HashSet<Address>> {
    let mut parsed = HashSet::with_capacity(value.len());
    for entry in value {
        ensure_non_empty(field, entry)?;
        parsed.insert(parse_address(entry.trim())?);
    }
    Ok(parsed)
}

fn ensure_valid_gas_mode(value: &str) -> Result<()> {
    let normalized = value.trim().to_ascii_lowercase();
    match normalized.as_str() {
        "legacy" | "eip1559" => Ok(()),
        _ => bail_config!("unsupported executor.gas_mode: {value}"),
    }
}

fn ensure_valid_auto_approve_mode(value: &str) -> Result<()> {
    AutoApproveMode::parse(value)?;
    Ok(())
}

fn ensure_valid_sell_simulation_mode(value: &str) -> Result<()> {
    SellSimulationMode::parse(value)?;
    Ok(())
}

fn ensure_valid_sell_simulation_override_mode(value: &str) -> Result<()> {
    SellSimulationOverrideMode::parse(value)?;
    Ok(())
}

fn ensure_valid_launch_gate_mode(value: &str) -> Result<()> {
    LaunchGateMode::parse(value)?;
    Ok(())
}

fn ensure_valid_log_format(value: &str) -> Result<()> {
    let normalized = value.trim().to_ascii_lowercase();
    match normalized.as_str() {
        "pretty" | "json" => Ok(()),
        _ => bail_config!("unsupported observability.log_format: {value}"),
    }
}

fn ensure_valid_lp_lock_check_mode(value: &str) -> Result<()> {
    let normalized = value.trim().to_ascii_lowercase();
    match normalized.as_str() {
        "strict" | "best_effort" => Ok(()),
        _ => bail_config!("unsupported risk.lp_lock_check_mode: {value}"),
    }
}

fn ensure_valid_lp_lock_burn_mode(value: &str) -> Result<()> {
    let normalized = value.trim().to_ascii_lowercase();
    match normalized.as_str() {
        "any" | "all" => Ok(()),
        _ => bail_config!("unsupported risk.lp_lock_burn_mode: {value}"),
    }
}

fn default_pair_cache_capacity() -> usize {
    2048
}

fn default_auto_approve_mode() -> String {
    "off".to_string()
}

fn default_gas_limit_buffer_bps() -> u32 {
    2000
}

fn default_sell_simulation_mode() -> String {
    "best_effort".to_string()
}

fn default_sell_simulation_override_mode() -> String {
    "detect".to_string()
}

fn default_lp_lock_check_mode() -> String {
    "strict".to_string()
}

fn default_lp_lock_burn_mode() -> String {
    "any".to_string()
}

fn default_launch_only_liquidity_gate_mode() -> String {
    "strict".to_string()
}

fn default_pair_cache_ttl_ms() -> u64 {
    300_000
}

fn default_pair_cache_negative_ttl_ms() -> u64 {
    15_000
}

fn default_sellability_recheck_interval_ms() -> u64 {
    0
}

fn default_base_decimals() -> u8 {
    18
}

fn default_dedup_capacity() -> usize {
    100_000
}

fn default_dedup_ttl_ms() -> u64 {
    60_000
}

fn default_ws_reconnect_base_ms() -> u64 {
    500
}

fn default_ws_reconnect_max_ms() -> u64 {
    30_000
}

fn default_log_level() -> String {
    "info".to_string()
}

fn default_log_format() -> String {
    "pretty".to_string()
}

fn default_candidate_cache_capacity() -> usize {
    10_000
}

fn default_candidate_ttl_ms() -> u64 {
    300_000
}

fn default_buy_amount_mode() -> String {
    "liquidity".to_string()
}

fn default_buy_amount_fixed() -> String {
    "0".to_string()
}

fn default_buy_amount_wallet_bps() -> u32 {
    0
}

fn default_buy_amount_min() -> String {
    "0".to_string()
}

fn default_buy_amount_max() -> String {
    "0".to_string()
}

fn default_buy_amount_max_liquidity_bps() -> u32 {
    0
}

fn default_buy_amount_unavailable_retry_ttl_ms() -> u64 {
    0
}

fn default_buy_amount_native_reserve() -> String {
    "0".to_string()
}

fn default_position_log_interval_ms() -> u64 {
    30_000
}

fn default_wait_for_mine() -> bool {
    true
}

fn default_exit_signal_ttl_secs() -> u64 {
    3600
}

fn default_emergency_reserve_drop_bps() -> u32 {
    0
}

fn default_emergency_sell_sim_failures() -> u8 {
    0
}

fn default_same_block_attempt() -> bool {
    true
}

fn default_same_block_requires_reserves() -> bool {
    true
}

fn default_wait_for_mine_poll_interval_ms() -> u64 {
    500
}

fn default_wait_for_mine_timeout_ms() -> u64 {
    60_000
}

fn default_nonce_sync_interval_ms() -> u64 {
    10_000
}

fn default_nonce_retry_delay_ms() -> u64 {
    200
}

fn default_nonce_retry_max_retries() -> u8 {
    1
}

fn default_receipt_poll_interval_ms() -> u64 {
    2_000
}

fn default_receipt_timeout_ms() -> u64 {
    120_000
}

fn default_max_block_number_delta() -> u64 {
    1
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RouterFactoryConfig {
    pub router: String,
    pub factory: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FactoryPairCodeHashConfig {
    pub factory: String,
    pub pair_code_hash: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base_config() -> AppConfig {
        AppConfig {
            chain: ChainConfig {
                chain_id: 1,
                rpc_http: "http://localhost:8545".to_string(),
                rpc_ws: "ws://localhost:8546".to_string(),
            },
            mempool: MempoolConfig {
                mode: "ws+txpool".to_string(),
                txpool_poll_ms: 1,
                fetch_concurrency: 1,
                tx_fetch_timeout_ms: 1,
                dedup_capacity: 1,
                dedup_ttl_ms: 1,
                ws_reconnect_base_ms: 1,
                ws_reconnect_max_ms: 1,
            },
            dex: DexConfig {
                routers: vec!["0x0000000000000000000000000000000000000001".to_string()],
                factories: vec!["0x0000000000000000000000000000000000000002".to_string()],
                router_factories: Vec::new(),
                factory_pair_code_hashes: Vec::new(),
                wrapped_native: None,
                base_tokens: vec!["0x0000000000000000000000000000000000000003".to_string()],
                pair_cache_capacity: 1,
                pair_cache_ttl_ms: 1,
                pair_cache_negative_ttl_ms: 1,
                sellability_recheck_interval_ms: 1,
                allow_execution_without_pair: false,
                launch_only_liquidity_gate: true,
                launch_only_liquidity_gate_mode: "strict".to_string(),
                min_base_amount: "1".to_string(),
                max_slippage_bps: 100,
                deadline_secs: 1,
            },
            risk: RiskConfig {
                sellability_amount_base: "1".to_string(),
                max_tax_bps: 0,
                erc20_call_timeout_ms: 1,
                trading_control_check: false,
                trading_control_fail_closed: false,
                max_tx_min_supply_bps: 0,
                max_wallet_min_supply_bps: 0,
                max_cooldown_secs: 0,
                sell_simulation_mode: "best_effort".to_string(),
                sell_simulation_override_mode: "detect".to_string(),
                token_override_slots: Vec::new(),
                min_lp_burn_bps: 0,
                min_lp_lock_bps: 0,
                lp_lockers: Vec::new(),
                lp_lock_check_mode: "strict".to_string(),
                lp_lock_burn_mode: "any".to_string(),
            },
            executor: ExecutorConfig {
                owner_private_key_env: "SNIPER_PK".to_string(),
                executor_contract: "0x0000000000000000000000000000000000000004".to_string(),
                gas_mode: "eip1559".to_string(),
                auto_approve_mode: "off".to_string(),
                gas_limit_buffer_bps: 0,
                max_fee_gwei: 1,
                max_priority_gwei: 1,
                bump_pct: 0,
                bump_interval_ms: 1,
                nonce_sync_interval_ms: 1,
                nonce_retry_delay_ms: 1,
                nonce_retry_max_retries: 1,
                receipt_poll_interval_ms: 1,
                receipt_timeout_ms: 1,
                max_block_number_delta: 1,
            },
            strategy: StrategyConfig {
                take_profit_bps: 0,
                stop_loss_bps: 0,
                max_hold_secs: 1,
                exit_signal_ttl_secs: 1,
                emergency_reserve_drop_bps: 0,
                emergency_sell_sim_failures: 0,
                buy_amount_mode: "fixed".to_string(),
                buy_amount_fixed: "1".to_string(),
                buy_amount_wallet_bps: 0,
                buy_amount_min: "0".to_string(),
                buy_amount_max: "0".to_string(),
                buy_amount_max_liquidity_bps: 0,
                buy_amount_unavailable_retry_ttl_ms: 1,
                buy_amount_native_reserve: "0".to_string(),
                position_log_interval_ms: 1,
                position_store_path: None,
                wait_for_mine: true,
                same_block_attempt: false,
                same_block_requires_reserves: true,
                wait_for_mine_poll_interval_ms: 1,
                wait_for_mine_timeout_ms: 1,
                candidate_cache_capacity: 1,
                candidate_ttl_ms: 1,
            },
            observability: ObservabilityConfig {
                metrics_enabled: false,
                metrics_bind: "127.0.0.1:0".to_string(),
                log_level: "info".to_string(),
                log_format: "pretty".to_string(),
                base_usd_price: None,
                base_decimals: 18,
            },
        }
    }

    #[test]
    fn validate_rejects_lp_lock_bps_without_lockers() {
        let mut cfg = base_config();
        cfg.risk.min_lp_lock_bps = 5000;
        let err = cfg.validate().unwrap_err().to_string();
        assert!(err.contains("risk.min_lp_lock_bps set but risk.lp_lockers is empty"));
    }

    #[test]
    fn validate_rejects_invalid_lp_modes() {
        let mut cfg = base_config();
        cfg.risk.lp_lock_check_mode = "unknown".to_string();
        let err = cfg.validate().unwrap_err().to_string();
        assert!(err.contains("risk.lp_lock_check_mode"));

        let mut cfg = base_config();
        cfg.risk.lp_lock_burn_mode = "unknown".to_string();
        let err = cfg.validate().unwrap_err().to_string();
        assert!(err.contains("risk.lp_lock_burn_mode"));
    }
}
