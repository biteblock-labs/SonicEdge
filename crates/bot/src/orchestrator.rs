use crate::metrics::{spawn_metrics_server, BotMetrics};
use crate::state::{BotState, CandidateStore, DropKind};
use alloy::primitives::{Address, B256, U256};
use alloy::rpc::types::TransactionRequest;
use alloy::providers::Provider;
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
use sonic_dex::pair::{contract_exists_at_block, get_reserves_at_block};
use sonic_dex::{decode_router_calldata, PairMetadataCache, RouterCall};
use sonic_executor::fees::{FeeStrategy, GasMode};
use sonic_executor::{nonce::NonceManager, sender::TxSender};
use sonic_executor::{
    BuySolidlyEthParams,
    BuySolidlyParams,
    BuyV2EthParams,
    BuyV2Params,
    ExecutorTxBuilder,
};
use sonic_risk::{RiskContext, RiskEngine};
use std::collections::{HashMap, HashSet};
use std::str::FromStr;
use std::sync::Arc;
use tokio::select;
use tokio::time::Duration;
use tracing::{debug, info, warn};

const SUMMARY_INTERVAL_MS: u64 = 30_000;
const HEAD_FRESHNESS_MS: u64 = 10_000;

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
    factories: Vec<Address>,
    base_tokens: HashSet<Address>,
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
    pending_receipts: HashMap<B256, PendingReceipt>,
    latest_head: Option<u64>,
    latest_head_seen_ms: Option<u64>,
    counters: CounterSummary,
    candidate_store: CandidateStore,
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
        let base_tokens = cfg
            .dex
            .base_tokens
            .iter()
            .map(|s| parse_address(s))
            .collect::<Result<HashSet<_>>>()?;
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
        let counters = CounterSummary::new(now_ms());
        let candidate_store = CandidateStore::new(
            cfg.strategy.candidate_cache_capacity,
            cfg.strategy.candidate_ttl_ms,
        );

        Ok(Self {
            cfg,
            chain,
            routers,
            router_factories,
            factories,
            base_tokens,
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
            pending_receipts: HashMap::new(),
            latest_head: None,
            latest_head_seen_ms: None,
            counters,
            candidate_store,
        })
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

        info!("bot running");
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
            {
                return Ok(());
            }
            self.candidate_store
                .track_detected(candidate.clone(), now_ms());
            self.resolve_candidate_pair(&mut candidate).await?;
            self.candidate_store
                .set_state(candidate.clone(), BotState::Qualifying, now_ms());
            if candidate.pair.is_some() {
                self.counters.totals.pair_resolved =
                    self.counters.totals.pair_resolved.saturating_add(1);
            } else {
                self.counters.totals.pair_unresolved =
                    self.counters.totals.pair_unresolved.saturating_add(1);
            }
            let risk_base = if candidate.base == Address::ZERO {
                match self.wrapped_native {
                    Some(addr) => addr,
                    None => {
                        warn!("wrapped_native not configured; skipping native execution");
                        self.candidate_store.drop_terminal(
                            candidate.add_liq_tx_hash,
                            "wrapped_native not configured",
                            now_ms(),
                        );
                        return Ok(());
                    }
                }
            } else {
                candidate.base
            };
            match self
                .launch_only_liquidity_gate(&candidate, risk_base)
                .await?
            {
                LaunchGateDecision::Allow => {}
                LaunchGateDecision::Reject { reason, kind } => {
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
                    match kind {
                        DropKind::Transient => {
                            self.candidate_store
                                .drop_transient(candidate.add_liq_tx_hash, reason, now_ms());
                        }
                        DropKind::Terminal => {
                            self.candidate_store
                                .drop_terminal(candidate.add_liq_tx_hash, reason, now_ms());
                        }
                    }
                    return Ok(());
                }
            }
            if candidate.pair.is_none() && !self.cfg.dex.allow_execution_without_pair {
                warn!(
                    token = %candidate.token,
                    base = %candidate.base,
                    router = %candidate.router,
                    "pair unresolved; skipping execution"
                );
                self.candidate_store.drop_transient(
                    candidate.add_liq_tx_hash,
                    "pair unresolved",
                    now_ms(),
                );
                return Ok(());
            }
            if candidate.pair.is_none() && self.cfg.dex.allow_execution_without_pair {
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
                "liquidity candidate detected"
            );
            let ctx = RiskContext {
                provider: &self.chain.http,
                router: candidate.router,
                base_token: risk_base,
                token: candidate.token,
                pair: candidate.pair,
            };
            let decision = self.risk.assess(&ctx).await?;
            if !decision.pass {
                self.counters.totals.risk_fail =
                    self.counters.totals.risk_fail.saturating_add(1);
                warn!(score = decision.score, reasons = ?decision.reasons, "risk reject");
                let reason = if decision.reasons.is_empty() {
                    "risk rejected".to_string()
                } else {
                    format!("risk rejected: {}", decision.reasons.join("; "))
                };
                self.candidate_store
                    .drop_terminal(candidate.add_liq_tx_hash, reason, now_ms());
                return Ok(());
            }
            self.counters.totals.risk_pass =
                self.counters.totals.risk_pass.saturating_add(1);
            self.candidate_store
                .set_state(candidate.clone(), BotState::Executing, now_ms());
            let candidate_hash = candidate.add_liq_tx_hash;
            match self.execute_candidate(candidate).await? {
                ExecutionOutcome::Sent { hash, tx } => {
                    self.candidate_store
                        .mark_executed(candidate_hash, hash, now_ms());
                    self.counters.totals.executed =
                        self.counters.totals.executed.saturating_add(1);
                    let now = now_ms();
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
                    match kind {
                        DropKind::Transient => {
                            self.candidate_store.drop_transient(candidate_hash, reason, now_ms());
                        }
                        DropKind::Terminal => {
                            self.candidate_store.drop_terminal(candidate_hash, reason, now_ms());
                        }
                    }
                    return Ok(());
                }
            }
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
                    } else {
                        warn!(%tx_hash, block, "tx reverted");
                        self.candidate_store.drop_terminal(
                            entry.candidate_hash,
                            "execution reverted",
                            now,
                        );
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

    fn latest_sent_ms_for_candidate(&self, candidate_hash: B256) -> Option<u64> {
        self.pending_receipts
            .values()
            .filter(|entry| entry.candidate_hash == candidate_hash)
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

#[cfg(test)]
mod tests {
    use super::*;
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
    use sonic_dex::abi::IUniswapV2Pair;
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
                candidate_cache_capacity: 1,
                candidate_ttl_ms: 0,
            },
            observability: ObservabilityConfig {
                metrics_enabled: false,
                metrics_bind: "0.0.0.0:0".to_string(),
                log_level: "info".to_string(),
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
            factories,
            base_tokens: HashSet::new(),
            wrapped_native: None,
            risk,
            pair_cache: PairMetadataCache::new(1, 0, 0, HashMap::new()),
            dedupe: DedupeCache::new(1, 0),
            metrics: None,
            tx_builder,
            nonce: NonceManager::new(0),
            sender: TxSender::new(provider),
            min_base_amount: U256::ZERO,
            pending_receipts: HashMap::new(),
            latest_head: None,
            latest_head_seen_ms: None,
            counters: CounterSummary::new(0),
            candidate_store: CandidateStore::new(1, 0),
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

    #[tokio::test]
    async fn launch_gate_allows_when_pair_missing_prior() {
        let asserter = Asserter::new();
        asserter.push_success(&100u64);
        asserter.push_success(&Bytes::from(Vec::<u8>::new()));

        let cfg = test_config(true, "strict");
        let mut bot = build_bot(cfg, &asserter, Vec::new());
        let candidate = candidate_with_pair(Some(address!("0x4000000000000000000000000000000000000004")));

        let decision = bot
            .launch_only_liquidity_gate(&candidate, candidate.base)
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
            .launch_only_liquidity_gate(&candidate, candidate.base)
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
            .launch_only_liquidity_gate(&candidate, candidate.base)
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
            .launch_only_liquidity_gate(&candidate, candidate.base)
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
            .launch_only_liquidity_gate(&candidate, candidate.base)
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
            .launch_only_liquidity_gate(&candidate, candidate.base)
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
            .launch_only_liquidity_gate(&candidate, candidate.base)
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
