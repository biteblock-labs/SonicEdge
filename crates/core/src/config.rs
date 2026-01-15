use serde::{Deserialize, Serialize};

use crate::error::Result;

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
    pub base_tokens: Vec<String>,
    #[serde(default = "default_pair_cache_capacity")]
    pub pair_cache_capacity: usize,
    #[serde(default = "default_pair_cache_ttl_ms")]
    pub pair_cache_ttl_ms: u64,
    #[serde(default = "default_pair_cache_negative_ttl_ms")]
    pub pair_cache_negative_ttl_ms: u64,
    #[serde(default)]
    pub allow_execution_without_pair: bool,
    pub min_base_amount: String,
    pub max_slippage_bps: u32,
    pub deadline_secs: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RiskConfig {
    pub sellability_amount_base: String,
    pub max_tax_bps: u32,
    pub erc20_call_timeout_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutorConfig {
    pub owner_private_key_env: String,
    pub executor_contract: String,
    pub gas_mode: String,
    pub max_fee_gwei: u64,
    pub max_priority_gwei: u64,
    pub bump_pct: u32,
    pub bump_interval_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StrategyConfig {
    pub take_profit_bps: u32,
    pub stop_loss_bps: u32,
    pub max_hold_secs: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ObservabilityConfig {
    pub metrics_enabled: bool,
    pub metrics_bind: String,
    #[serde(default = "default_log_level")]
    pub log_level: String,
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
}

fn default_pair_cache_capacity() -> usize {
    2048
}

fn default_pair_cache_ttl_ms() -> u64 {
    300_000
}

fn default_pair_cache_negative_ttl_ms() -> u64 {
    15_000
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
