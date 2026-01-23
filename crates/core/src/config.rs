use alloy::primitives::Address;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

use crate::error::Result;
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
    #[serde(default = "default_sell_simulation_mode")]
    pub sell_simulation_mode: String,
    #[serde(default = "default_sell_simulation_override_mode")]
    pub sell_simulation_override_mode: String,
    #[serde(default)]
    pub token_override_slots: Vec<TokenOverrideSlotsConfig>,
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
    pub max_fee_gwei: u64,
    pub max_priority_gwei: u64,
    pub bump_pct: u32,
    pub bump_interval_ms: u64,
    #[serde(default = "default_nonce_sync_interval_ms")]
    pub nonce_sync_interval_ms: u64,
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
                bail_config!(
                    "dex.router_factories router {router:?} missing from dex.routers"
                );
            }
            if !factories.contains(&factory) {
                bail_config!(
                    "dex.router_factories factory {factory:?} missing from dex.factories"
                );
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

        let _ = parse_u256_decimal(self.risk.sellability_amount_base.trim())?;
        ensure_max_bps("risk.max_tax_bps", self.risk.max_tax_bps)?;
        ensure_valid_sell_simulation_mode(&self.risk.sell_simulation_mode)?;
        ensure_valid_sell_simulation_override_mode(&self.risk.sell_simulation_override_mode)?;
        let mut override_tokens = HashSet::new();
        for entry in &self.risk.token_override_slots {
            ensure_non_empty("risk.token_override_slots.token", &entry.token)?;
            let token = parse_address(entry.token.trim())?;
            if token == Address::ZERO {
                bail_config!("risk.token_override_slots token must not be zero");
            }
            if !override_tokens.insert(token) {
                bail_config!(
                    "risk.token_override_slots token {token:?} appears more than once"
                );
            }
        }

        ensure_non_empty("executor.owner_private_key_env", &self.executor.owner_private_key_env)?;
        let _ = parse_address(self.executor.executor_contract.trim())?;
        ensure_valid_gas_mode(&self.executor.gas_mode)?;
        ensure_valid_auto_approve_mode(&self.executor.auto_approve_mode)?;

        ensure_max_bps("strategy.take_profit_bps", self.strategy.take_profit_bps)?;
        ensure_max_bps("strategy.stop_loss_bps", self.strategy.stop_loss_bps)?;
        ensure_max_bps(
            "strategy.emergency_reserve_drop_bps",
            self.strategy.emergency_reserve_drop_bps,
        )?;
        if let Some(path) = self.strategy.position_store_path.as_deref() {
            ensure_non_empty("strategy.position_store_path", path)?;
        }

        if self.observability.metrics_enabled {
            ensure_non_empty("observability.metrics_bind", &self.observability.metrics_bind)?;
        }
        ensure_non_empty("observability.log_level", &self.observability.log_level)?;
        ensure_valid_log_format(&self.observability.log_format)?;

        ensure_non_zero_usize("mempool.fetch_concurrency", self.mempool.fetch_concurrency)?;
        ensure_non_zero_u64("mempool.tx_fetch_timeout_ms", self.mempool.tx_fetch_timeout_ms)?;
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
                } else if trimmed.len() != 64 || !trimmed.chars().all(|c| c.is_ascii_hexdigit())
                {
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
            warnings.push(
                "executor.executor_contract is zero; executions will be skipped".to_string(),
            );
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
    let normalized = value.trim().to_ascii_lowercase();
    match normalized.as_str() {
        "off" | "exact" | "max" => Ok(()),
        _ => bail_config!("unsupported executor.auto_approve_mode: {value}"),
    }
}

fn ensure_valid_sell_simulation_mode(value: &str) -> Result<()> {
    let normalized = value.trim().to_ascii_lowercase();
    match normalized.as_str() {
        "strict" | "best_effort" | "best-effort" | "besteffort" => Ok(()),
        _ => bail_config!("unsupported risk.sell_simulation_mode: {value}"),
    }
}

fn ensure_valid_sell_simulation_override_mode(value: &str) -> Result<()> {
    let normalized = value.trim().to_ascii_lowercase();
    match normalized.as_str() {
        "detect" | "skip_any" | "skip-any" | "skipany" => Ok(()),
        _ => bail_config!("unsupported risk.sell_simulation_override_mode: {value}"),
    }
}

fn ensure_valid_log_format(value: &str) -> Result<()> {
    let normalized = value.trim().to_ascii_lowercase();
    match normalized.as_str() {
        "pretty" | "json" => Ok(()),
        _ => bail_config!("unsupported observability.log_format: {value}"),
    }
}

fn default_pair_cache_capacity() -> usize {
    2048
}

fn default_auto_approve_mode() -> String {
    "off".to_string()
}

fn default_sell_simulation_mode() -> String {
    "best_effort".to_string()
}

fn default_sell_simulation_override_mode() -> String {
    "detect".to_string()
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
