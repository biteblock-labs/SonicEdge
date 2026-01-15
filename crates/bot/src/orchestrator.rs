use crate::metrics::{spawn_metrics_server, BotMetrics};
use alloy::primitives::{Address, B256, U256};
use anyhow::Result;
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
use sonic_dex::{decode_router_calldata, PairMetadataCache, RouterCall};
use sonic_executor::fees::{FeeStrategy, GasMode};
use sonic_executor::{nonce::NonceManager, sender::TxSender};
use sonic_executor::{BuySolidlyParams, BuyV2Params, ExecutorTxBuilder};
use sonic_risk::{RiskContext, RiskEngine};
use std::collections::{HashMap, HashSet};
use std::str::FromStr;
use std::sync::Arc;
use tokio::select;
use tracing::{debug, info, warn};

pub struct Bot {
    cfg: AppConfig,
    chain: NodeClient,
    routers: HashSet<Address>,
    router_factories: HashMap<Address, Address>,
    factories: Vec<Address>,
    base_tokens: HashSet<Address>,
    risk: RiskEngine,
    pair_cache: PairMetadataCache,
    dedupe: DedupeCache<B256>,
    metrics: Option<Arc<BotMetrics>>,
    tx_builder: ExecutorTxBuilder,
    nonce: NonceManager,
    sender: TxSender,
    min_base_amount: U256,
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
        let risk = RiskEngine::new(cfg.risk.clone());
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

        Ok(Self {
            cfg,
            chain,
            routers,
            router_factories,
            factories,
            base_tokens,
            risk,
            pair_cache,
            dedupe,
            metrics,
            tx_builder,
            nonce,
            sender,
            min_base_amount,
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

        info!("bot running");
        loop {
            select! {
                Some(hash) = pending_rx.recv() => {
                    self.handle_hash(&fetcher, hash).await?;
                }
                Some(hash) = async {
                    match txpool_rx.as_mut() {
                        Some(rx) => rx.recv().await,
                        None => None,
                    }
                } => {
                    self.handle_hash(&fetcher, hash).await?;
                }
                Some(head) = heads_rx.recv() => {
                    debug!(block = head, "new head");
                }
            }
        }
    }

    async fn handle_hash(&mut self, fetcher: &TxFetcher, hash: B256) -> Result<()> {
        let now = now_ms();
        if !self.dedupe.check_and_update(hash, now) {
            if let Some(metrics) = &self.metrics {
                metrics.dedup_hits.inc();
            }
            return Ok(());
        }

        let tx = match fetcher.fetch(hash).await? {
            Some(tx) => tx,
            None => return Ok(()),
        };

        if let Some(mut candidate) = self.detect_liquidity_add(&tx)? {
            self.resolve_candidate_pair(&mut candidate).await?;
            if candidate.base == Address::ZERO {
                warn!("native base token not supported in v1 executor");
                return Ok(());
            }
            if candidate.pair.is_none() && !self.cfg.dex.allow_execution_without_pair {
                warn!(
                    token = %candidate.token,
                    base = %candidate.base,
                    router = %candidate.router,
                    "pair unresolved; skipping execution"
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
                base_token: candidate.base,
                token: candidate.token,
            };
            let decision = self.risk.assess(&ctx).await?;
            if !decision.pass {
                debug!(reasons = ?decision.reasons, "risk reject");
                return Ok(());
            }
            self.execute_candidate(candidate).await?;
        }

        Ok(())
    }

    fn detect_liquidity_add(&self, tx: &MempoolTx) -> Result<Option<LiquidityCandidate>> {
        let Some(to) = tx.to else {
            return Ok(None);
        };
        if !self.routers.contains(&to) {
            return Ok(None);
        }

        let call = match decode_router_calldata(&tx.input)? {
            Some(call) => call,
            None => return Ok(None),
        };

        match call {
            RouterCall::AddLiquidity(add) => {
                let (base, token, base_amount) = if self.base_tokens.contains(&add.token_a) {
                    (add.token_a, add.token_b, add.amount_a_desired)
                } else if self.base_tokens.contains(&add.token_b) {
                    (add.token_b, add.token_a, add.amount_b_desired)
                } else {
                    return Ok(None);
                };

                if base_amount < self.min_base_amount {
                    return Ok(None);
                }

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
                    return Ok(None);
                }
                if add.amount_eth_min < self.min_base_amount {
                    return Ok(None);
                }

                Ok(Some(LiquidityCandidate {
                    token: add.token,
                    base,
                    router: to,
                    factory: self.router_factories.get(&to).copied(),
                    pair: None,
                    stable: None,
                    add_liq_tx_hash: tx.hash,
                    first_seen_ms: tx.first_seen_ms,
                    implied_liquidity: add.amount_eth_min,
                }))
            }
            RouterCall::AddLiquiditySolidly(add) => {
                let (base, token, base_amount) = if self.base_tokens.contains(&add.token_a) {
                    (add.token_a, add.token_b, add.amount_a_desired)
                } else if self.base_tokens.contains(&add.token_b) {
                    (add.token_b, add.token_a, add.amount_b_desired)
                } else {
                    return Ok(None);
                };

                if base_amount < self.min_base_amount {
                    return Ok(None);
                }

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
        }
    }

    async fn resolve_candidate_pair(&mut self, candidate: &mut LiquidityCandidate) -> Result<()> {
        if candidate.base == Address::ZERO || candidate.pair.is_some() {
            return Ok(());
        }

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
                candidate.base,
                candidate.token,
                candidate.stable,
            )
            .await?
        {
            candidate.pair = Some(metadata.pair);
            candidate.factory = Some(metadata.factory);
        }

        Ok(())
    }

    async fn execute_candidate(&self, candidate: LiquidityCandidate) -> Result<()> {
        if self.tx_builder.owner == Address::ZERO {
            warn!("owner key not configured; skipping execution");
            return Ok(());
        }

        if candidate.base == Address::ZERO {
            warn!("native base token not supported in v1 executor");
            return Ok(());
        }

        let nonce = self.nonce.next_nonce();
        let deadline = U256::from(now_ms() / 1000 + self.cfg.dex.deadline_secs as u64);

        let tx = if let Some(stable) = candidate.stable {
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
                max_block_number: 0,
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
                max_block_number: 0,
            };
            self.tx_builder.build_buy_v2(params, nonce)
        };
        let hash = self.sender.send(tx).await?;
        info!(%hash, "buy tx sent");
        Ok(())
    }
}
