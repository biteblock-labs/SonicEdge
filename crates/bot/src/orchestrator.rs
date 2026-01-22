use crate::abi::IERC20;
use crate::metrics::{spawn_metrics_server, BotMetrics};
use crate::state::{
    BotState,
    CandidateStore,
    DropKind,
    ExitReason,
    Position,
    PositionStatus,
    PositionStore,
};
use alloy::eips::BlockId;
use alloy::primitives::{address, keccak256, Address, B256, TxKind, U256};
use alloy::rpc::types::state::{AccountOverride, StateOverride, StateOverridesBuilder};
use alloy::rpc::types::transaction::TransactionInput;
use alloy::rpc::types::TransactionRequest;
use alloy::providers::{DynProvider, Provider};
use alloy::sol_types::SolCall;
use anyhow::{bail, Result};
use sonic_chain::{
    NewHeadStream,
    NodeClient,
    PendingTxStream,
    ReconnectConfig,
    TxFetcher,
    TxpoolBackfill,
};
use sonic_core::config::AppConfig;
use sonic_core::dedupe::DedupeCache;
use sonic_core::types::{LiquidityCandidate, MempoolTx};
use sonic_core::utils::{now_ms, parse_address, parse_b256, parse_u256_decimal};
use sonic_dex::abi::{IUniswapV2Router02, ISolidlyRouter, Route};
use sonic_dex::pair::{
    contract_exists_at_block,
    get_pair_tokens,
    get_reserves,
    get_reserves_at_block,
    get_pair_address,
    get_pair_address_solidly,
};
use sonic_dex::{decode_router_calldata, PairMetadataCache, RouterCall};
use sonic_executor::fees::{FeeStrategy, GasMode};
use sonic_executor::{nonce::NonceManager, sender::TxSender};
use sonic_executor::{
    BuySolidlyEthParams,
    BuySolidlyParams,
    BuyV2EthParams,
    BuyV2Params,
    ExecutorTxBuilder,
    SellSolidlyEthParams,
    SellSolidlyParams,
    SellV2EthParams,
    SellV2Params,
};
use sonic_risk::{RiskContext, RiskEngine};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::Arc;
use tokio::select;
use tokio::time::Duration;
use tracing::{debug, info, warn};

const SUMMARY_INTERVAL_MS: u64 = 30_000;
const HEAD_FRESHNESS_MS: u64 = 10_000;
const MAX_EXECUTION_ATTEMPTS: u8 = 2;
const EXIT_POLL_INTERVAL_MS: u64 = 1_000;
const BPS_DENOMINATOR: u64 = 10_000;
const PRICE_SCALE: u128 = 1_000_000_000_000_000_000u128;
const ERC20_BALANCES_SLOT: u64 = 0;
const ERC20_ALLOWANCES_SLOT: u64 = 1;
const SIMULATION_DEADLINE_SECS: u64 = 10_000_000_000;
const SIMULATION_SENDER: Address = address!("0x1111111111111111111111111111111111111111");
const VERIFY_AMOUNT_IN: u64 = 1_000_000_000_000_000_000;

enum ExecutionOutcome {
    Sent { hash: B256, tx: TransactionRequest },
    Skipped(&'static str, DropKind),
}

enum LaunchGateDecision {
    Allow,
    Reject {
        reason: String,
        kind: DropKind,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LaunchGateMode {
    Strict,
    BestEffort,
}

impl LaunchGateMode {
    fn parse(raw: &str) -> Result<Self> {
        let normalized = raw.trim().to_ascii_lowercase();
        match normalized.as_str() {
            "strict" => Ok(Self::Strict),
            "best_effort" | "best-effort" | "besteffort" => Ok(Self::BestEffort),
            _ => bail!("unsupported launch_only_liquidity_gate_mode: {raw}"),
        }
    }
}

pub struct Bot {
    cfg: AppConfig,
    chain: NodeClient,
    routers: HashSet<Address>,
    router_factories: HashMap<Address, Address>,
    router_meta: HashMap<Address, RouterMeta>,
    factories: Vec<Address>,
    base_tokens: HashSet<Address>,
    base_token_list: Vec<Address>,
    wrapped_native: Option<Address>,
    risk: RiskEngine,
    pair_cache: PairMetadataCache,
    dedupe: DedupeCache<B256>,
    metrics: Option<Arc<BotMetrics>>,
    tx_builder: ExecutorTxBuilder,
    nonce: NonceManager,
    sender: TxSender,
    min_base_amount: U256,
    launch_gate_mode: LaunchGateMode,
    pending_liquidity: HashMap<B256, PendingLiquidity>,
    pending_receipts: HashMap<B256, PendingReceipt>,
    pending_exits: HashMap<B256, PendingExit>,
    sell_sim_failures: HashMap<B256, u8>,
    latest_head: Option<u64>,
    latest_head_seen_ms: Option<u64>,
    counters: CounterSummary,
    candidate_store: CandidateStore,
    positions: PositionStore,
    position_store_path: Option<PathBuf>,
}

#[derive(Default, Clone, Copy)]
struct Counters {
    hashes_seen: u64,
    dedupe_dropped: u64,
    tx_missing: u64,
    tx_fetched: u64,
    router_hits: u64,
    decoded: u64,
    candidates: u64,
    filtered_base_token: u64,
    filtered_min_amount: u64,
    pair_resolved: u64,
    pair_unresolved: u64,
    risk_pass: u64,
    risk_fail: u64,
    executed: u64,
}

impl Counters {
    fn delta(&self, previous: &Counters) -> Counters {
        Counters {
            hashes_seen: self.hashes_seen.saturating_sub(previous.hashes_seen),
            dedupe_dropped: self.dedupe_dropped.saturating_sub(previous.dedupe_dropped),
            tx_missing: self.tx_missing.saturating_sub(previous.tx_missing),
            tx_fetched: self.tx_fetched.saturating_sub(previous.tx_fetched),
            router_hits: self.router_hits.saturating_sub(previous.router_hits),
            decoded: self.decoded.saturating_sub(previous.decoded),
            candidates: self.candidates.saturating_sub(previous.candidates),
            filtered_base_token: self.filtered_base_token.saturating_sub(previous.filtered_base_token),
            filtered_min_amount: self.filtered_min_amount.saturating_sub(previous.filtered_min_amount),
            pair_resolved: self.pair_resolved.saturating_sub(previous.pair_resolved),
            pair_unresolved: self.pair_unresolved.saturating_sub(previous.pair_unresolved),
            risk_pass: self.risk_pass.saturating_sub(previous.risk_pass),
            risk_fail: self.risk_fail.saturating_sub(previous.risk_fail),
            executed: self.executed.saturating_sub(previous.executed),
        }
    }
}

struct CounterSummary {
    totals: Counters,
    last: Counters,
    last_log_ms: u64,
}

#[derive(Clone, Debug)]
struct PendingReceipt {
    candidate_hash: B256,
    sent_at_ms: u64,
    last_sent_ms: u64,
    tx: TransactionRequest,
}

#[derive(Clone, Debug)]
struct PendingExit {
    position_hash: B256,
    sent_at_ms: u64,
    last_sent_ms: u64,
    tx: Option<TransactionRequest>,
}

#[derive(Clone, Debug)]
struct PendingLiquidity {
    candidate: LiquidityCandidate,
    enqueued_ms: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RouterKind {
    V2,
    Solidly,
    Unknown,
}

impl RouterKind {
    fn as_str(&self) -> &'static str {
        match self {
            RouterKind::V2 => "v2",
            RouterKind::Solidly => "solidly",
            RouterKind::Unknown => "unknown",
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct RouterMeta {
    kind: RouterKind,
    sellability_enabled: bool,
}

impl CounterSummary {
    fn new(now_ms: u64) -> Self {
        Self {
            totals: Counters::default(),
            last: Counters::default(),
            last_log_ms: now_ms,
        }
    }

    fn maybe_log(&mut self, now_ms: u64) {
        if now_ms.saturating_sub(self.last_log_ms) < SUMMARY_INTERVAL_MS {
            return;
        }
        let delta = self.totals.delta(&self.last);
        self.last = self.totals;
        self.last_log_ms = now_ms;
        info!(
            hashes = delta.hashes_seen,
            dedupe_dropped = delta.dedupe_dropped,
            tx_missing = delta.tx_missing,
            tx_fetched = delta.tx_fetched,
            router_hits = delta.router_hits,
            decoded = delta.decoded,
            candidates = delta.candidates,
            filtered_base_token = delta.filtered_base_token,
            filtered_min_amount = delta.filtered_min_amount,
            pair_resolved = delta.pair_resolved,
            pair_unresolved = delta.pair_unresolved,
            risk_pass = delta.risk_pass,
            risk_fail = delta.risk_fail,
            executed = delta.executed,
            "counter summary (last 30s)"
        );
    }
}

impl Bot {
    pub async fn new(cfg: AppConfig) -> Result<Self> {
        let chain = NodeClient::connect(&cfg.chain).await?;
        let routers = cfg
            .dex
            .routers
            .iter()
            .map(|s| parse_address(s))
            .collect::<Result<HashSet<_>>>()?;
        let factories = cfg
            .dex
            .factories
            .iter()
            .map(|s| parse_address(s))
            .collect::<Result<Vec<_>>>()?;
        let mut router_factories = HashMap::new();
        for entry in &cfg.dex.router_factories {
            let router = parse_address(&entry.router)?;
            let factory = parse_address(&entry.factory)?;
            if router_factories.insert(router, factory).is_some() {
                warn!(router = %router, "router factory mapping overwritten");
            }
        }
        let mut pair_code_hashes = HashMap::new();
        for entry in &cfg.dex.factory_pair_code_hashes {
            let factory = parse_address(&entry.factory)?;
            let code_hash = parse_b256(&entry.pair_code_hash)?;
            if pair_code_hashes.insert(factory, code_hash).is_some() {
                warn!(factory = %factory, "pair code hash mapping overwritten");
            }
        }
        let base_token_list = cfg
            .dex
            .base_tokens
            .iter()
            .map(|s| parse_address(s))
            .collect::<Result<Vec<_>>>()?;
        let base_tokens = base_token_list.iter().copied().collect::<HashSet<_>>();
        let wrapped_native = match cfg.dex.wrapped_native.as_deref() {
            Some(raw) => {
                let parsed = parse_address(raw)?;
                if parsed == Address::ZERO {
                    warn!("wrapped_native is zero; native execution disabled");
                    None
                } else {
                    Some(parsed)
                }
            }
            None => None,
        };

        let owner = match std::env::var(&cfg.executor.owner_private_key_env) {
            Ok(pk) => {
                let key = B256::from_str(pk.trim_start_matches("0x"))?;
                let signer = alloy::signers::local::PrivateKeySigner::from_bytes(&key)?;
                signer.address()
            }
            Err(_) => Address::ZERO,
        };

        let contract = parse_address(&cfg.executor.executor_contract)?;
        let gas_mode = match cfg.executor.gas_mode.as_str() {
            "legacy" => GasMode::Legacy,
            _ => GasMode::Eip1559,
        };
        let fees = FeeStrategy {
            gas_mode,
            max_fee_gwei: cfg.executor.max_fee_gwei,
            max_priority_gwei: cfg.executor.max_priority_gwei,
        };
        let tx_builder = ExecutorTxBuilder::new(contract, owner, cfg.chain.chain_id, fees);
        let nonce = NonceManager::new(0);
        let sender = TxSender::new(chain.http.clone());
        let min_base_amount = parse_u256_decimal(&cfg.dex.min_base_amount)?;
        let launch_gate_mode =
            LaunchGateMode::parse(&cfg.dex.launch_only_liquidity_gate_mode)?;
        let risk = RiskEngine::new(cfg.risk.clone())?;
        let pair_cache = PairMetadataCache::new(
            cfg.dex.pair_cache_capacity,
            cfg.dex.pair_cache_ttl_ms,
            cfg.dex.pair_cache_negative_ttl_ms,
            pair_code_hashes,
        );
        let router_meta = Self::verify_router_support(
            &chain.http,
            &routers,
            &router_factories,
            &base_token_list,
            wrapped_native,
        )
        .await;
        let dedupe = DedupeCache::new(cfg.mempool.dedup_capacity, cfg.mempool.dedup_ttl_ms);
        let metrics = if cfg.observability.metrics_enabled {
            let metrics = Arc::new(BotMetrics::new()?);
            if let Err(err) = spawn_metrics_server(&cfg.observability.metrics_bind, metrics.clone())
            {
                warn!(?err, "metrics server failed to start");
            }
            Some(metrics)
        } else {
            None
        };
        Self::report_router_meta(&router_meta, metrics.as_ref().map(Arc::as_ref));
        let counters = CounterSummary::new(now_ms());
        let candidate_store = CandidateStore::new(
            cfg.strategy.candidate_cache_capacity,
            cfg.strategy.candidate_ttl_ms,
        );
        let position_store_path = cfg
            .strategy
            .position_store_path
            .as_deref()
            .map(|path| path.trim())
            .filter(|path| !path.is_empty())
            .map(PathBuf::from);
        let (positions, pending_exits) = match &position_store_path {
            Some(path) => {
                let positions = match PositionStore::load_from(path) {
                    Ok(store) => store,
                    Err(err) => {
                        if let Some(io_err) = err.downcast_ref::<std::io::Error>() {
                            if io_err.kind() == std::io::ErrorKind::NotFound {
                                info!(
                                    path = %path.display(),
                                    "position store not found; starting fresh"
                                );
                                PositionStore::new()
                            } else {
                                warn!(
                                    path = %path.display(),
                                    ?err,
                                    "position store load failed; starting fresh"
                                );
                                PositionStore::new()
                            }
                        } else {
                            warn!(
                                path = %path.display(),
                                ?err,
                                "position store load failed; starting fresh"
                            );
                            PositionStore::new()
                        }
                    }
                };
                let pending_exits = Self::rehydrate_pending_exits(&positions);
                if !positions.is_empty() {
                    info!(
                        path = %path.display(),
                        count = positions.len(),
                        "loaded positions from disk"
                    );
                }
                if !pending_exits.is_empty() {
                    info!(
                        count = pending_exits.len(),
                        "rehydrated pending exit receipts"
                    );
                }
                (positions, pending_exits)
            }
            None => (PositionStore::new(), HashMap::new()),
        };

        Ok(Self {
            cfg,
            chain,
            routers,
            router_factories,
            router_meta,
            factories,
            base_tokens,
            base_token_list,
            wrapped_native,
            risk,
            pair_cache,
            dedupe,
            metrics,
            tx_builder,
            nonce,
            sender,
            min_base_amount,
            launch_gate_mode,
            pending_liquidity: HashMap::new(),
            pending_receipts: HashMap::new(),
            pending_exits,
            sell_sim_failures: HashMap::new(),
            latest_head: None,
            latest_head_seen_ms: None,
            counters,
            candidate_store,
            positions,
            position_store_path,
        })
    }

    fn rehydrate_pending_exits(positions: &PositionStore) -> HashMap<B256, PendingExit> {
        let mut pending = HashMap::new();
        for (position_hash, position) in positions.snapshot() {
            let Some(exit_tx_hash) = position.exit_tx_hash else {
                continue;
            };
            let sent_at_ms = position.last_update_ms.max(position.opened_ms);
            if pending
                .insert(
                    exit_tx_hash,
                    PendingExit {
                        position_hash,
                        sent_at_ms,
                        last_sent_ms: sent_at_ms,
                        tx: None,
                    },
                )
                .is_some()
            {
                warn!(
                    exit_tx_hash = %exit_tx_hash,
                    position_hash = %position_hash,
                    "duplicate exit tx hash recovered from position store"
                );
            }
        }
        pending
    }

    fn persist_positions(&self) {
        let Some(path) = &self.position_store_path else {
            return;
        };
        if let Err(err) = self.positions.persist_to(path) {
            warn!(path = %path.display(), ?err, "position store persist failed");
        }
    }

    async fn verify_router_support(
        provider: &DynProvider,
        routers: &HashSet<Address>,
        router_factories: &HashMap<Address, Address>,
        base_tokens: &[Address],
        wrapped_native: Option<Address>,
    ) -> HashMap<Address, RouterMeta> {
        let mut tokens: Vec<Address> = base_tokens
            .iter()
            .copied()
            .filter(|addr| *addr != Address::ZERO)
            .collect();
        if tokens.len() < 2 {
            if let Some(wrapped_native) = wrapped_native {
                if wrapped_native != Address::ZERO && !tokens.contains(&wrapped_native) {
                    tokens.push(wrapped_native);
                }
            }
        }
        let mut meta = HashMap::new();

        if tokens.len() < 2 {
            warn!(
                "router verification needs at least two non-zero base tokens; sellability disabled"
            );
            for &router in routers {
                meta.insert(
                    router,
                    RouterMeta {
                        kind: RouterKind::Unknown,
                        sellability_enabled: false,
                    },
                );
            }
            return meta;
        }

        for &router in routers {
            let Some(&factory) = router_factories.get(&router) else {
                warn!(router = %router, "router missing factory mapping; sellability disabled");
                meta.insert(
                    router,
                    RouterMeta {
                        kind: RouterKind::Unknown,
                        sellability_enabled: false,
                    },
                );
                continue;
            };

            let kind = Self::detect_factory_kind(provider, factory, &tokens).await;
            let mut sellability_enabled = false;
            match kind {
                RouterKind::V2 => {
                    if let Some((token_a, token_b)) =
                        Self::find_v2_pair(provider, factory, &tokens).await
                    {
                        sellability_enabled =
                            Self::verify_v2_router(provider, router, token_a, token_b).await;
                        if !sellability_enabled {
                            warn!(
                                router = %router,
                                factory = %factory,
                                "v2 router quote failed; sellability disabled"
                            );
                        }
                    } else {
                        warn!(
                            router = %router,
                            factory = %factory,
                            "no v2 pair found for verification; sellability disabled"
                        );
                    }
                }
                RouterKind::Solidly => {
                    if let Some((token_a, token_b, stable)) =
                        Self::find_solidly_pair(provider, factory, &tokens).await
                    {
                        sellability_enabled = Self::verify_solidly_router(
                            provider, router, token_a, token_b, stable,
                        )
                        .await;
                        if !sellability_enabled {
                            warn!(
                                router = %router,
                                factory = %factory,
                                "solidly router quote failed; sellability disabled"
                            );
                        }
                    } else {
                        warn!(
                            router = %router,
                            factory = %factory,
                            "no solidly pair found for verification; sellability disabled"
                        );
                    }
                }
                RouterKind::Unknown => {
                    warn!(
                        router = %router,
                        factory = %factory,
                        "factory type unknown; sellability disabled"
                    );
                }
            }

            meta.insert(
                router,
                RouterMeta {
                    kind,
                    sellability_enabled,
                },
            );
        }

        meta
    }

    fn report_router_meta(
        router_meta: &HashMap<Address, RouterMeta>,
        metrics: Option<&BotMetrics>,
    ) {
        let mut total = 0u64;
        let mut enabled = 0u64;
        let mut v2 = 0u64;
        let mut solidly = 0u64;
        let mut unknown = 0u64;
        for (router, meta) in router_meta {
            total = total.saturating_add(1);
            if meta.sellability_enabled {
                enabled = enabled.saturating_add(1);
            }
            match meta.kind {
                RouterKind::V2 => v2 = v2.saturating_add(1),
                RouterKind::Solidly => solidly = solidly.saturating_add(1),
                RouterKind::Unknown => unknown = unknown.saturating_add(1),
            }
            info!(
                router = %router,
                kind = meta.kind.as_str(),
                sellability_enabled = meta.sellability_enabled,
                "router verification result"
            );
            if let Some(metrics) = metrics {
                let value = if meta.sellability_enabled { 1 } else { 0 };
                metrics
                    .router_sellability
                    .with_label_values(&[
                        &router.to_string(),
                        meta.kind.as_str(),
                    ])
                    .set(value);
            }
        }
        if total > 0 {
            info!(
                total,
                enabled,
                disabled = total.saturating_sub(enabled),
                v2,
                solidly,
                unknown,
                "router verification summary"
            );
            if enabled == 0 {
                warn!("all routers disabled for sellability checks");
            }
        }
    }

    fn router_verification_tokens(&self) -> Vec<Address> {
        let mut tokens = Vec::new();
        for &token in &self.base_token_list {
            if token == Address::ZERO {
                continue;
            }
            if !tokens.contains(&token) {
                tokens.push(token);
            }
        }
        if tokens.len() < 2 {
            if let Some(wrapped_native) = self.wrapped_native {
                if wrapped_native != Address::ZERO && !tokens.contains(&wrapped_native) {
                    tokens.push(wrapped_native);
                }
            }
        }
        tokens
    }

    fn clear_router_sellability_metric(&self, router: Address, kind: RouterKind) {
        if let Some(metrics) = &self.metrics {
            metrics
                .router_sellability
                .with_label_values(&[&router.to_string(), kind.as_str()])
                .set(0);
        }
    }

    fn set_router_sellability_metric(&self, router: Address, kind: RouterKind, enabled: bool) {
        if let Some(metrics) = &self.metrics {
            let value = if enabled { 1 } else { 0 };
            metrics
                .router_sellability
                .with_label_values(&[&router.to_string(), kind.as_str()])
                .set(value);
        }
    }

    fn record_candidate_metric(&self) {
        if let Some(metrics) = &self.metrics {
            metrics.candidates_total.inc();
        }
    }

    fn record_execution_metric(&self) {
        if let Some(metrics) = &self.metrics {
            metrics.executions_total.inc();
        }
    }

    fn record_failure_metric(&self, kind: &'static str) {
        if let Some(metrics) = &self.metrics {
            metrics.failures_total.with_label_values(&[kind]).inc();
        }
    }

    async fn recheck_router_sellability(&mut self) -> Result<()> {
        let disabled: Vec<Address> = self
            .router_meta
            .iter()
            .filter(|(_, meta)| !meta.sellability_enabled)
            .map(|(router, _)| *router)
            .collect();
        if disabled.is_empty() {
            return Ok(());
        }

        let tokens = self.router_verification_tokens();
        if tokens.len() < 2 {
            debug!(
                "router sellability recheck skipped; need at least two non-zero base tokens"
            );
            return Ok(());
        }

        let mut attempted = 0u64;
        let mut updated = 0u64;
        for router in disabled {
            let Some(&factory) = self.router_factories.get(&router) else {
                continue;
            };
            attempted = attempted.saturating_add(1);
            let meta = self
                .router_meta
                .get(&router)
                .copied()
                .unwrap_or(RouterMeta {
                    kind: RouterKind::Unknown,
                    sellability_enabled: false,
                });
            let mut kind = meta.kind;
            if kind == RouterKind::Unknown {
                kind = Self::detect_factory_kind(&self.chain.http, factory, &tokens).await;
            }
            let mut sellability_enabled = false;
            match kind {
                RouterKind::V2 => {
                    if let Some((token_a, token_b)) =
                        Self::find_v2_pair(&self.chain.http, factory, &tokens).await
                    {
                        sellability_enabled =
                            Self::verify_v2_router(&self.chain.http, router, token_a, token_b)
                                .await;
                    }
                }
                RouterKind::Solidly => {
                    if let Some((token_a, token_b, stable)) =
                        Self::find_solidly_pair(&self.chain.http, factory, &tokens).await
                    {
                        sellability_enabled = Self::verify_solidly_router(
                            &self.chain.http,
                            router,
                            token_a,
                            token_b,
                            stable,
                        )
                        .await;
                    }
                }
                RouterKind::Unknown => {}
            }

            let mut kind_changed = false;
            let mut newly_enabled = false;
            let mut previous_kind = None;
            let final_enabled = {
                let entry = self
                    .router_meta
                    .entry(router)
                    .or_insert(RouterMeta {
                        kind,
                        sellability_enabled: false,
                    });
                if entry.kind != kind {
                    previous_kind = Some(entry.kind);
                    entry.kind = kind;
                    kind_changed = true;
                }
                if sellability_enabled && !entry.sellability_enabled {
                    entry.sellability_enabled = true;
                    newly_enabled = true;
                }
                entry.sellability_enabled
            };
            if let Some(previous_kind) = previous_kind {
                self.clear_router_sellability_metric(router, previous_kind);
            }
            if kind_changed || newly_enabled {
                self.set_router_sellability_metric(router, kind, final_enabled);
            }
            if newly_enabled {
                updated = updated.saturating_add(1);
                info!(
                    router = %router,
                    kind = kind.as_str(),
                    "router sellability re-enabled"
                );
            }
        }

        if attempted > 0 {
            if updated > 0 {
                info!(updated, attempted, "router sellability recheck updated");
            } else {
                debug!(attempted, "router sellability recheck completed");
            }
        }

        Ok(())
    }

    async fn detect_factory_kind(
        provider: &DynProvider,
        factory: Address,
        tokens: &[Address],
    ) -> RouterKind {
        if tokens.len() < 2 {
            return RouterKind::Unknown;
        }
        let token_a = tokens[0];
        let token_b = tokens[1];
        let solidly_ok =
            get_pair_address_solidly(provider, factory, token_a, token_b, false)
                .await
                .is_ok();
        let v2_ok = get_pair_address(provider, factory, token_a, token_b)
            .await
            .is_ok();
        match (solidly_ok, v2_ok) {
            (true, false) => RouterKind::Solidly,
            (false, true) => RouterKind::V2,
            _ => RouterKind::Unknown,
        }
    }

    async fn find_v2_pair(
        provider: &DynProvider,
        factory: Address,
        tokens: &[Address],
    ) -> Option<(Address, Address)> {
        for (idx, &token_a) in tokens.iter().enumerate() {
            for &token_b in tokens.iter().skip(idx + 1) {
                if let Ok(Some(_)) =
                    get_pair_address(provider, factory, token_a, token_b).await
                {
                    return Some((token_a, token_b));
                }
            }
        }
        None
    }

    async fn find_solidly_pair(
        provider: &DynProvider,
        factory: Address,
        tokens: &[Address],
    ) -> Option<(Address, Address, bool)> {
        for (idx, &token_a) in tokens.iter().enumerate() {
            for &token_b in tokens.iter().skip(idx + 1) {
                for stable in [false, true] {
                    if let Ok(Some(_)) = get_pair_address_solidly(
                        provider,
                        factory,
                        token_a,
                        token_b,
                        stable,
                    )
                    .await
                    {
                        return Some((token_a, token_b, stable));
                    }
                }
            }
        }
        None
    }

    async fn verify_v2_router(
        provider: &DynProvider,
        router: Address,
        token_a: Address,
        token_b: Address,
    ) -> bool {
        let path = vec![token_a, token_b];
        let call = IUniswapV2Router02::getAmountsOutCall {
            amountIn: U256::from(VERIFY_AMOUNT_IN),
            path,
        };
        let tx = TransactionRequest {
            to: Some(TxKind::Call(router)),
            input: TransactionInput::new(call.abi_encode().into()),
            ..Default::default()
        };
        let data = match provider.call(tx).await {
            Ok(data) => data,
            Err(_) => return false,
        };
        let amounts = match IUniswapV2Router02::getAmountsOutCall::abi_decode_returns(&data) {
            Ok(amounts) => amounts,
            Err(_) => return false,
        };
        !amounts.is_empty()
    }

    async fn verify_solidly_router(
        provider: &DynProvider,
        router: Address,
        token_a: Address,
        token_b: Address,
        stable: bool,
    ) -> bool {
        let routes = vec![Route {
            from: token_a,
            to: token_b,
            stable,
        }];
        let call = ISolidlyRouter::getAmountsOutCall {
            amountIn: U256::from(VERIFY_AMOUNT_IN),
            routes,
        };
        let tx = TransactionRequest {
            to: Some(TxKind::Call(router)),
            input: TransactionInput::new(call.abi_encode().into()),
            ..Default::default()
        };
        let data = match provider.call(tx).await {
            Ok(data) => data,
            Err(_) => return false,
        };
        let amounts = match ISolidlyRouter::getAmountsOutCall::abi_decode_returns(&data) {
            Ok(amounts) => amounts,
            Err(_) => return false,
        };
        !amounts.is_empty()
    }

    pub async fn run(&mut self) -> Result<()> {
        let reconnect = ReconnectConfig::new(
            self.cfg.mempool.ws_reconnect_base_ms,
            self.cfg.mempool.ws_reconnect_max_ms,
        );
        let pending_metrics = self.metrics.as_ref().map(|metrics| metrics.pending.clone());
        let heads_metrics = self.metrics.as_ref().map(|metrics| metrics.heads.clone());
        let txpool_metrics = self.metrics.as_ref().map(|metrics| metrics.txpool.clone());
        let pending_rx = PendingTxStream::new(
            self.chain.ws.clone(),
            self.cfg.mempool.fetch_concurrency,
            reconnect,
            pending_metrics,
        )
        .spawn()
        .await?;
        let heads_rx = NewHeadStream::new(self.chain.ws.clone(), 128, reconnect, heads_metrics)
            .spawn()
            .await?;
        let txpool_rx = if self.cfg.mempool.mode.contains("txpool") {
            Some(
                TxpoolBackfill::new(
                    self.chain.http.clone(),
                    self.cfg.mempool.txpool_poll_ms,
                    1024,
                    txpool_metrics,
                )
                .spawn()
                .await?,
            )
        } else {
            None
        };

        let fetcher = TxFetcher::new(
            self.chain.http.clone(),
            self.cfg.mempool.tx_fetch_timeout_ms,
        );
        let mut pending_rx = pending_rx;
        let mut heads_rx = heads_rx;
        let mut txpool_rx = txpool_rx;
        let nonce_sync_enabled = self.tx_builder.owner != Address::ZERO
            && self.cfg.executor.nonce_sync_interval_ms > 0;
        let mut nonce_sync = tokio::time::interval(Duration::from_millis(
            self.cfg.executor.nonce_sync_interval_ms.max(1),
        ));
        let receipt_poll_enabled = self.cfg.executor.receipt_poll_interval_ms > 0;
        let mut receipt_poll = tokio::time::interval(Duration::from_millis(
            self.cfg.executor.receipt_poll_interval_ms.max(1),
        ));
        let wait_for_mine_enabled = self.wait_for_mine_enabled();
        let mut wait_for_mine_poll = tokio::time::interval(Duration::from_millis(
            self.cfg.strategy.wait_for_mine_poll_interval_ms.max(1),
        ));
        let exit_poll_enabled = self.exit_loop_enabled();
        let mut exit_poll =
            tokio::time::interval(Duration::from_millis(EXIT_POLL_INTERVAL_MS));
        let sellability_recheck_enabled = self.cfg.dex.sellability_recheck_interval_ms > 0;
        let mut sellability_recheck = tokio::time::interval(Duration::from_millis(
            self.cfg
                .dex
                .sellability_recheck_interval_ms
                .max(1),
        ));

        info!("bot running");
        if self.cfg.strategy.wait_for_mine && !wait_for_mine_enabled {
            warn!(
                "wait_for_mine enabled but wait_for_mine_poll_interval_ms is 0; disabling wait-for-mine"
            );
        }
        if nonce_sync_enabled {
            if let Err(err) = self.sync_nonce().await {
                warn!(?err, "nonce sync on startup failed");
            }
        }
        loop {
            select! {
                Some(hash) = pending_rx.recv() => {
                    self.handle_hash(&fetcher, hash).await?;
                    self.counters.maybe_log(now_ms());
                }
                Some(hash) = async {
                    match txpool_rx.as_mut() {
                        Some(rx) => rx.recv().await,
                        None => None,
                    }
                } => {
                    self.handle_hash(&fetcher, hash).await?;
                    self.counters.maybe_log(now_ms());
                }
                Some(head) = heads_rx.recv() => {
                    self.latest_head = Some(head);
                    self.latest_head_seen_ms = Some(now_ms());
                    debug!(block = head, "new head");
                    self.counters.maybe_log(now_ms());
                }
                _ = nonce_sync.tick(), if nonce_sync_enabled => {
                    if let Err(err) = self.sync_nonce().await {
                        warn!(?err, "nonce sync failed");
                    }
                }
                _ = receipt_poll.tick(), if receipt_poll_enabled => {
                    if let Err(err) = self.poll_receipts().await {
                        warn!(?err, "receipt poll failed");
                    }
                    if let Err(err) = self.poll_exit_receipts().await {
                        warn!(?err, "exit receipt poll failed");
                    }
                }
                _ = wait_for_mine_poll.tick(), if wait_for_mine_enabled => {
                    if let Err(err) = self.poll_liquidity_receipts().await {
                        warn!(?err, "add liquidity receipt poll failed");
                    }
                }
                _ = exit_poll.tick(), if exit_poll_enabled => {
                    if let Err(err) = self.poll_positions().await {
                        warn!(?err, "position exit loop failed");
                    }
                }
                _ = sellability_recheck.tick(), if sellability_recheck_enabled => {
                    if let Err(err) = self.recheck_router_sellability().await {
                        warn!(?err, "router sellability recheck failed");
                    }
                }
            }
        }
    }

    async fn handle_hash(&mut self, fetcher: &TxFetcher, hash: B256) -> Result<()> {
        self.counters.totals.hashes_seen =
            self.counters.totals.hashes_seen.saturating_add(1);
        let now = now_ms();
        self.candidate_store.prune(now);
        if !self.dedupe.check_and_update(hash, now) {
            self.counters.totals.dedupe_dropped =
                self.counters.totals.dedupe_dropped.saturating_add(1);
            if let Some(metrics) = &self.metrics {
                metrics.dedup_hits.inc();
            }
            return Ok(());
        }

        let tx = match fetcher.fetch(hash).await? {
            Some(tx) => {
                self.counters.totals.tx_fetched =
                    self.counters.totals.tx_fetched.saturating_add(1);
                tx
            }
            None => {
                self.counters.totals.tx_missing =
                    self.counters.totals.tx_missing.saturating_add(1);
                return Ok(());
            }
        };

        if let Some(mut candidate) = self.detect_liquidity_add(&tx)? {
            if self
                .candidate_store
                .is_terminal(candidate.add_liq_tx_hash)
                || self
                    .candidate_store
                    .has_execution(candidate.add_liq_tx_hash)
            {
                return Ok(());
            }
            self.candidate_store
                .track_detected(candidate.clone(), now);
            self.resolve_candidate_pair(&mut candidate).await?;
            self.candidate_store
                .set_state(candidate.clone(), BotState::Qualifying, now);
            if candidate.pair.is_some() {
                self.counters.totals.pair_resolved =
                    self.counters.totals.pair_resolved.saturating_add(1);
            } else {
                self.counters.totals.pair_unresolved =
                    self.counters.totals.pair_unresolved.saturating_add(1);
            }
            let Some(risk_base) = self.resolve_risk_base(&candidate, now) else {
                return Ok(());
            };
            let wait_for_mine_enabled = self.wait_for_mine_enabled();
            if wait_for_mine_enabled {
                if self.cfg.strategy.same_block_attempt
                    && self.same_block_attempt_allowed(&candidate).await
                {
                    if let Err(err) = self
                        .execute_ready_candidate(candidate.clone(), risk_base, None, false, true)
                        .await
                    {
                        warn!(?err, "same-block attempt failed; deferring to receipt path");
                    }
                    if self.candidate_store.has_execution(candidate.add_liq_tx_hash)
                        || self.candidate_store.is_terminal(candidate.add_liq_tx_hash)
                    {
                        return Ok(());
                    }
                }
                self.enqueue_pending_liquidity(candidate.clone(), now_ms());
                info!(
                    token = %candidate.token,
                    base = %candidate.base,
                    router = %candidate.router,
                    factory = ?candidate.factory,
                    pair = ?candidate.pair,
                    stable = ?candidate.stable,
                    implied_liquidity = %candidate.implied_liquidity,
                    add_liq_tx_hash = %candidate.add_liq_tx_hash,
                    "liquidity candidate queued; waiting for addLiquidity receipt"
                );
                return Ok(());
            }
            self.execute_ready_candidate(candidate, risk_base, None, false, false)
                .await?;
        }

        Ok(())
    }

    fn detect_liquidity_add(&mut self, tx: &MempoolTx) -> Result<Option<LiquidityCandidate>> {
        let Some(to) = tx.to else {
            return Ok(None);
        };
        if !self.routers.contains(&to) {
            return Ok(None);
        }
        self.counters.totals.router_hits = self.counters.totals.router_hits.saturating_add(1);

        let call = match decode_router_calldata(&tx.input)? {
            Some(call) => call,
            None => return Ok(None),
        };
        self.counters.totals.decoded = self.counters.totals.decoded.saturating_add(1);

        match call {
            RouterCall::AddLiquidity(add) => {
                let (base, token, base_amount) = if self.base_tokens.contains(&add.token_a) {
                    (add.token_a, add.token_b, add.amount_a_desired)
                } else if self.base_tokens.contains(&add.token_b) {
                    (add.token_b, add.token_a, add.amount_b_desired)
                } else {
                    self.counters.totals.filtered_base_token =
                        self.counters.totals.filtered_base_token.saturating_add(1);
                    return Ok(None);
                };

                if base_amount < self.min_base_amount {
                    self.counters.totals.filtered_min_amount =
                        self.counters.totals.filtered_min_amount.saturating_add(1);
                    return Ok(None);
                }

                self.counters.totals.candidates =
                    self.counters.totals.candidates.saturating_add(1);
                self.record_candidate_metric();
                Ok(Some(LiquidityCandidate {
                    token,
                    base,
                    router: to,
                    factory: self.router_factories.get(&to).copied(),
                    pair: None,
                    stable: None,
                    add_liq_tx_hash: tx.hash,
                    first_seen_ms: tx.first_seen_ms,
                    implied_liquidity: base_amount,
                }))
            }
            RouterCall::AddLiquidityEth(add) => {
                let base = Address::ZERO;
                if !self.base_tokens.contains(&base) {
                    self.counters.totals.filtered_base_token =
                        self.counters.totals.filtered_base_token.saturating_add(1);
                    return Ok(None);
                }
                let base_amount = tx.value;
                if base_amount < self.min_base_amount {
                    self.counters.totals.filtered_min_amount =
                        self.counters.totals.filtered_min_amount.saturating_add(1);
                    return Ok(None);
                }

                self.counters.totals.candidates =
                    self.counters.totals.candidates.saturating_add(1);
                self.record_candidate_metric();
                Ok(Some(LiquidityCandidate {
                    token: add.token,
                    base,
                    router: to,
                    factory: self.router_factories.get(&to).copied(),
                    pair: None,
                    stable: None,
                    add_liq_tx_hash: tx.hash,
                    first_seen_ms: tx.first_seen_ms,
                    implied_liquidity: base_amount,
                }))
            }
            RouterCall::AddLiquiditySolidly(add) => {
                let (base, token, base_amount) = if self.base_tokens.contains(&add.token_a) {
                    (add.token_a, add.token_b, add.amount_a_desired)
                } else if self.base_tokens.contains(&add.token_b) {
                    (add.token_b, add.token_a, add.amount_b_desired)
                } else {
                    self.counters.totals.filtered_base_token =
                        self.counters.totals.filtered_base_token.saturating_add(1);
                    return Ok(None);
                };

                if base_amount < self.min_base_amount {
                    self.counters.totals.filtered_min_amount =
                        self.counters.totals.filtered_min_amount.saturating_add(1);
                    return Ok(None);
                }

                self.counters.totals.candidates =
                    self.counters.totals.candidates.saturating_add(1);
                self.record_candidate_metric();
                Ok(Some(LiquidityCandidate {
                    token,
                    base,
                    router: to,
                    factory: self.router_factories.get(&to).copied(),
                    pair: None,
                    stable: Some(add.stable),
                    add_liq_tx_hash: tx.hash,
                    first_seen_ms: tx.first_seen_ms,
                    implied_liquidity: base_amount,
                }))
            }
            RouterCall::AddLiquidityEthSolidly(add) => {
                let base = Address::ZERO;
                if !self.base_tokens.contains(&base) {
                    self.counters.totals.filtered_base_token =
                        self.counters.totals.filtered_base_token.saturating_add(1);
                    return Ok(None);
                }
                let base_amount = tx.value;
                if base_amount < self.min_base_amount {
                    self.counters.totals.filtered_min_amount =
                        self.counters.totals.filtered_min_amount.saturating_add(1);
                    return Ok(None);
                }

                self.counters.totals.candidates =
                    self.counters.totals.candidates.saturating_add(1);
                self.record_candidate_metric();
                Ok(Some(LiquidityCandidate {
                    token: add.token,
                    base,
                    router: to,
                    factory: self.router_factories.get(&to).copied(),
                    pair: None,
                    stable: Some(add.stable),
                    add_liq_tx_hash: tx.hash,
                    first_seen_ms: tx.first_seen_ms,
                    implied_liquidity: base_amount,
                }))
            }
        }
    }

    async fn resolve_candidate_pair(&mut self, candidate: &mut LiquidityCandidate) -> Result<()> {
        if candidate.pair.is_some() {
            return Ok(());
        }

        let (base_token, token) = if candidate.base == Address::ZERO {
            match self.wrapped_native {
                Some(addr) => (addr, candidate.token),
                None => {
                    warn!("wrapped_native not configured; skipping pair resolution");
                    return Ok(());
                }
            }
        } else {
            (candidate.base, candidate.token)
        };

        if candidate.factory.is_none() && !self.router_factories.is_empty() {
            warn!(router = %candidate.router, "missing factory mapping for router");
            return Ok(());
        }

        let factory = candidate.factory;
        let factories: &[Address] = match factory {
            Some(ref addr) => std::slice::from_ref(addr),
            None => self.factories.as_slice(),
        };

        if let Some(metadata) = self
            .pair_cache
            .resolve(
                &self.chain.http,
                factories,
                base_token,
                token,
                candidate.stable,
            )
            .await?
        {
            candidate.pair = Some(metadata.pair);
            candidate.factory = Some(metadata.factory);
        }

        Ok(())
    }

    async fn launch_only_liquidity_gate(
        &mut self,
        candidate: &LiquidityCandidate,
        base_token: Address,
        reference_block: Option<u64>,
    ) -> Result<LaunchGateDecision> {
        if !self.cfg.dex.launch_only_liquidity_gate {
            return Ok(LaunchGateDecision::Allow);
        }

        let pair = match self
            .launch_gate_pair_address(candidate, base_token)
            .await?
        {
            Some(pair) => pair,
            None => {
                return Ok(self.launch_gate_unavailable("launch gate requires pair address"));
            }
        };

        let prior_block = match reference_block {
            Some(block) => match block.checked_sub(1) {
                Some(prior) => prior,
                None => {
                    return Ok(self.launch_gate_unavailable(
                        "launch gate prior block unavailable",
                    ))
                }
            },
            None => {
                let latest_block = match self.chain.http.get_block_number().await {
                    Ok(block) => block,
                    Err(err) => {
                        return Ok(self.launch_gate_unavailable(format!(
                            "launch gate block fetch failed: {err}"
                        )));
                    }
                };
                let Some(prior_block) = latest_block.checked_sub(1) else {
                    return Ok(self.launch_gate_unavailable(
                        "launch gate prior block unavailable",
                    ));
                };
                prior_block
            }
        };

        let exists_prior = match contract_exists_at_block(&self.chain.http, pair, prior_block).await
        {
            Ok(exists) => exists,
            Err(err) => {
                let reason = format!("launch gate code check failed: {err}");
                if is_historical_state_unavailable(&reason) {
                    return Ok(self.launch_gate_unavailable(
                        "launch gate historical state unavailable",
                    ));
                }
                return Ok(self.launch_gate_unavailable(reason));
            }
        };
        if !exists_prior {
            return Ok(LaunchGateDecision::Allow);
        }

        let reserves = match get_reserves_at_block(&self.chain.http, pair, prior_block).await {
            Ok(Some(reserves)) => reserves,
            Ok(None) => {
                return Ok(self.launch_gate_unavailable(
                    "launch gate reserves unavailable",
                ));
            }
            Err(err) => {
                let reason = format!("launch gate reserve check failed: {err}");
                if is_historical_state_unavailable(&reason) {
                    return Ok(self.launch_gate_unavailable(
                        "launch gate historical state unavailable",
                    ));
                }
                return Ok(self.launch_gate_unavailable(reason));
            }
        };

        if reserves.0.is_zero() && reserves.1.is_zero() {
            return Ok(LaunchGateDecision::Allow);
        }

        Ok(LaunchGateDecision::Reject {
            reason: "pair had liquidity in prior block".to_string(),
            kind: DropKind::Terminal,
        })
    }

    async fn launch_gate_pair_address(
        &mut self,
        candidate: &LiquidityCandidate,
        base_token: Address,
    ) -> Result<Option<Address>> {
        if let Some(pair) = candidate.pair {
            return Ok(Some(pair));
        }

        let factory = match candidate.factory {
            Some(factory) => Some(factory),
            None => {
                if self.factories.len() == 1 {
                    self.factories.first().copied()
                } else {
                    None
                }
            }
        };
        let Some(factory) = factory else {
            return Ok(None);
        };

        self.pair_cache
            .predict_pair_address(
                &self.chain.http,
                factory,
                base_token,
                candidate.token,
                candidate.stable,
            )
            .await
    }

    fn launch_gate_unavailable(&self, reason: impl Into<String>) -> LaunchGateDecision {
        let reason = reason.into();
        match self.launch_gate_mode {
            LaunchGateMode::BestEffort => {
                warn!(reason = %reason, "launch gate unavailable; allowing candidate");
                LaunchGateDecision::Allow
            }
            LaunchGateMode::Strict => LaunchGateDecision::Reject {
                reason,
                kind: DropKind::Transient,
            },
        }
    }

    async fn execute_candidate(&self, candidate: LiquidityCandidate) -> Result<ExecutionOutcome> {
        if self.tx_builder.owner == Address::ZERO {
            warn!("owner key not configured; skipping execution");
            return Ok(ExecutionOutcome::Skipped(
                "owner key not configured",
                DropKind::Terminal,
            ));
        }

        let nonce = self.nonce.next_nonce();
        let deadline = U256::from(now_ms() / 1000 + self.cfg.dex.deadline_secs as u64);
        let max_block_number = self.resolve_max_block_number().await?;

        let tx = if candidate.base == Address::ZERO {
            let wrapped_native = match self.wrapped_native {
                Some(addr) => addr,
                None => {
                    warn!("wrapped_native not configured; skipping native execution");
                    return Ok(ExecutionOutcome::Skipped(
                        "wrapped_native not configured",
                        DropKind::Terminal,
                    ));
                }
            };
            if let Some(stable) = candidate.stable {
                let params = BuySolidlyEthParams {
                    router: candidate.router,
                    token_in: wrapped_native,
                    token_out: candidate.token,
                    stable,
                    amount_in: candidate.implied_liquidity,
                    min_amount_out: U256::from(0u64),
                    recipient: self.tx_builder.owner,
                    deadline,
                    pair: candidate.pair.unwrap_or(Address::ZERO),
                    min_base_reserve: 0u128,
                    min_token_reserve: 0u128,
                    max_block_number,
                };
                self.tx_builder.build_buy_solidly_eth(params, nonce)
            } else {
                let params = BuyV2EthParams {
                    router: candidate.router,
                    path: vec![wrapped_native, candidate.token],
                    amount_in: candidate.implied_liquidity,
                    min_amount_out: U256::from(0u64),
                    recipient: self.tx_builder.owner,
                    deadline,
                    pair: candidate.pair.unwrap_or(Address::ZERO),
                    min_base_reserve: 0u128,
                    min_token_reserve: 0u128,
                    max_block_number,
                };
                self.tx_builder.build_buy_v2_eth(params, nonce)
            }
        } else if let Some(stable) = candidate.stable {
            let params = BuySolidlyParams {
                router: candidate.router,
                token_in: candidate.base,
                token_out: candidate.token,
                stable,
                amount_in: candidate.implied_liquidity,
                min_amount_out: U256::from(0u64),
                recipient: self.tx_builder.owner,
                deadline,
                pair: candidate.pair.unwrap_or(Address::ZERO),
                min_base_reserve: 0u128,
                min_token_reserve: 0u128,
                max_block_number,
            };
            self.tx_builder.build_buy_solidly(params, nonce)
        } else {
            let params = BuyV2Params {
                router: candidate.router,
                path: vec![candidate.base, candidate.token],
                amount_in: candidate.implied_liquidity,
                min_amount_out: U256::from(0u64),
                recipient: self.tx_builder.owner,
                deadline,
                pair: candidate.pair.unwrap_or(Address::ZERO),
                min_base_reserve: 0u128,
                min_token_reserve: 0u128,
                max_block_number,
            };
            self.tx_builder.build_buy_v2(params, nonce)
        };
        let hash = self.sender.send(tx.clone()).await?;
        info!(%hash, "buy tx sent");
        Ok(ExecutionOutcome::Sent { hash, tx })
    }
}

impl Bot {
    fn wait_for_mine_enabled(&self) -> bool {
        self.cfg.strategy.wait_for_mine && self.cfg.strategy.wait_for_mine_poll_interval_ms > 0
    }

    fn exit_loop_enabled(&self) -> bool {
        self.cfg.strategy.take_profit_bps > 0
            || self.cfg.strategy.stop_loss_bps > 0
            || self.cfg.strategy.max_hold_secs > 0
            || self.cfg.strategy.emergency_reserve_drop_bps > 0
            || self.cfg.strategy.emergency_sell_sim_failures > 0
    }

    fn price_checks_enabled(&self) -> bool {
        self.cfg.strategy.take_profit_bps > 0 || self.cfg.strategy.stop_loss_bps > 0
    }

    async fn poll_positions(&mut self) -> Result<()> {
        if self.positions.is_empty() {
            return Ok(());
        }

        let now = now_ms();
        let price_checks = self.price_checks_enabled();
        let positions = self.positions.snapshot();
        let mut entry_updates: Vec<(B256, U256, U256)> = Vec::new();
        let mut reserve_updates: Vec<(B256, U256, U256)> = Vec::new();
        let mut exit_requests: Vec<(B256, ExitReason, Option<U256>)> = Vec::new();
        let mut positions_changed = false;

        for (hash, mut position) in positions {
            if position.is_open() {
                let emergency_reason = match self
                    .emergency_exit_reason(hash, &position, &mut reserve_updates)
                    .await
                {
                    Ok(reason) => reason,
                    Err(err) => {
                        warn!(
                            ?err,
                            token = %position.token,
                            base = %position.base,
                            router = %position.router,
                            pair = ?position.pair,
                            "emergency exit check failed"
                        );
                        None
                    }
                };
                if let Some(reason) = emergency_reason {
                    exit_requests.push((hash, reason, None));
                    continue;
                }

                if price_checks
                    && (position.entry_price_base_per_token.is_none()
                        || position.entry_token_amount.is_none())
                {
                    match self.entry_quote_for_position(&position).await {
                        Ok(Some((token_amount, price))) => {
                            entry_updates.push((hash, token_amount, price));
                            position.entry_token_amount = Some(token_amount);
                            position.entry_price_base_per_token = Some(price);
                        }
                        Ok(None) => {}
                        Err(err) => {
                            warn!(
                                ?err,
                                token = %position.token,
                                base = %position.base,
                                router = %position.router,
                                pair = ?position.pair,
                                "entry quote failed"
                            );
                        }
                    }
                }

                let current_price = if price_checks {
                    match self.current_price_for_position(&position).await {
                        Ok(price) => price,
                        Err(err) => {
                            warn!(
                                ?err,
                                token = %position.token,
                                base = %position.base,
                                router = %position.router,
                                pair = ?position.pair,
                                "price fetch failed"
                            );
                            None
                        }
                    }
                } else {
                    None
                };

                if let Some(reason) = self.evaluate_exit_reason(&position, now, current_price) {
                    exit_requests.push((hash, reason, current_price));
                }
            } else if let PositionStatus::ExitSignaled { reason, .. } = position.status {
                if position.exit_tx_hash.is_none() && !self.exit_pending(hash) {
                    let current_price = if price_checks {
                        match self.current_price_for_position(&position).await {
                            Ok(price) => price,
                            Err(err) => {
                                warn!(
                                    ?err,
                                    token = %position.token,
                                    base = %position.base,
                                    router = %position.router,
                                    pair = ?position.pair,
                                    "price fetch failed"
                                );
                                None
                            }
                        }
                    } else {
                        None
                    };
                    exit_requests.push((hash, reason, current_price));
                }
            }
        }

        for (hash, token_amount, price) in entry_updates {
            if self
                .positions
                .set_entry_quote(hash, token_amount, price, now)
            {
                positions_changed = true;
            }
        }

        for (hash, base_reserve, token_reserve) in reserve_updates {
            if self
                .positions
                .set_entry_reserves(hash, base_reserve, token_reserve, now)
            {
                positions_changed = true;
            }
        }

        for (hash, reason, current_price) in exit_requests {
            if let Some(position) = self.positions.get(hash).cloned() {
                if position.is_open() {
                    if let Some(position) = self.positions.mark_exit(hash, reason, now) {
                        positions_changed = true;
                        warn!(
                            token = %position.token,
                            base = %position.base,
                            pair = ?position.pair,
                            reason = ?reason,
                            entry_price = ?position.entry_price_base_per_token,
                            current_price = ?current_price,
                            "exit signal"
                        );
                    }
                }
                if let Err(err) = self
                    .execute_exit_for_position(hash, reason, current_price)
                    .await
                {
                    warn!(?err, "exit execution failed");
                }
            }
        }

        let ttl_ms = self
            .cfg
            .strategy
            .exit_signal_ttl_secs
            .saturating_mul(1_000);
        if ttl_ms > 0 {
            let pruned = self.positions.prune_exit_signaled(now, ttl_ms);
            if pruned > 0 {
                debug!(pruned, "pruned exit-signaled positions");
                self.sell_sim_failures
                    .retain(|hash, _| self.positions.get(*hash).is_some());
                positions_changed = true;
            }
        }

        if positions_changed {
            self.persist_positions();
        }

        Ok(())
    }

    fn exit_pending(&self, position_hash: B256) -> bool {
        self.pending_exits
            .values()
            .any(|entry| entry.position_hash == position_hash)
    }

    async fn execute_exit_for_position(
        &mut self,
        position_hash: B256,
        reason: ExitReason,
        current_price: Option<U256>,
    ) -> Result<()> {
        let Some(position) = self.positions.get(position_hash).cloned() else {
            return Ok(());
        };
        if position.exit_tx_hash.is_some() || self.exit_pending(position_hash) {
            return Ok(());
        }
        if self.tx_builder.owner == Address::ZERO {
            warn!(%position_hash, "owner key not configured; skipping exit");
            return Ok(());
        }

        let amount_in = match self.exit_amount_for_position(&position).await {
            Ok(Some(amount)) => amount,
            Ok(None) => {
                warn!(
                    token = %position.token,
                    base = %position.base,
                    "exit skipped; token amount unavailable"
                );
                return Ok(());
            }
            Err(err) => {
                warn!(
                    ?err,
                    token = %position.token,
                    base = %position.base,
                    "exit skipped; token balance unavailable"
                );
                self.touch_exit_retry(position_hash, &position);
                return Ok(());
            }
        };

        let slippage_bps = self.cfg.dex.max_slippage_bps;
        let min_amount_out = if slippage_bps == 0 {
            match self
                .quote_amount_out(
                    position.router,
                    position.token,
                    position.pricing_base,
                    position.stable,
                    amount_in,
                    None,
                )
                .await?
            {
                Some(amount) if !amount.is_zero() => amount,
                Some(_) => {
                    warn!(
                        token = %position.token,
                        base = %position.base,
                        "exit skipped; router quote returned zero"
                    );
                    self.touch_exit_retry(position_hash, &position);
                    return Ok(());
                }
                None => {
                    warn!(
                        token = %position.token,
                        base = %position.base,
                        "exit skipped; router quote unavailable"
                    );
                    self.touch_exit_retry(position_hash, &position);
                    return Ok(());
                }
            }
        } else {
            self.exit_min_amount_out(&position, amount_in, current_price)
                .await?
        };
        let deadline = U256::from(now_ms() / 1000 + self.cfg.dex.deadline_secs as u64);
        let max_block_number = self.resolve_max_block_number().await?;
        let nonce = self.nonce.next_nonce();
        let pair = position.pair.unwrap_or(Address::ZERO);

        let tx = if position.base == Address::ZERO {
            let wrapped_native = match self.wrapped_native {
                Some(addr) => addr,
                None => {
                    warn!(
                        token = %position.token,
                        "wrapped_native not configured; skipping exit"
                    );
                    return Ok(());
                }
            };
            if let Some(stable) = position.stable {
                let params = SellSolidlyEthParams {
                    router: position.router,
                    token_in: position.token,
                    token_out: wrapped_native,
                    stable,
                    amount_in,
                    min_amount_out,
                    recipient: self.tx_builder.owner,
                    deadline,
                    pair,
                    min_base_reserve: 0u128,
                    min_token_reserve: 0u128,
                    max_block_number,
                };
                self.tx_builder.build_sell_solidly_eth(params, nonce)
            } else {
                let params = SellV2EthParams {
                    router: position.router,
                    path: vec![position.token, wrapped_native],
                    amount_in,
                    min_amount_out,
                    recipient: self.tx_builder.owner,
                    deadline,
                    pair,
                    min_base_reserve: 0u128,
                    min_token_reserve: 0u128,
                    max_block_number,
                };
                self.tx_builder.build_sell_v2_eth(params, nonce)
            }
        } else if let Some(stable) = position.stable {
            let params = SellSolidlyParams {
                router: position.router,
                token_in: position.token,
                token_out: position.base,
                stable,
                amount_in,
                min_amount_out,
                recipient: self.tx_builder.owner,
                deadline,
                pair,
                min_base_reserve: 0u128,
                min_token_reserve: 0u128,
                max_block_number,
            };
            self.tx_builder.build_sell_solidly(params, nonce)
        } else {
            let params = SellV2Params {
                router: position.router,
                path: vec![position.token, position.base],
                amount_in,
                min_amount_out,
                recipient: self.tx_builder.owner,
                deadline,
                pair,
                min_base_reserve: 0u128,
                min_token_reserve: 0u128,
                max_block_number,
            };
            self.tx_builder.build_sell_v2(params, nonce)
        };

        let hash = self.sender.send(tx.clone()).await?;
        let now = now_ms();
        self.positions.set_exit_tx_hash(position_hash, hash, now);
        self.persist_positions();
        self.pending_exits.insert(
            hash,
            PendingExit {
                position_hash,
                sent_at_ms: now,
                last_sent_ms: now,
                tx: Some(tx),
            },
        );
        info!(
            %hash,
            token = %position.token,
            base = %position.base,
            reason = ?reason,
            "exit tx sent"
        );
        Ok(())
    }

    async fn exit_amount_for_position(
        &self,
        position: &Position,
    ) -> Result<Option<U256>> {
        let estimate = self.position_token_amount(position);
        if self.tx_builder.owner == Address::ZERO {
            return Ok(estimate);
        }
        match self
            .token_balance(position.token, self.tx_builder.owner)
            .await
        {
            Ok(balance) => {
                if balance.is_zero() {
                    return Ok(None);
                }
                let amount = if let Some(estimate) = estimate {
                    if balance < estimate {
                        balance
                    } else {
                        estimate
                    }
                } else {
                    balance
                };
                Ok(Some(amount))
            }
            Err(err) => Err(err),
        }
    }

    async fn exit_min_amount_out(
        &self,
        position: &Position,
        amount_in: U256,
        current_price: Option<U256>,
    ) -> Result<U256> {
        let slippage_bps = self.cfg.dex.max_slippage_bps;
        if amount_in.is_zero() || position.pricing_base == Address::ZERO {
            return Ok(U256::ZERO);
        }
        let expected_out = if let Some(price) = current_price {
            price.saturating_mul(amount_in) / U256::from(PRICE_SCALE)
        } else if let Some(amount_out) = self
            .quote_amount_out(
                position.router,
                position.token,
                position.pricing_base,
                position.stable,
                amount_in,
                None,
            )
            .await?
        {
            amount_out
        } else {
            U256::ZERO
        };

        if expected_out.is_zero() {
            return Ok(U256::ZERO);
        }
        Ok(apply_bps(expected_out, slippage_bps, false))
    }

    fn touch_exit_retry(&mut self, position_hash: B256, position: &Position) {
        let ttl_ms = self
            .cfg
            .strategy
            .exit_signal_ttl_secs
            .saturating_mul(1_000);
        if ttl_ms == 0 {
            return;
        }
        let PositionStatus::ExitSignaled { decided_ms, .. } = position.status else {
            return;
        };
        let now = now_ms();
        if now.saturating_sub(decided_ms) > ttl_ms {
            return;
        }
        if self.positions.touch(position_hash, now) {
            self.persist_positions();
        }
    }

    async fn token_balance(&self, token: Address, account: Address) -> Result<U256> {
        let call = IERC20::balanceOfCall { account };
        let tx = TransactionRequest {
            to: Some(TxKind::Call(token)),
            input: TransactionInput::new(call.abi_encode().into()),
            ..Default::default()
        };
        let data = self.chain.http.call(tx).await?;
        let balance = IERC20::balanceOfCall::abi_decode_returns(&data)?;
        Ok(balance)
    }

    async fn record_position(
        &mut self,
        candidate_hash: B256,
        entry_tx_hash: B256,
        entry_block: Option<u64>,
        now_ms: u64,
    ) -> Result<()> {
        if self.positions.get(candidate_hash).is_some() {
            return Ok(());
        }
        let Some(candidate) = self.candidate_store.candidate_snapshot(candidate_hash) else {
            return Ok(());
        };
        let position =
            self.build_position(candidate, entry_tx_hash, entry_block, now_ms)
                .await?;
        if self.positions.insert(position.clone()) {
            self.persist_positions();
            info!(
                router = %position.router,
                token = %position.token,
                base = %position.base,
                pair = ?position.pair,
                entry_token_amount = ?position.entry_token_amount,
                entry_price = ?position.entry_price_base_per_token,
                add_liq_tx_hash = %position.add_liq_tx_hash,
                entry_tx_hash = %position.entry_tx_hash,
                "position opened"
            );
            if self.price_checks_enabled()
                && (position.entry_price_base_per_token.is_none()
                    || position.entry_token_amount.is_none())
            {
                warn!(
                    token = %position.token,
                    base = %position.base,
                    pair = ?position.pair,
                    "entry quote unavailable; TP/SL paused until price resolves"
                );
            }
        }
        Ok(())
    }

    async fn build_position(
        &self,
        candidate: LiquidityCandidate,
        entry_tx_hash: B256,
        entry_block: Option<u64>,
        now_ms: u64,
    ) -> Result<Position> {
        let pricing_base = self
            .resolve_pricing_base_token(candidate.base)
            .unwrap_or(candidate.base);
        let mut position = Position {
            add_liq_tx_hash: candidate.add_liq_tx_hash,
            entry_tx_hash,
            router: candidate.router,
            token: candidate.token,
            base: candidate.base,
            pricing_base,
            pair: candidate.pair,
            stable: candidate.stable,
            entry_base_amount: candidate.implied_liquidity,
            entry_token_amount: None,
            entry_price_base_per_token: None,
            entry_base_reserve: None,
            entry_token_reserve: None,
            entry_block,
            opened_ms: now_ms,
            last_update_ms: now_ms,
            status: PositionStatus::Open,
            exit_tx_hash: None,
        };

        match self.entry_quote_for_position(&position).await {
            Ok(Some((token_amount, price))) => {
                position.entry_token_amount = Some(token_amount);
                position.entry_price_base_per_token = Some(price);
            }
            Ok(None) => {}
            Err(err) => {
                warn!(
                    ?err,
                    token = %position.token,
                    base = %position.base,
                    router = %position.router,
                    pair = ?position.pair,
                    "entry quote unavailable"
                );
            }
        }

        if let Ok(Some((base_reserve, token_reserve))) =
            self.reserves_for_position(&position, position.entry_block).await
        {
            if !base_reserve.is_zero() && !token_reserve.is_zero() {
                position.entry_base_reserve = Some(base_reserve);
                position.entry_token_reserve = Some(token_reserve);
            }
        }

        Ok(position)
    }

    async fn entry_quote_for_position(
        &self,
        position: &Position,
    ) -> Result<Option<(U256, U256)>> {
        if position.pricing_base == Address::ZERO {
            return Ok(None);
        }
        if position.entry_base_amount.is_zero() {
            return Ok(None);
        }

        let amount_out = match self
            .quote_amount_out(
                position.router,
                position.pricing_base,
                position.token,
                position.stable,
                position.entry_base_amount,
                position.entry_block,
            )
            .await
        {
            Ok(amount_out) => amount_out,
            Err(err) => {
                warn!(
                    ?err,
                    token = %position.token,
                    base = %position.base,
                    router = %position.router,
                    pair = ?position.pair,
                    "entry quote swap failed"
                );
                None
            }
        };
        if let Some(amount_out) = amount_out {
            if amount_out.is_zero() {
                return Ok(None);
            }
            let price = position
                .entry_base_amount
                .saturating_mul(U256::from(PRICE_SCALE))
                / amount_out;
            return Ok(Some((amount_out, price)));
        }

        if let Some(pair) = position.pair {
            match self
                .spot_price_base_per_token(
                    pair,
                    position.pricing_base,
                    position.token,
                    position.entry_block,
                )
                .await
            {
                Ok(Some(price)) => {
                    if price.is_zero() {
                        return Ok(None);
                    }
                    let token_amount = position
                        .entry_base_amount
                        .saturating_mul(U256::from(PRICE_SCALE))
                        / price;
                    if token_amount.is_zero() {
                        return Ok(None);
                    }
                    return Ok(Some((token_amount, price)));
                }
                Ok(None) => {}
                Err(err) => {
                    let reason = err.to_string();
                    if is_historical_state_unavailable(&reason) {
                        debug!(%reason, "entry spot price unavailable");
                    } else {
                        warn!(%reason, "entry spot price failed");
                    }
                }
            }
        }

        Ok(None)
    }

    async fn current_price_for_position(&self, position: &Position) -> Result<Option<U256>> {
        if position.pricing_base == Address::ZERO {
            return Ok(None);
        }
        let Some(token_amount) = self.position_token_amount(position) else {
            return Ok(None);
        };

        let amount_out = match self
            .quote_amount_out(
                position.router,
                position.token,
                position.pricing_base,
                position.stable,
                token_amount,
                None,
            )
            .await
        {
            Ok(amount_out) => amount_out,
            Err(err) => {
                warn!(
                    ?err,
                    token = %position.token,
                    base = %position.base,
                    router = %position.router,
                    pair = ?position.pair,
                    "price quote failed"
                );
                None
            }
        };
        if let Some(amount_out) = amount_out {
            if token_amount.is_zero() {
                return Ok(None);
            }
            let price =
                amount_out.saturating_mul(U256::from(PRICE_SCALE)) / token_amount;
            return Ok(Some(price));
        }

        if let Some(pair) = position.pair {
            match self
                .spot_price_base_per_token(
                    pair,
                    position.pricing_base,
                    position.token,
                    None,
                )
                .await
            {
                Ok(price) => return Ok(price),
                Err(err) => {
                    warn!(
                        ?err,
                        token = %position.token,
                        base = %position.base,
                        router = %position.router,
                        pair = ?position.pair,
                        "spot price failed"
                    );
                }
            }
        }

        Ok(None)
    }

    fn position_token_amount(&self, position: &Position) -> Option<U256> {
        if let Some(amount) = position.entry_token_amount {
            if amount.is_zero() {
                return None;
            }
            return Some(amount);
        }
        let entry_price = position.entry_price_base_per_token?;
        if entry_price.is_zero() {
            return None;
        }
        let amount = position
            .entry_base_amount
            .saturating_mul(U256::from(PRICE_SCALE))
            / entry_price;
        if amount.is_zero() {
            return None;
        }
        Some(amount)
    }

    async fn quote_amount_out(
        &self,
        router: Address,
        token_in: Address,
        token_out: Address,
        stable: Option<bool>,
        amount_in: U256,
        block: Option<u64>,
    ) -> Result<Option<U256>> {
        if amount_in.is_zero() {
            return Ok(None);
        }
        match stable {
            Some(stable) => {
                if let Some(amount) = self
                    .quote_solidly_swap_amount_out(
                        router,
                        token_in,
                        token_out,
                        stable,
                        amount_in,
                        block,
                    )
                    .await?
                {
                    return Ok(Some(amount));
                }
                self.quote_solidly_amounts_out(
                    router,
                    token_in,
                    token_out,
                    stable,
                    amount_in,
                    block,
                )
                .await
            }
            None => {
                if let Some(amount) = self
                    .quote_v2_swap_amount_out(
                        router,
                        token_in,
                        token_out,
                        amount_in,
                        block,
                    )
                    .await?
                {
                    return Ok(Some(amount));
                }
                self.quote_v2_amounts_out(router, token_in, token_out, amount_in, block)
                    .await
            }
        }
    }

    async fn quote_v2_swap_amount_out(
        &self,
        router: Address,
        token_in: Address,
        token_out: Address,
        amount_in: U256,
        block: Option<u64>,
    ) -> Result<Option<U256>> {
        let path = vec![token_in, token_out];
        let call = IUniswapV2Router02::swapExactTokensForTokensCall {
            amountIn: amount_in,
            amountOutMin: U256::from(0u64),
            path,
            to: SIMULATION_SENDER,
            deadline: U256::from(SIMULATION_DEADLINE_SECS),
        };
        let tx = TransactionRequest {
            from: Some(SIMULATION_SENDER),
            to: Some(TxKind::Call(router)),
            input: TransactionInput::new(call.abi_encode().into()),
            ..Default::default()
        };
        let overrides = simulation_overrides(token_in, SIMULATION_SENDER, router, amount_in);
        let call = match block {
            Some(block_number) => self
                .chain
                .http
                .call(tx)
                .overrides(overrides)
                .block(BlockId::number(block_number)),
            None => self.chain.http.call(tx).overrides(overrides),
        };
        let data = match call.await {
            Ok(data) => data,
            Err(err) => {
                debug!(?err, "v2 swap quote failed");
                return Ok(None);
            }
        };
        let amounts =
            match IUniswapV2Router02::swapExactTokensForTokensCall::abi_decode_returns(&data) {
                Ok(amounts) => amounts,
                Err(err) => {
                    debug!(?err, "v2 swap quote decode failed");
                    return Ok(None);
                }
            };
        Ok(amounts.last().copied())
    }

    async fn quote_v2_amounts_out(
        &self,
        router: Address,
        token_in: Address,
        token_out: Address,
        amount_in: U256,
        block: Option<u64>,
    ) -> Result<Option<U256>> {
        let path = vec![token_in, token_out];
        let call = IUniswapV2Router02::getAmountsOutCall {
            amountIn: amount_in,
            path,
        };
        let tx = TransactionRequest {
            to: Some(TxKind::Call(router)),
            input: TransactionInput::new(call.abi_encode().into()),
            ..Default::default()
        };
        let call = match block {
            Some(block_number) => self.chain.http.call(tx).block(BlockId::number(block_number)),
            None => self.chain.http.call(tx),
        };
        let data = match call.await {
            Ok(data) => data,
            Err(err) => {
                debug!(?err, "v2 getAmountsOut quote failed");
                return Ok(None);
            }
        };
        let amounts = match IUniswapV2Router02::getAmountsOutCall::abi_decode_returns(&data) {
            Ok(amounts) => amounts,
            Err(err) => {
                debug!(?err, "v2 getAmountsOut decode failed");
                return Ok(None);
            }
        };
        Ok(amounts.last().copied())
    }

    async fn quote_solidly_swap_amount_out(
        &self,
        router: Address,
        token_in: Address,
        token_out: Address,
        stable: bool,
        amount_in: U256,
        block: Option<u64>,
    ) -> Result<Option<U256>> {
        let routes = vec![Route {
            from: token_in,
            to: token_out,
            stable,
        }];
        let call = ISolidlyRouter::swapExactTokensForTokensCall {
            amountIn: amount_in,
            amountOutMin: U256::from(0u64),
            routes,
            to: SIMULATION_SENDER,
            deadline: U256::from(SIMULATION_DEADLINE_SECS),
        };
        let tx = TransactionRequest {
            from: Some(SIMULATION_SENDER),
            to: Some(TxKind::Call(router)),
            input: TransactionInput::new(call.abi_encode().into()),
            ..Default::default()
        };
        let overrides = simulation_overrides(token_in, SIMULATION_SENDER, router, amount_in);
        let call = match block {
            Some(block_number) => self
                .chain
                .http
                .call(tx)
                .overrides(overrides)
                .block(BlockId::number(block_number)),
            None => self.chain.http.call(tx).overrides(overrides),
        };
        let data = match call.await {
            Ok(data) => data,
            Err(err) => {
                debug!(?err, "solidly swap quote failed");
                return Ok(None);
            }
        };
        let amounts =
            match ISolidlyRouter::swapExactTokensForTokensCall::abi_decode_returns(&data) {
                Ok(amounts) => amounts,
                Err(err) => {
                    debug!(?err, "solidly swap quote decode failed");
                    return Ok(None);
                }
            };
        Ok(amounts.last().copied())
    }

    async fn quote_solidly_amounts_out(
        &self,
        router: Address,
        token_in: Address,
        token_out: Address,
        stable: bool,
        amount_in: U256,
        block: Option<u64>,
    ) -> Result<Option<U256>> {
        let routes = vec![Route {
            from: token_in,
            to: token_out,
            stable,
        }];
        let call = ISolidlyRouter::getAmountsOutCall {
            amountIn: amount_in,
            routes,
        };
        let tx = TransactionRequest {
            to: Some(TxKind::Call(router)),
            input: TransactionInput::new(call.abi_encode().into()),
            ..Default::default()
        };
        let call = match block {
            Some(block_number) => self.chain.http.call(tx).block(BlockId::number(block_number)),
            None => self.chain.http.call(tx),
        };
        let data = match call.await {
            Ok(data) => data,
            Err(err) => {
                debug!(?err, "solidly getAmountsOut quote failed");
                return Ok(None);
            }
        };
        let amounts = match ISolidlyRouter::getAmountsOutCall::abi_decode_returns(&data) {
            Ok(amounts) => amounts,
            Err(err) => {
                debug!(?err, "solidly getAmountsOut decode failed");
                return Ok(None);
            }
        };
        Ok(amounts.last().copied())
    }

    async fn spot_price_base_per_token(
        &self,
        pair: Address,
        base: Address,
        token: Address,
        block: Option<u64>,
    ) -> Result<Option<U256>> {
        let Some((token0, token1)) = get_pair_tokens(&self.chain.http, pair).await? else {
            return Ok(None);
        };

        let reserves = match block {
            Some(block_number) => {
                get_reserves_at_block(&self.chain.http, pair, block_number).await?
            }
            None => get_reserves(&self.chain.http, pair).await?,
        };
        let Some((reserve0, reserve1)) = reserves else {
            return Ok(None);
        };

        let (base_reserve, token_reserve) = if token0 == base && token1 == token {
            (reserve0, reserve1)
        } else if token0 == token && token1 == base {
            (reserve1, reserve0)
        } else {
            return Ok(None);
        };

        if token_reserve.is_zero() {
            return Ok(None);
        }

        let price = base_reserve.saturating_mul(U256::from(PRICE_SCALE)) / token_reserve;
        Ok(Some(price))
    }

    async fn reserves_for_position(
        &self,
        position: &Position,
        block: Option<u64>,
    ) -> Result<Option<(U256, U256)>> {
        let Some(pair) = position.pair else {
            return Ok(None);
        };
        if position.pricing_base == Address::ZERO {
            return Ok(None);
        }
        let Some((token0, token1)) = get_pair_tokens(&self.chain.http, pair).await? else {
            return Ok(None);
        };
        let reserves = match block {
            Some(block_number) => {
                get_reserves_at_block(&self.chain.http, pair, block_number).await?
            }
            None => get_reserves(&self.chain.http, pair).await?,
        };
        let Some((reserve0, reserve1)) = reserves else {
            return Ok(None);
        };
        let base = position.pricing_base;
        let token = position.token;
        let (base_reserve, token_reserve) = if token0 == base && token1 == token {
            (reserve0, reserve1)
        } else if token0 == token && token1 == base {
            (reserve1, reserve0)
        } else {
            return Ok(None);
        };
        Ok(Some((base_reserve, token_reserve)))
    }

    fn resolve_pricing_base_token(&self, base: Address) -> Option<Address> {
        if base == Address::ZERO {
            self.wrapped_native
        } else {
            Some(base)
        }
    }

    fn evaluate_exit_reason(
        &self,
        position: &Position,
        now_ms: u64,
        current_price: Option<U256>,
    ) -> Option<ExitReason> {
        if !position.is_open() {
            return None;
        }

        if let (Some(entry_price), Some(current_price)) =
            (position.entry_price_base_per_token, current_price)
        {
            if !entry_price.is_zero() {
                if self.cfg.strategy.take_profit_bps > 0
                    && current_price
                        >= apply_bps(entry_price, self.cfg.strategy.take_profit_bps, true)
                {
                    return Some(ExitReason::TakeProfit);
                }
                if self.cfg.strategy.stop_loss_bps > 0
                    && current_price
                        <= apply_bps(entry_price, self.cfg.strategy.stop_loss_bps, false)
                {
                    return Some(ExitReason::StopLoss);
                }
            }
        }

        let max_hold_ms = self.cfg.strategy.max_hold_secs.saturating_mul(1_000);
        if max_hold_ms > 0
            && now_ms.saturating_sub(position.opened_ms) >= max_hold_ms
        {
            return Some(ExitReason::MaxHold);
        }

        None
    }

    async fn emergency_exit_reason(
        &mut self,
        position_hash: B256,
        position: &Position,
        reserve_updates: &mut Vec<(B256, U256, U256)>,
    ) -> Result<Option<ExitReason>> {
        if !position.is_open() {
            return Ok(None);
        }

        let reserve_drop_bps = self.cfg.strategy.emergency_reserve_drop_bps;
        if reserve_drop_bps > 0 {
            let reserve_drop_bps = reserve_drop_bps.min(BPS_DENOMINATOR as u32);
            if let Some((base_reserve, token_reserve)) =
                self.reserves_for_position(position, None).await?
            {
                if (position.entry_base_reserve.is_none()
                    || position.entry_token_reserve.is_none())
                    && !base_reserve.is_zero()
                    && !token_reserve.is_zero()
                {
                    reserve_updates.push((position_hash, base_reserve, token_reserve));
                }
                if base_reserve.is_zero() || token_reserve.is_zero() {
                    return Ok(Some(ExitReason::EmergencyReserveDrop));
                }
                if let (Some(entry_base), Some(entry_token)) =
                    (position.entry_base_reserve, position.entry_token_reserve)
                {
                    if !entry_base.is_zero() && !entry_token.is_zero() {
                        let min_base = apply_bps(entry_base, reserve_drop_bps, false);
                        let min_token = apply_bps(entry_token, reserve_drop_bps, false);
                        if base_reserve <= min_base || token_reserve <= min_token {
                            return Ok(Some(ExitReason::EmergencyReserveDrop));
                        }
                    }
                }
            }
        }

        let failure_limit = self.cfg.strategy.emergency_sell_sim_failures;
        if failure_limit > 0 && position.pricing_base != Address::ZERO {
            if let Some(amount_in) = self.position_token_amount(position) {
                let amount_out = self
                    .quote_amount_out(
                        position.router,
                        position.token,
                        position.pricing_base,
                        position.stable,
                        amount_in,
                        None,
                    )
                    .await
                    .unwrap_or(None);
                let success = matches!(amount_out, Some(amount) if !amount.is_zero());
                if success {
                    self.sell_sim_failures.remove(&position_hash);
                } else {
                    let failures = self.sell_sim_failures.entry(position_hash).or_insert(0);
                    *failures = failures.saturating_add(1);
                    if *failures >= failure_limit {
                        return Ok(Some(ExitReason::EmergencySellSimFailure));
                    }
                }
            }
        }

        Ok(None)
    }

    fn invalidate_pair_cache(&mut self, candidate: &LiquidityCandidate) {
        if candidate.pair.is_some() {
            return;
        }

        let base_token = if candidate.base == Address::ZERO {
            match self.wrapped_native {
                Some(addr) => addr,
                None => {
                    warn!("wrapped_native not configured; skipping pair cache refresh");
                    return;
                }
            }
        } else {
            candidate.base
        };

        if candidate.factory.is_none() && !self.router_factories.is_empty() {
            warn!(router = %candidate.router, "missing factory mapping for router");
            return;
        }

        let factory = candidate.factory;
        let factories: &[Address] = match factory {
            Some(ref addr) => std::slice::from_ref(addr),
            None => self.factories.as_slice(),
        };

        for &factory in factories {
            self.pair_cache
                .invalidate(factory, base_token, candidate.token, candidate.stable);
        }
    }

    async fn same_block_attempt_allowed(&self, candidate: &LiquidityCandidate) -> bool {
        if !self.cfg.strategy.same_block_requires_reserves {
            return true;
        }
        let Some(pair) = candidate.pair else {
            info!(
                token = %candidate.token,
                base = %candidate.base,
                router = %candidate.router,
                "same-block attempt skipped; pair unresolved"
            );
            return false;
        };
        match get_reserves(&self.chain.http, pair).await {
            Ok(Some((reserve0, reserve1))) => {
                if reserve0.is_zero() || reserve1.is_zero() {
                    info!(
                        token = %candidate.token,
                        base = %candidate.base,
                        router = %candidate.router,
                        "same-block attempt skipped; reserves not ready"
                    );
                    return false;
                }
                true
            }
            Ok(None) => {
                info!(
                    token = %candidate.token,
                    base = %candidate.base,
                    router = %candidate.router,
                    "same-block attempt skipped; reserves unavailable"
                );
                false
            }
            Err(err) => {
                warn!(
                    ?err,
                    token = %candidate.token,
                    base = %candidate.base,
                    router = %candidate.router,
                    "same-block attempt skipped; reserve check failed"
                );
                false
            }
        }
    }

    fn enqueue_pending_liquidity(&mut self, candidate: LiquidityCandidate, now_ms: u64) {
        let hash = candidate.add_liq_tx_hash;
        self.pending_liquidity
            .entry(hash)
            .and_modify(|entry| entry.candidate = candidate.clone())
            .or_insert(PendingLiquidity {
                candidate,
                enqueued_ms: now_ms,
            });
    }

    async fn poll_liquidity_receipts(&mut self) -> Result<()> {
        if self.pending_liquidity.is_empty() {
            return Ok(());
        }
        let now = now_ms();
        let timeout_ms = self.cfg.strategy.wait_for_mine_timeout_ms;
        let entries: Vec<(B256, PendingLiquidity)> = self
            .pending_liquidity
            .iter()
            .map(|(hash, entry)| (*hash, entry.clone()))
            .collect();

        for (tx_hash, entry) in entries {
            if self.candidate_store.is_terminal(tx_hash)
                || self.candidate_store.has_execution(tx_hash)
            {
                self.pending_liquidity.remove(&tx_hash);
                continue;
            }
            if timeout_ms > 0 && now.saturating_sub(entry.enqueued_ms) > timeout_ms {
                warn!(%tx_hash, "add liquidity receipt timeout");
                self.pending_liquidity.remove(&tx_hash);
                self.record_failure_metric("liquidity_timeout");
                self.candidate_store.drop_transient(
                    tx_hash,
                    "addLiquidity receipt timeout",
                    now,
                );
                continue;
            }

            match self.chain.http.get_transaction_receipt(tx_hash).await {
                Ok(Some(receipt)) => {
                    let success = receipt.inner.status();
                    let mined_block = receipt.block_number;
                    if !success {
                        warn!(%tx_hash, "add liquidity reverted");
                        self.pending_liquidity.remove(&tx_hash);
                        self.record_failure_metric("liquidity_revert");
                        self.candidate_store.drop_terminal(
                            tx_hash,
                            "addLiquidity reverted",
                            now,
                        );
                        continue;
                    }
                    if let Err(err) = self
                        .handle_mined_liquidity(entry.candidate, mined_block)
                        .await
                    {
                        warn!(?err, %tx_hash, "failed to process mined add liquidity");
                        continue;
                    }
                    self.pending_liquidity.remove(&tx_hash);
                }
                Ok(None) => {}
                Err(err) => {
                    warn!(?err, %tx_hash, "add liquidity receipt fetch failed");
                }
            }
        }

        Ok(())
    }

    async fn handle_mined_liquidity(
        &mut self,
        mut candidate: LiquidityCandidate,
        mined_block: Option<u64>,
    ) -> Result<()> {
        if candidate.pair.is_none() {
            self.invalidate_pair_cache(&candidate);
            self.resolve_candidate_pair(&mut candidate).await?;
        }
        self.candidate_store
            .set_state(candidate.clone(), BotState::Qualifying, now_ms());
        let Some(risk_base) = self.resolve_risk_base(&candidate, now_ms()) else {
            return Ok(());
        };
        info!(
            token = %candidate.token,
            base = %candidate.base,
            router = %candidate.router,
            factory = ?candidate.factory,
            pair = ?candidate.pair,
            stable = ?candidate.stable,
            implied_liquidity = %candidate.implied_liquidity,
            add_liq_tx_hash = %candidate.add_liq_tx_hash,
            add_liq_block = ?mined_block,
            "add liquidity mined; evaluating candidate"
        );
        self.execute_ready_candidate(candidate, risk_base, mined_block, false, false)
            .await
    }

    fn resolve_risk_base(
        &mut self,
        candidate: &LiquidityCandidate,
        now_ms: u64,
    ) -> Option<Address> {
        if candidate.base == Address::ZERO {
            match self.wrapped_native {
                Some(addr) => Some(addr),
                None => {
                    warn!("wrapped_native not configured; skipping native execution");
                    self.record_failure_metric("config");
                    self.candidate_store.drop_terminal(
                        candidate.add_liq_tx_hash,
                        "wrapped_native not configured",
                        now_ms,
                    );
                    None
                }
            }
        } else {
            Some(candidate.base)
        }
    }

    async fn execute_ready_candidate(
        &mut self,
        candidate: LiquidityCandidate,
        risk_base: Address,
        mined_block: Option<u64>,
        allow_unresolved_pair: bool,
        defer_transient: bool,
    ) -> Result<()> {
        let now = now_ms();
        let allow_unresolved_pair =
            allow_unresolved_pair || self.cfg.dex.allow_execution_without_pair;
        match self
            .launch_only_liquidity_gate(&candidate, risk_base, mined_block)
            .await?
        {
            LaunchGateDecision::Allow => {}
            LaunchGateDecision::Reject { reason, kind } => {
                if defer_transient && kind == DropKind::Transient {
                    info!(
                        token = %candidate.token,
                        base = %candidate.base,
                        router = %candidate.router,
                        factory = ?candidate.factory,
                        pair = ?candidate.pair,
                        stable = ?candidate.stable,
                        reason = %reason,
                        "launch gate unavailable; deferring candidate"
                    );
                    return Ok(());
                }
                warn!(
                    token = %candidate.token,
                    base = %candidate.base,
                    router = %candidate.router,
                    factory = ?candidate.factory,
                    pair = ?candidate.pair,
                    stable = ?candidate.stable,
                    reason = %reason,
                    "launch gate rejected candidate"
                );
                self.record_failure_metric("launch_gate");
                match kind {
                    DropKind::Transient => {
                        self.candidate_store
                            .drop_transient(candidate.add_liq_tx_hash, reason, now);
                    }
                    DropKind::Terminal => {
                        self.candidate_store
                            .drop_terminal(candidate.add_liq_tx_hash, reason, now);
                    }
                }
                return Ok(());
            }
        }
        if candidate.pair.is_none() && !allow_unresolved_pair {
            if defer_transient {
                info!(
                    token = %candidate.token,
                    base = %candidate.base,
                    router = %candidate.router,
                    "pair unresolved; deferring candidate"
                );
                return Ok(());
            }
            warn!(
                token = %candidate.token,
                base = %candidate.base,
                router = %candidate.router,
                "pair unresolved; skipping execution"
            );
            self.record_failure_metric("pair_unresolved");
            self.candidate_store.drop_transient(
                candidate.add_liq_tx_hash,
                "pair unresolved",
                now,
            );
            return Ok(());
        }
        if candidate.pair.is_none() && allow_unresolved_pair {
            warn!(
                token = %candidate.token,
                base = %candidate.base,
                router = %candidate.router,
                "pair unresolved; executing without reserve guard"
            );
        }
        info!(
            token = %candidate.token,
            base = %candidate.base,
            router = %candidate.router,
            factory = ?candidate.factory,
            pair = ?candidate.pair,
            stable = ?candidate.stable,
            implied_liquidity = %candidate.implied_liquidity,
            add_liq_tx_hash = %candidate.add_liq_tx_hash,
            add_liq_block = ?mined_block,
            "liquidity candidate ready"
        );
        let sellability_enabled = self
            .router_meta
            .get(&candidate.router)
            .map(|meta| meta.sellability_enabled)
            .unwrap_or(false);
        let ctx = RiskContext {
            provider: &self.chain.http,
            router: candidate.router,
            base_token: risk_base,
            token: candidate.token,
            pair: candidate.pair,
            stable: candidate.stable,
            sellability_enabled,
        };
        let decision = self.risk.assess(&ctx).await?;
        if !decision.pass {
            self.counters.totals.risk_fail =
                self.counters.totals.risk_fail.saturating_add(1);
            warn!(score = decision.score, reasons = ?decision.reasons, "risk reject");
            self.record_failure_metric("risk");
            let reason = if decision.reasons.is_empty() {
                "risk rejected".to_string()
            } else {
                format!("risk rejected: {}", decision.reasons.join("; "))
            };
            self.candidate_store
                .drop_terminal(candidate.add_liq_tx_hash, reason, now);
            return Ok(());
        }
        self.counters.totals.risk_pass =
            self.counters.totals.risk_pass.saturating_add(1);
        self.candidate_store
            .set_state(candidate.clone(), BotState::Executing, now);
        let candidate_hash = candidate.add_liq_tx_hash;
        match self.execute_candidate(candidate).await? {
            ExecutionOutcome::Sent { hash, tx } => {
                self.candidate_store
                    .mark_executed(candidate_hash, hash, now);
                self.counters.totals.executed =
                    self.counters.totals.executed.saturating_add(1);
                self.record_execution_metric();
                self.pending_receipts.insert(
                    hash,
                    PendingReceipt {
                        candidate_hash,
                        sent_at_ms: now,
                        last_sent_ms: now,
                        tx,
                    },
                );
            }
            ExecutionOutcome::Skipped(reason, kind) => {
                if defer_transient && kind == DropKind::Transient {
                    info!(%reason, "execution deferred");
                    return Ok(());
                }
                self.record_failure_metric("execution_skipped");
                match kind {
                    DropKind::Transient => {
                        self.candidate_store.drop_transient(candidate_hash, reason, now);
                    }
                    DropKind::Terminal => {
                        self.candidate_store.drop_terminal(candidate_hash, reason, now);
                    }
                }
                return Ok(());
            }
        }

        Ok(())
    }

    async fn poll_receipts(&mut self) -> Result<()> {
        if self.pending_receipts.is_empty() {
            return Ok(());
        }
        let now = now_ms();
        let timeout_ms = self.cfg.executor.receipt_timeout_ms;
        let bump_pct = self.cfg.executor.bump_pct;
        let bump_interval_ms = self.cfg.executor.bump_interval_ms;
        let entries: Vec<(B256, PendingReceipt)> = self
            .pending_receipts
            .iter()
            .map(|(hash, entry)| (*hash, entry.clone()))
            .collect();
        let mut resolved_candidates = HashSet::new();
        let mut bump_attempted = HashSet::new();

        for (tx_hash, entry) in entries {
            if resolved_candidates.contains(&entry.candidate_hash) {
                continue;
            }
            if timeout_ms > 0 && now.saturating_sub(entry.sent_at_ms) > timeout_ms {
                warn!(%tx_hash, "receipt retry window expired");
                self.remove_pending_candidate(entry.candidate_hash);
                resolved_candidates.insert(entry.candidate_hash);
                continue;
            }

            match self.chain.http.get_transaction_receipt(tx_hash).await {
                Ok(Some(receipt)) => {
                    let success = receipt.inner.status();
                    let block = receipt.block_number.unwrap_or_default();
                    if success {
                        info!(%tx_hash, block, "tx confirmed");
                        if let Err(err) = self
                            .record_position(
                                entry.candidate_hash,
                                tx_hash,
                                receipt.block_number,
                                now,
                            )
                            .await
                        {
                            warn!(?err, "position tracking failed");
                        }
                        self.remove_pending_candidate(entry.candidate_hash);
                        resolved_candidates.insert(entry.candidate_hash);
                        continue;
                    } else {
                        warn!(%tx_hash, block, "tx reverted");
                        let attempts = self.candidate_store.exec_attempts(entry.candidate_hash);
                        if self.wait_for_mine_enabled() && attempts < MAX_EXECUTION_ATTEMPTS {
                            self.remove_pending_candidate(entry.candidate_hash);
                            resolved_candidates.insert(entry.candidate_hash);
                            self.candidate_store
                                .mark_execution_failed(entry.candidate_hash, now);
                            if let Err(err) = self
                                .fallback_after_revert(entry.candidate_hash, now)
                                .await
                            {
                                warn!(?err, "fallback after execution revert failed");
                            }
                            continue;
                        } else {
                            self.record_failure_metric("execution_revert");
                            self.candidate_store.drop_terminal(
                                entry.candidate_hash,
                                "execution reverted",
                                now,
                            );
                        }
                    }
                    self.remove_pending_candidate(entry.candidate_hash);
                    resolved_candidates.insert(entry.candidate_hash);
                }
                Ok(None) => {
                    if bump_pct > 0
                        && bump_interval_ms > 0
                        && now.saturating_sub(entry.last_sent_ms) >= bump_interval_ms
                    {
                        if bump_attempted.contains(&entry.candidate_hash) {
                            continue;
                        }
                        let latest_sent = self
                            .latest_sent_ms_for_candidate(entry.candidate_hash)
                            .unwrap_or(entry.last_sent_ms);
                        if entry.last_sent_ms < latest_sent {
                            continue;
                        }
                        bump_attempted.insert(entry.candidate_hash);
                        let mut bumped_tx = entry.tx.clone();
                        if !bump_tx_fees(&mut bumped_tx, bump_pct) {
                            warn!(%tx_hash, "gas bump skipped: missing fee fields");
                            continue;
                        }
                        match self.sender.send(bumped_tx.clone()).await {
                            Ok(new_hash) => {
                                info!(%tx_hash, %new_hash, "tx fee bumped");
                                self.pending_receipts.insert(
                                    new_hash,
                                    PendingReceipt {
                                        candidate_hash: entry.candidate_hash,
                                        sent_at_ms: entry.sent_at_ms,
                                        last_sent_ms: now,
                                        tx: bumped_tx,
                                    },
                                );
                                self.candidate_store
                                    .mark_executed(entry.candidate_hash, new_hash, now_ms());
                            }
                            Err(err) => {
                                warn!(?err, %tx_hash, "gas bump failed");
                            }
                        }
                    }
                }
                Err(err) => {
                    warn!(?err, %tx_hash, "receipt fetch failed");
                }
            }
        }

        Ok(())
    }

    async fn poll_exit_receipts(&mut self) -> Result<()> {
        if self.pending_exits.is_empty() {
            return Ok(());
        }
        let now = now_ms();
        let timeout_ms = self.cfg.executor.receipt_timeout_ms;
        let bump_pct = self.cfg.executor.bump_pct;
        let bump_interval_ms = self.cfg.executor.bump_interval_ms;
        let entries: Vec<(B256, PendingExit)> = self
            .pending_exits
            .iter()
            .map(|(hash, entry)| (*hash, entry.clone()))
            .collect();
        let mut resolved_positions = HashSet::new();
        let mut bump_attempted = HashSet::new();
        let mut positions_changed = false;

        for (tx_hash, entry) in entries {
            if resolved_positions.contains(&entry.position_hash) {
                continue;
            }
            if timeout_ms > 0 && now.saturating_sub(entry.sent_at_ms) > timeout_ms {
                warn!(%tx_hash, "exit receipt retry window expired");
                self.positions.clear_exit_tx_hash(entry.position_hash, now);
                positions_changed = true;
                self.remove_pending_exit(entry.position_hash);
                resolved_positions.insert(entry.position_hash);
                continue;
            }

            match self.chain.http.get_transaction_receipt(tx_hash).await {
                Ok(Some(receipt)) => {
                    let success = receipt.inner.status();
                    let block = receipt.block_number.unwrap_or_default();
                    if success {
                        info!(%tx_hash, block, "exit tx confirmed");
                        self.positions.remove(entry.position_hash);
                        self.sell_sim_failures.remove(&entry.position_hash);
                        positions_changed = true;
                    } else {
                        warn!(%tx_hash, block, "exit tx reverted");
                        self.positions.clear_exit_tx_hash(entry.position_hash, now);
                        positions_changed = true;
                    }
                    self.remove_pending_exit(entry.position_hash);
                    resolved_positions.insert(entry.position_hash);
                }
                Ok(None) => {
                    if bump_pct > 0
                        && bump_interval_ms > 0
                        && now.saturating_sub(entry.last_sent_ms) >= bump_interval_ms
                    {
                        if bump_attempted.contains(&entry.position_hash) {
                            continue;
                        }
                        let latest_sent = self
                            .latest_sent_ms_for_exit(entry.position_hash)
                            .unwrap_or(entry.last_sent_ms);
                        if entry.last_sent_ms < latest_sent {
                            continue;
                        }
                        bump_attempted.insert(entry.position_hash);
                        let Some(mut bumped_tx) = entry.tx.clone() else {
                            warn!(%tx_hash, "exit gas bump skipped: missing tx");
                            continue;
                        };
                        if !bump_tx_fees(&mut bumped_tx, bump_pct) {
                            warn!(%tx_hash, "exit gas bump skipped: missing fee fields");
                            continue;
                        }
                        match self.sender.send(bumped_tx.clone()).await {
                            Ok(new_hash) => {
                                info!(%tx_hash, %new_hash, "exit tx fee bumped");
                                self.pending_exits.insert(
                                    new_hash,
                                    PendingExit {
                                        position_hash: entry.position_hash,
                                        sent_at_ms: entry.sent_at_ms,
                                        last_sent_ms: now,
                                        tx: Some(bumped_tx),
                                    },
                                );
                                self.positions
                                    .set_exit_tx_hash(entry.position_hash, new_hash, now);
                                positions_changed = true;
                            }
                            Err(err) => {
                                warn!(?err, %tx_hash, "exit gas bump failed");
                            }
                        }
                    }
                }
                Err(err) => {
                    warn!(?err, %tx_hash, "exit receipt fetch failed");
                }
            }
        }

        if positions_changed {
            self.persist_positions();
        }

        Ok(())
    }

    async fn fallback_after_revert(&mut self, candidate_hash: B256, now: u64) -> Result<()> {
        let Some(candidate) = self.candidate_store.candidate_snapshot(candidate_hash) else {
            return Ok(());
        };
        match self.chain.http.get_transaction_receipt(candidate_hash).await {
            Ok(Some(receipt)) => {
                if !receipt.inner.status() {
                    self.candidate_store
                        .drop_terminal(candidate_hash, "addLiquidity reverted", now);
                    self.record_failure_metric("liquidity_revert");
                    return Ok(());
                }
                self.handle_mined_liquidity(candidate, receipt.block_number)
                    .await?;
            }
            Ok(None) => {
                self.enqueue_pending_liquidity(candidate, now);
            }
            Err(err) => {
                warn!(?err, "add liquidity receipt fetch failed after execution revert");
                self.enqueue_pending_liquidity(candidate, now);
            }
        }
        Ok(())
    }

    fn latest_sent_ms_for_candidate(&self, candidate_hash: B256) -> Option<u64> {
        self.pending_receipts
            .values()
            .filter(|entry| entry.candidate_hash == candidate_hash)
            .map(|entry| entry.last_sent_ms)
            .max()
    }

    fn latest_sent_ms_for_exit(&self, position_hash: B256) -> Option<u64> {
        self.pending_exits
            .values()
            .filter(|entry| entry.position_hash == position_hash)
            .map(|entry| entry.last_sent_ms)
            .max()
    }

    fn remove_pending_candidate(&mut self, candidate_hash: B256) {
        let keys: Vec<B256> = self
            .pending_receipts
            .iter()
            .filter(|(_, entry)| entry.candidate_hash == candidate_hash)
            .map(|(hash, _)| *hash)
            .collect();
        for key in keys {
            self.pending_receipts.remove(&key);
        }
    }

    fn remove_pending_exit(&mut self, position_hash: B256) {
        let keys: Vec<B256> = self
            .pending_exits
            .iter()
            .filter(|(_, entry)| entry.position_hash == position_hash)
            .map(|(hash, _)| *hash)
            .collect();
        for key in keys {
            self.pending_exits.remove(&key);
        }
    }

    async fn resolve_max_block_number(&self) -> Result<u64> {
        let delta = self.cfg.executor.max_block_number_delta;
        if delta == 0 {
            return Ok(0);
        }
        let now = now_ms();
        let current = match (self.latest_head, self.latest_head_seen_ms) {
            (Some(head), Some(seen_ms)) if now.saturating_sub(seen_ms) <= HEAD_FRESHNESS_MS => head,
            _ => self.chain.http.get_block_number().await?,
        };
        Ok(current.saturating_add(delta))
    }

    async fn sync_nonce(&self) -> Result<()> {
        if self.tx_builder.owner == Address::ZERO {
            return Ok(());
        }
        let nonce = self
            .nonce
            .sync(&self.chain.http, self.tx_builder.owner)
            .await?;
        debug!(nonce, "nonce synced");
        Ok(())
    }
}

fn is_historical_state_unavailable(reason: &str) -> bool {
    let lower = reason.to_ascii_lowercase();
    let patterns = [
        "historical state",
        "state not available",
        "state is pruned",
        "missing trie node",
        "state unavailable",
        "pruned",
        "no state for block",
        "unknown block",
        "header not found",
        "block not found",
        "missing state",
    ];
    patterns.iter().any(|pattern| lower.contains(pattern))
}

fn bump_tx_fees(tx: &mut TransactionRequest, bump_pct: u32) -> bool {
    let mut bumped = false;
    if let Some(max_fee) = tx.max_fee_per_gas {
        tx.max_fee_per_gas = Some(bump_value(max_fee, bump_pct));
        bumped = true;
    }
    if let Some(priority_fee) = tx.max_priority_fee_per_gas {
        tx.max_priority_fee_per_gas = Some(bump_value(priority_fee, bump_pct));
        bumped = true;
    }
    if let Some(gas_price) = tx.gas_price {
        tx.gas_price = Some(bump_value(gas_price, bump_pct));
        bumped = true;
    }
    bumped
}

fn bump_value(value: u128, bump_pct: u32) -> u128 {
    if bump_pct == 0 {
        return value;
    }
    let bump = value
        .saturating_mul(bump_pct as u128)
        .checked_div(100)
        .unwrap_or(0)
        .max(1);
    value.saturating_add(bump)
}

fn apply_bps(value: U256, bps: u32, increase: bool) -> U256 {
    let denom = U256::from(BPS_DENOMINATOR);
    let factor = if increase {
        U256::from(BPS_DENOMINATOR.saturating_add(bps as u64))
    } else {
        U256::from(BPS_DENOMINATOR.saturating_sub(bps as u64))
    };
    value.saturating_mul(factor) / denom
}

fn simulation_overrides(
    token: Address,
    owner: Address,
    spender: Address,
    amount_in: U256,
) -> StateOverride {
    let balance_slot = mapping_slot(owner, ERC20_BALANCES_SLOT);
    let allowance_slot = double_mapping_slot(owner, spender, ERC20_ALLOWANCES_SLOT);
    StateOverridesBuilder::default()
        .append(
            token,
            AccountOverride::default().with_state_diff([
                (balance_slot, B256::from(amount_in)),
                (allowance_slot, B256::from(U256::MAX)),
            ]),
        )
        .build()
}

fn mapping_slot(key: Address, slot: u64) -> B256 {
    let mut buf = [0u8; 64];
    buf[..32].copy_from_slice(&key.into_word().0);
    buf[32..64].copy_from_slice(&B256::from(U256::from(slot)).0);
    keccak256(buf)
}

fn double_mapping_slot(owner: Address, spender: Address, slot: u64) -> B256 {
    let inner = mapping_slot(owner, slot);
    let mut buf = [0u8; 64];
    buf[..32].copy_from_slice(&spender.into_word().0);
    buf[32..64].copy_from_slice(&inner.0);
    keccak256(buf)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::{ExitReason, Position, PositionStatus};
    use alloy::primitives::{address, Bytes, Uint, B256, U256};
    use alloy::providers::{Provider, ProviderBuilder};
    use alloy::rpc::types::TransactionReceipt;
    use alloy::rpc::types::TransactionRequest;
    use alloy::sol_types::SolCall;
    use alloy::transports::mock::Asserter;
    use sonic_chain::NodeClient;
    use sonic_core::config::{
        AppConfig,
        ChainConfig,
        DexConfig,
        ExecutorConfig,
        MempoolConfig,
        ObservabilityConfig,
        RiskConfig,
        StrategyConfig,
    };
    use sonic_core::dedupe::DedupeCache;
    use sonic_dex::abi::{IUniswapV2Pair, IUniswapV2Router02, ISolidlyRouter};
    use sonic_executor::fees::{FeeStrategy, GasMode};
    use sonic_executor::nonce::NonceManager;
    use sonic_executor::sender::TxSender;
    use sonic_core::utils::now_ms;
    use std::collections::{HashMap, HashSet};

    fn test_config(launch_gate: bool, gate_mode: &str) -> AppConfig {
        AppConfig {
            chain: ChainConfig {
                chain_id: 1,
                rpc_http: "http://localhost".to_string(),
                rpc_ws: "ws://localhost".to_string(),
            },
            mempool: MempoolConfig {
                mode: "ws".to_string(),
                txpool_poll_ms: 0,
                fetch_concurrency: 1,
                tx_fetch_timeout_ms: 0,
                dedup_capacity: 1,
                dedup_ttl_ms: 0,
                ws_reconnect_base_ms: 0,
                ws_reconnect_max_ms: 0,
            },
            dex: DexConfig {
                routers: Vec::new(),
                factories: Vec::new(),
                router_factories: Vec::new(),
                factory_pair_code_hashes: Vec::new(),
                wrapped_native: None,
                base_tokens: Vec::new(),
                pair_cache_capacity: 1,
                pair_cache_ttl_ms: 0,
                pair_cache_negative_ttl_ms: 0,
                sellability_recheck_interval_ms: 0,
                allow_execution_without_pair: false,
                launch_only_liquidity_gate: launch_gate,
                launch_only_liquidity_gate_mode: gate_mode.to_string(),
                min_base_amount: "0".to_string(),
                max_slippage_bps: 0,
                deadline_secs: 0,
            },
            risk: RiskConfig {
                sellability_amount_base: "0".to_string(),
                max_tax_bps: 0,
                erc20_call_timeout_ms: 0,
                sell_simulation_mode: "best_effort".to_string(),
                sell_simulation_override_mode: "detect".to_string(),
            },
            executor: ExecutorConfig {
                owner_private_key_env: "SNIPER_PK".to_string(),
                executor_contract: "0x0000000000000000000000000000000000000000".to_string(),
                gas_mode: "eip1559".to_string(),
                max_fee_gwei: 0,
                max_priority_gwei: 0,
                bump_pct: 0,
                bump_interval_ms: 0,
                nonce_sync_interval_ms: 0,
                receipt_poll_interval_ms: 0,
                receipt_timeout_ms: 0,
                max_block_number_delta: 0,
            },
            strategy: StrategyConfig {
                take_profit_bps: 0,
                stop_loss_bps: 0,
                max_hold_secs: 0,
                exit_signal_ttl_secs: 3600,
                emergency_reserve_drop_bps: 0,
                emergency_sell_sim_failures: 0,
                position_store_path: None,
                wait_for_mine: false,
                same_block_attempt: false,
                same_block_requires_reserves: false,
                wait_for_mine_poll_interval_ms: 0,
                wait_for_mine_timeout_ms: 0,
                candidate_cache_capacity: 1,
                candidate_ttl_ms: 0,
            },
            observability: ObservabilityConfig {
                metrics_enabled: false,
                metrics_bind: "0.0.0.0:0".to_string(),
                log_level: "info".to_string(),
                log_format: "pretty".to_string(),
            },
        }
    }

    fn build_bot(cfg: AppConfig, asserter: &Asserter, factories: Vec<Address>) -> Bot {
        let provider = ProviderBuilder::new()
            .connect_mocked_client(asserter.clone())
            .erased();
        let chain = NodeClient {
            ws: provider.clone(),
            http: provider.clone(),
        };
        let risk = RiskEngine::new(cfg.risk.clone()).unwrap();
        let tx_builder = ExecutorTxBuilder::new(
            Address::ZERO,
            Address::ZERO,
            cfg.chain.chain_id,
            FeeStrategy {
                gas_mode: GasMode::Eip1559,
                max_fee_gwei: 0,
                max_priority_gwei: 0,
            },
        );
        Bot {
            launch_gate_mode: LaunchGateMode::parse(&cfg.dex.launch_only_liquidity_gate_mode).unwrap(),
            cfg,
            chain,
            routers: HashSet::new(),
            router_factories: HashMap::new(),
            router_meta: HashMap::new(),
            factories,
            base_tokens: HashSet::new(),
            base_token_list: Vec::new(),
            wrapped_native: None,
            risk,
            pair_cache: PairMetadataCache::new(1, 0, 0, HashMap::new()),
            dedupe: DedupeCache::new(1, 0),
            metrics: None,
            tx_builder,
            nonce: NonceManager::new(0),
            sender: TxSender::new(provider),
            min_base_amount: U256::ZERO,
            pending_liquidity: HashMap::new(),
            pending_receipts: HashMap::new(),
            pending_exits: HashMap::new(),
            sell_sim_failures: HashMap::new(),
            latest_head: None,
            latest_head_seen_ms: None,
            counters: CounterSummary::new(0),
            candidate_store: CandidateStore::new(1, 0),
            positions: PositionStore::new(),
            position_store_path: None,
        }
    }

    fn candidate_with_pair(pair: Option<Address>) -> LiquidityCandidate {
        LiquidityCandidate {
            token: address!("0x3000000000000000000000000000000000000003"),
            base: address!("0x2000000000000000000000000000000000000002"),
            router: address!("0x1000000000000000000000000000000000000001"),
            factory: None,
            pair,
            stable: None,
            add_liq_tx_hash: B256::ZERO,
            first_seen_ms: 0,
            implied_liquidity: U256::from(1000u64),
        }
    }

    fn sample_position(entry_price: Option<U256>, opened_ms: u64) -> Position {
        Position {
            add_liq_tx_hash: B256::from([9u8; 32]),
            entry_tx_hash: B256::from([8u8; 32]),
            router: address!("0x1000000000000000000000000000000000000001"),
            token: address!("0x3000000000000000000000000000000000000003"),
            base: address!("0x2000000000000000000000000000000000000002"),
            pricing_base: address!("0x2000000000000000000000000000000000000002"),
            pair: Some(address!("0x4000000000000000000000000000000000000004")),
            stable: None,
            entry_base_amount: U256::from(1_000u64),
            entry_token_amount: Some(U256::from(10u64)),
            entry_price_base_per_token: entry_price,
            entry_base_reserve: None,
            entry_token_reserve: None,
            entry_block: Some(1),
            opened_ms,
            last_update_ms: opened_ms,
            status: PositionStatus::Open,
            exit_tx_hash: None,
        }
    }

    #[tokio::test]
    async fn launch_gate_allows_when_pair_missing_prior() {
        let asserter = Asserter::new();
        asserter.push_success(&100u64);
        asserter.push_success(&Bytes::from(Vec::<u8>::new()));

        let cfg = test_config(true, "strict");
        let mut bot = build_bot(cfg, &asserter, Vec::new());
        let candidate = candidate_with_pair(Some(address!("0x4000000000000000000000000000000000000004")));

        let decision = bot
            .launch_only_liquidity_gate(&candidate, candidate.base, None)
            .await
            .unwrap();
        assert!(matches!(decision, LaunchGateDecision::Allow));
        assert!(asserter.read_q().is_empty());
    }

    #[tokio::test]
    async fn launch_gate_rejects_when_prior_reserves_nonzero() {
        let asserter = Asserter::new();
        asserter.push_success(&100u64);
        asserter.push_success(&Bytes::from(vec![1u8]));

        type U112 = Uint<112, 2>;
        type ReservesReturn = <IUniswapV2Pair::getReservesCall as SolCall>::Return;
        let reserves = ReservesReturn {
            reserve0: U112::from(1_000u64),
            reserve1: U112::from(2_000u64),
            blockTimestampLast: 1u32,
        };
        let encoded = IUniswapV2Pair::getReservesCall::abi_encode_returns(&reserves);
        asserter.push_success(&Bytes::from(encoded));

        let cfg = test_config(true, "strict");
        let mut bot = build_bot(cfg, &asserter, Vec::new());
        let candidate = candidate_with_pair(Some(address!("0x4000000000000000000000000000000000000004")));

        let decision = bot
            .launch_only_liquidity_gate(&candidate, candidate.base, None)
            .await
            .unwrap();
        match decision {
            LaunchGateDecision::Reject { reason, kind } => {
                assert_eq!(kind, DropKind::Terminal);
                assert!(reason.contains("pair had liquidity in prior block"));
            }
            _ => panic!("expected launch gate rejection"),
        }
        assert!(asserter.read_q().is_empty());
    }

    #[tokio::test]
    async fn launch_gate_allows_when_prior_reserves_zero() {
        let asserter = Asserter::new();
        asserter.push_success(&100u64);
        asserter.push_success(&Bytes::from(vec![1u8]));

        type U112 = Uint<112, 2>;
        type ReservesReturn = <IUniswapV2Pair::getReservesCall as SolCall>::Return;
        let reserves = ReservesReturn {
            reserve0: U112::from(0u64),
            reserve1: U112::from(0u64),
            blockTimestampLast: 1u32,
        };
        let encoded = IUniswapV2Pair::getReservesCall::abi_encode_returns(&reserves);
        asserter.push_success(&Bytes::from(encoded));

        let cfg = test_config(true, "strict");
        let mut bot = build_bot(cfg, &asserter, Vec::new());
        let candidate = candidate_with_pair(Some(address!("0x4000000000000000000000000000000000000004")));

        let decision = bot
            .launch_only_liquidity_gate(&candidate, candidate.base, None)
            .await
            .unwrap();
        assert!(matches!(decision, LaunchGateDecision::Allow));
        assert!(asserter.read_q().is_empty());
    }

    #[tokio::test]
    async fn launch_gate_allows_when_pair_unresolved_in_best_effort() {
        let asserter = Asserter::new();
        let cfg = test_config(true, "best_effort");
        let mut bot = build_bot(cfg, &asserter, Vec::new());
        let candidate = candidate_with_pair(None);

        let decision = bot
            .launch_only_liquidity_gate(&candidate, candidate.base, None)
            .await
            .unwrap();
        assert!(matches!(decision, LaunchGateDecision::Allow));
        assert!(asserter.read_q().is_empty());
    }

    #[tokio::test]
    async fn launch_gate_rejects_when_pair_unresolved_in_strict() {
        let asserter = Asserter::new();
        let cfg = test_config(true, "strict");
        let mut bot = build_bot(cfg, &asserter, Vec::new());
        let candidate = candidate_with_pair(None);

        let decision = bot
            .launch_only_liquidity_gate(&candidate, candidate.base, None)
            .await
            .unwrap();
        match decision {
            LaunchGateDecision::Reject { reason, kind } => {
                assert_eq!(kind, DropKind::Transient);
                assert!(reason.contains("launch gate requires pair address"));
            }
            _ => panic!("expected launch gate rejection"),
        }
        assert!(asserter.read_q().is_empty());
    }

    #[tokio::test]
    async fn launch_gate_allows_on_historical_state_error_in_best_effort() {
        let asserter = Asserter::new();
        asserter.push_success(&100u64);
        asserter.push_failure_msg("missing trie node");

        let cfg = test_config(true, "best_effort");
        let mut bot = build_bot(cfg, &asserter, Vec::new());
        let candidate = candidate_with_pair(Some(address!("0x4000000000000000000000000000000000000004")));

        let decision = bot
            .launch_only_liquidity_gate(&candidate, candidate.base, None)
            .await
            .unwrap();
        assert!(matches!(decision, LaunchGateDecision::Allow));
        assert!(asserter.read_q().is_empty());
    }

    #[tokio::test]
    async fn launch_gate_rejects_on_historical_state_error_in_strict() {
        let asserter = Asserter::new();
        asserter.push_success(&100u64);
        asserter.push_failure_msg("missing trie node");

        let cfg = test_config(true, "strict");
        let mut bot = build_bot(cfg, &asserter, Vec::new());
        let candidate = candidate_with_pair(Some(address!("0x4000000000000000000000000000000000000004")));

        let decision = bot
            .launch_only_liquidity_gate(&candidate, candidate.base, None)
            .await
            .unwrap();
        match decision {
            LaunchGateDecision::Reject { reason, kind } => {
                assert_eq!(kind, DropKind::Transient);
                assert!(reason.contains("launch gate historical state unavailable"));
            }
            _ => panic!("expected launch gate rejection"),
        }
        assert!(asserter.read_q().is_empty());
    }

    #[tokio::test]
    async fn poll_receipts_keeps_pending_when_missing() {
        let asserter = Asserter::new();
        let cfg = test_config(true, "strict");
        let mut bot = build_bot(cfg, &asserter, Vec::new());
        let tx_hash = B256::from([1u8; 32]);
        bot.pending_receipts.insert(
            tx_hash,
            PendingReceipt {
                candidate_hash: B256::ZERO,
                sent_at_ms: now_ms(),
                last_sent_ms: now_ms(),
                tx: TransactionRequest::default(),
            },
        );

        let none: Option<TransactionReceipt> = None;
        asserter.push_success(&none);

        bot.poll_receipts().await.unwrap();
        assert!(bot.pending_receipts.contains_key(&tx_hash));
        assert!(asserter.read_q().is_empty());
    }

    #[tokio::test]
    async fn poll_receipts_drops_after_timeout() {
        let asserter = Asserter::new();
        let mut cfg = test_config(true, "strict");
        cfg.executor.receipt_timeout_ms = 1;
        let mut bot = build_bot(cfg, &asserter, Vec::new());
        let tx_hash = B256::from([2u8; 32]);
        bot.pending_receipts.insert(
            tx_hash,
            PendingReceipt {
                candidate_hash: B256::ZERO,
                sent_at_ms: 0,
                last_sent_ms: 0,
                tx: TransactionRequest::default(),
            },
        );

        bot.poll_receipts().await.unwrap();
        assert!(bot.pending_receipts.is_empty());
        assert!(asserter.read_q().is_empty());
    }

    #[tokio::test]
    async fn poll_liquidity_receipts_keeps_pending_when_missing() {
        let asserter = Asserter::new();
        let cfg = test_config(true, "strict");
        let mut bot = build_bot(cfg, &asserter, Vec::new());
        let tx_hash = B256::from([3u8; 32]);
        let candidate = candidate_with_pair(Some(address!("0x4000000000000000000000000000000000000004")));
        bot.candidate_store.track_detected(candidate.clone(), now_ms());
        bot.pending_liquidity.insert(
            tx_hash,
            PendingLiquidity {
                candidate,
                enqueued_ms: now_ms(),
            },
        );

        let none: Option<TransactionReceipt> = None;
        asserter.push_success(&none);

        bot.poll_liquidity_receipts().await.unwrap();
        assert!(bot.pending_liquidity.contains_key(&tx_hash));
        assert!(asserter.read_q().is_empty());
    }

    #[tokio::test]
    async fn poll_liquidity_receipts_drops_after_timeout() {
        let asserter = Asserter::new();
        let mut cfg = test_config(true, "strict");
        cfg.strategy.wait_for_mine_timeout_ms = 1;
        let mut bot = build_bot(cfg, &asserter, Vec::new());
        let tx_hash = B256::from([4u8; 32]);
        let candidate = candidate_with_pair(Some(address!("0x4000000000000000000000000000000000000004")));
        bot.candidate_store.track_detected(candidate.clone(), 0);
        bot.pending_liquidity.insert(
            tx_hash,
            PendingLiquidity {
                candidate,
                enqueued_ms: 0,
            },
        );

        bot.poll_liquidity_receipts().await.unwrap();
        assert!(bot.pending_liquidity.is_empty());
        assert!(asserter.read_q().is_empty());
    }

    #[tokio::test]
    async fn build_position_sets_entry_quote_from_router() {
        let asserter = Asserter::new();
        let cfg = test_config(true, "strict");
        let bot = build_bot(cfg, &asserter, Vec::new());
        let mut candidate = candidate_with_pair(None);
        candidate.implied_liquidity = U256::from(1_000u64);

        let amounts = vec![U256::from(1_000u64), U256::from(100u64)];
        asserter.push_success(&Bytes::from(
            IUniswapV2Router02::swapExactTokensForTokensCall::abi_encode_returns(&amounts),
        ));

        let position = bot
            .build_position(candidate, B256::ZERO, Some(12), now_ms())
            .await
            .unwrap();
        assert_eq!(position.entry_token_amount, Some(U256::from(100u64)));
        let expected_price =
            U256::from(1_000u64) * U256::from(PRICE_SCALE) / U256::from(100u64);
        assert_eq!(position.entry_price_base_per_token, Some(expected_price));
        assert!(asserter.read_q().is_empty());
    }

    #[tokio::test]
    async fn current_price_uses_v2_router_quote() {
        let asserter = Asserter::new();
        let cfg = test_config(true, "strict");
        let bot = build_bot(cfg, &asserter, Vec::new());
        let mut position = sample_position(Some(U256::from(1u64)), 0);
        position.entry_token_amount = Some(U256::from(100u64));

        let amounts = vec![U256::from(100u64), U256::from(150u64)];
        asserter.push_success(&Bytes::from(
            IUniswapV2Router02::swapExactTokensForTokensCall::abi_encode_returns(&amounts),
        ));

        let price = bot
            .current_price_for_position(&position)
            .await
            .unwrap()
            .expect("price");
        let expected_price =
            U256::from(150u64) * U256::from(PRICE_SCALE) / U256::from(100u64);
        assert_eq!(price, expected_price);
        assert!(asserter.read_q().is_empty());
    }

    #[tokio::test]
    async fn current_price_uses_solidly_router_quote() {
        let asserter = Asserter::new();
        let cfg = test_config(true, "strict");
        let bot = build_bot(cfg, &asserter, Vec::new());
        let mut position = sample_position(Some(U256::from(1u64)), 0);
        position.entry_token_amount = Some(U256::from(100u64));
        position.stable = Some(true);

        let amounts = vec![U256::from(100u64), U256::from(140u64)];
        asserter.push_success(&Bytes::from(
            ISolidlyRouter::swapExactTokensForTokensCall::abi_encode_returns(&amounts),
        ));

        let price = bot
            .current_price_for_position(&position)
            .await
            .unwrap()
            .expect("price");
        let expected_price =
            U256::from(140u64) * U256::from(PRICE_SCALE) / U256::from(100u64);
        assert_eq!(price, expected_price);
        assert!(asserter.read_q().is_empty());
    }

    #[test]
    fn exit_reason_triggers_take_profit() {
        let asserter = Asserter::new();
        let mut cfg = test_config(true, "strict");
        cfg.strategy.take_profit_bps = 1_000;
        let bot = build_bot(cfg, &asserter, Vec::new());

        let entry_price = U256::from(PRICE_SCALE) * U256::from(100u64);
        let current_price = entry_price * U256::from(11u64) / U256::from(10u64);
        let position = sample_position(Some(entry_price), 0);

        let reason = bot.evaluate_exit_reason(&position, 0, Some(current_price));
        assert_eq!(reason, Some(ExitReason::TakeProfit));
        assert!(asserter.read_q().is_empty());
    }

    #[test]
    fn exit_reason_triggers_stop_loss() {
        let asserter = Asserter::new();
        let mut cfg = test_config(true, "strict");
        cfg.strategy.stop_loss_bps = 500;
        let bot = build_bot(cfg, &asserter, Vec::new());

        let entry_price = U256::from(PRICE_SCALE) * U256::from(100u64);
        let current_price = entry_price * U256::from(94u64) / U256::from(100u64);
        let position = sample_position(Some(entry_price), 0);

        let reason = bot.evaluate_exit_reason(&position, 0, Some(current_price));
        assert_eq!(reason, Some(ExitReason::StopLoss));
        assert!(asserter.read_q().is_empty());
    }

    #[test]
    fn exit_reason_triggers_max_hold() {
        let asserter = Asserter::new();
        let mut cfg = test_config(true, "strict");
        cfg.strategy.max_hold_secs = 1;
        let bot = build_bot(cfg, &asserter, Vec::new());

        let position = sample_position(None, 0);
        let reason = bot.evaluate_exit_reason(&position, 2_000, None);
        assert_eq!(reason, Some(ExitReason::MaxHold));
        assert!(asserter.read_q().is_empty());
    }

    #[tokio::test]
    async fn exit_min_amount_out_applies_slippage() {
        let asserter = Asserter::new();
        let mut cfg = test_config(true, "strict");
        cfg.dex.max_slippage_bps = 500;
        let bot = build_bot(cfg, &asserter, Vec::new());

        let position = sample_position(Some(U256::from(1u64)), 0);
        let amount_in = U256::from(100u64);
        let current_price = U256::from(2u64) * U256::from(PRICE_SCALE);

        let min_out = bot
            .exit_min_amount_out(&position, amount_in, Some(current_price))
            .await
            .unwrap();
        assert_eq!(min_out, U256::from(190u64));
        assert!(asserter.read_q().is_empty());
    }

    #[tokio::test]
    async fn exit_strict_skips_on_zero_quote_and_touches() {
        let asserter = Asserter::new();
        let mut cfg = test_config(true, "strict");
        cfg.dex.max_slippage_bps = 0;
        cfg.strategy.exit_signal_ttl_secs = 10;
        let mut bot = build_bot(cfg, &asserter, Vec::new());
        bot.tx_builder.owner = address!("0xaaaa00000000000000000000000000000000aaaa");

        let mut position = sample_position(Some(U256::from(1u64)), 0);
        let decided_ms = now_ms();
        position.status = PositionStatus::ExitSignaled {
            reason: ExitReason::StopLoss,
            decided_ms,
        };
        position.last_update_ms = 0;
        let position_hash = position.add_liq_tx_hash;
        bot.positions.insert(position);

        let amount_in = U256::from(10u64);
        asserter.push_success(&Bytes::from(
            IERC20::balanceOfCall::abi_encode_returns(&amount_in),
        ));
        let amounts = vec![amount_in, U256::from(0u64)];
        asserter.push_success(&Bytes::from(
            IUniswapV2Router02::swapExactTokensForTokensCall::abi_encode_returns(&amounts),
        ));

        bot.execute_exit_for_position(position_hash, ExitReason::StopLoss, None)
            .await
            .unwrap();

        let updated = bot.positions.get(position_hash).expect("position");
        assert!(updated.last_update_ms >= decided_ms);
        assert!(updated.exit_tx_hash.is_none());
        assert!(bot.pending_exits.is_empty());
        assert!(asserter.read_q().is_empty());
    }

    #[tokio::test]
    async fn exit_amount_for_position_errors_on_balance_failure() {
        let asserter = Asserter::new();
        let cfg = test_config(true, "strict");
        let mut bot = build_bot(cfg, &asserter, Vec::new());
        bot.tx_builder.owner = address!("0xaaaa00000000000000000000000000000000aaaa");

        let position = sample_position(Some(U256::from(1u64)), 0);
        asserter.push_failure_msg("balanceOf failed");

        let err = bot.exit_amount_for_position(&position).await.unwrap_err();
        assert!(err.to_string().contains("balanceOf failed"));
        assert!(asserter.read_q().is_empty());
    }

    #[tokio::test]
    async fn exit_amount_for_position_uses_balance_floor() {
        let asserter = Asserter::new();
        let cfg = test_config(true, "strict");
        let mut bot = build_bot(cfg, &asserter, Vec::new());
        bot.tx_builder.owner = address!("0xaaaa00000000000000000000000000000000aaaa");

        let position = sample_position(Some(U256::from(1u64)), 0);
        asserter.push_success(&Bytes::from(
            IERC20::balanceOfCall::abi_encode_returns(&U256::from(5u64)),
        ));

        let amount = bot
            .exit_amount_for_position(&position)
            .await
            .unwrap()
            .expect("amount");
        assert_eq!(amount, U256::from(5u64));
        assert!(asserter.read_q().is_empty());
    }

    #[test]
    fn exit_loop_enabled_with_emergency_triggers() {
        let asserter = Asserter::new();
        let mut cfg = test_config(false, "strict");
        cfg.strategy.emergency_reserve_drop_bps = 2500;
        let bot = build_bot(cfg, &asserter, Vec::new());
        assert!(bot.exit_loop_enabled());

        let asserter = Asserter::new();
        let mut cfg = test_config(false, "strict");
        cfg.strategy.emergency_sell_sim_failures = 2;
        let bot = build_bot(cfg, &asserter, Vec::new());
        assert!(bot.exit_loop_enabled());
    }

    #[test]
    fn bump_tx_fees_increases_gas_fields() {
        let mut tx = TransactionRequest::default();
        tx.max_fee_per_gas = Some(1_000u128);
        tx.max_priority_fee_per_gas = Some(100u128);
        assert!(bump_tx_fees(&mut tx, 10));
        assert_eq!(tx.max_fee_per_gas, Some(1_100u128));
        assert_eq!(tx.max_priority_fee_per_gas, Some(110u128));

        let mut legacy = TransactionRequest::default();
        legacy.gas_price = Some(1_000u128);
        assert!(bump_tx_fees(&mut legacy, 20));
        assert_eq!(legacy.gas_price, Some(1_200u128));
    }

    #[tokio::test]
    async fn resolve_max_block_number_uses_latest_head() {
        let asserter = Asserter::new();
        let mut cfg = test_config(true, "strict");
        cfg.executor.max_block_number_delta = 2;
        let mut bot = build_bot(cfg, &asserter, Vec::new());
        bot.latest_head = Some(10);
        bot.latest_head_seen_ms = Some(now_ms());

        let max_block = bot.resolve_max_block_number().await.unwrap();
        assert_eq!(max_block, 12);
        assert!(asserter.read_q().is_empty());
    }

    #[tokio::test]
    async fn resolve_max_block_number_fetches_when_head_stale() {
        let asserter = Asserter::new();
        let mut cfg = test_config(true, "strict");
        cfg.executor.max_block_number_delta = 1;
        let mut bot = build_bot(cfg, &asserter, Vec::new());
        bot.latest_head = Some(10);
        bot.latest_head_seen_ms = Some(0);
        asserter.push_success(&100u64);

        let max_block = bot.resolve_max_block_number().await.unwrap();
        assert_eq!(max_block, 101);
        assert!(asserter.read_q().is_empty());
    }

    #[tokio::test]
    async fn resolve_max_block_number_fetches_when_head_missing() {
        let asserter = Asserter::new();
        let mut cfg = test_config(true, "strict");
        cfg.executor.max_block_number_delta = 1;
        let bot = build_bot(cfg, &asserter, Vec::new());
        asserter.push_success(&100u64);

        let max_block = bot.resolve_max_block_number().await.unwrap();
        assert_eq!(max_block, 101);
        assert!(asserter.read_q().is_empty());
    }
}
