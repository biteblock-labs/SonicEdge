use alloy::primitives::{address, keccak256, Address, B256, TxKind, U256};
use alloy::providers::{DynProvider, Provider};
use alloy::rpc::types::state::{AccountOverride, StateOverride, StateOverridesBuilder};
use alloy::rpc::types::transaction::TransactionInput;
use alloy::rpc::types::TransactionRequest;
use alloy::sol_types::SolCall;
use anyhow::{bail, Result};
use sonic_core::config::RiskConfig;
use sonic_core::utils::parse_u256_decimal;
use std::fmt::Display;
use std::future::Future;
use std::time::Duration;
use tokio::time::timeout;
use tracing::{debug, warn};

use crate::abi::{IERC20, IUniswapV2Router02};
use crate::types::RiskDecision;

struct RiskFinding {
    reason: String,
    score: u32,
}

impl RiskFinding {
    fn new(reason: impl Into<String>, score: u32) -> Self {
        Self {
            reason: reason.into(),
            score,
        }
    }
}

const SCORE_FATAL: u32 = 100;
const SCORE_HIGH: u32 = 50;
const SCORE_MEDIUM: u32 = 25;
const ERC20_BALANCES_SLOT: u64 = 0;
const ERC20_ALLOWANCES_SLOT: u64 = 1;
const SIMULATION_DEADLINE_SECS: u64 = 10_000_000_000;
const SIMULATION_SENDER: Address = address!("0x1111111111111111111111111111111111111111");

#[derive(Clone)]
pub struct RiskEngine {
    cfg: RiskConfig,
    sellability_amount_base: U256,
    call_timeout: Duration,
    sell_simulation_mode: SellSimulationMode,
    sell_simulation_override_mode: SellSimulationOverrideMode,
}

enum SellSimulationResult {
    Amount(U256),
    Skipped(String),
}

pub struct RiskContext<'a> {
    pub provider: &'a DynProvider,
    pub router: Address,
    pub base_token: Address,
    pub token: Address,
    pub pair: Option<Address>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SellSimulationMode {
    Strict,
    BestEffort,
}

impl SellSimulationMode {
    fn parse(raw: &str) -> Result<Self> {
        let normalized = raw.trim().to_ascii_lowercase();
        match normalized.as_str() {
            "strict" => Ok(Self::Strict),
            "best_effort" | "best-effort" | "besteffort" => Ok(Self::BestEffort),
            _ => bail!("unsupported sell_simulation_mode: {raw}"),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SellSimulationOverrideMode {
    Detect,
    SkipAny,
}

impl SellSimulationOverrideMode {
    fn parse(raw: &str) -> Result<Self> {
        let normalized = raw.trim().to_ascii_lowercase();
        match normalized.as_str() {
            "detect" => Ok(Self::Detect),
            "skip_any" | "skip-any" | "skipany" => Ok(Self::SkipAny),
            _ => bail!("unsupported sell_simulation_override_mode: {raw}"),
        }
    }
}

impl RiskEngine {
    pub fn new(cfg: RiskConfig) -> Result<Self> {
        let sellability_amount_base = parse_u256_decimal(&cfg.sellability_amount_base)?;
        let call_timeout = Duration::from_millis(cfg.erc20_call_timeout_ms);
        let sell_simulation_mode = SellSimulationMode::parse(&cfg.sell_simulation_mode)?;
        let sell_simulation_override_mode =
            SellSimulationOverrideMode::parse(&cfg.sell_simulation_override_mode)?;
        Ok(Self {
            cfg,
            sellability_amount_base,
            call_timeout,
            sell_simulation_mode,
            sell_simulation_override_mode,
        })
    }

    pub async fn assess(&self, ctx: &RiskContext<'_>) -> Result<RiskDecision> {
        let mut findings = Vec::new();

        if ctx.base_token == ctx.token {
            findings.push(RiskFinding::new(
                "token matches base token",
                SCORE_FATAL,
            ));
        }

        findings.extend(self.check_erc20_sanity(ctx).await);
        findings.extend(self.check_sellability_and_tax(ctx).await);

        if findings.is_empty() {
            Ok(RiskDecision::pass())
        } else {
            let score = findings
                .iter()
                .fold(0u32, |acc, finding| acc.saturating_add(finding.score));
            let reasons: Vec<String> = findings
                .into_iter()
                .map(|finding| finding.reason)
                .collect();
            debug!(score, ?reasons, "risk reject");
            Ok(RiskDecision {
                pass: false,
                score,
                reasons,
            })
        }
    }

    async fn check_erc20_sanity(&self, ctx: &RiskContext<'_>) -> Vec<RiskFinding> {
        let mut findings = Vec::new();
        let code = match self
            .with_timeout("token code", async {
                ctx.provider.get_code_at(ctx.token).await
            })
            .await
        {
            Ok(code) => code,
            Err(reason) => {
                findings.push(RiskFinding::new(reason, SCORE_HIGH));
                return findings;
            }
        };

        if code.is_empty() {
            findings.push(RiskFinding::new(
                "token has no contract code",
                SCORE_FATAL,
            ));
            return findings;
        }

        let decimals = match self.erc20_decimals(ctx).await {
            Ok(decimals) => decimals,
            Err(reason) => {
                findings.push(RiskFinding::new(reason, SCORE_HIGH));
                return findings;
            }
        };
        if decimals > 30 {
            findings.push(RiskFinding::new(
                format!("token decimals too large: {decimals}"),
                SCORE_MEDIUM,
            ));
        }

        if let Err(reason) = self.erc20_name(ctx).await {
            findings.push(RiskFinding::new(reason, SCORE_MEDIUM));
        }
        if let Err(reason) = self.erc20_symbol(ctx).await {
            findings.push(RiskFinding::new(reason, SCORE_MEDIUM));
        }

        findings
    }

    async fn check_sellability_and_tax(&self, ctx: &RiskContext<'_>) -> Vec<RiskFinding> {
        let mut findings = Vec::new();
        if self.sellability_amount_base.is_zero() {
            debug!("sellability check disabled");
            return findings;
        }

        let pair = match ctx.pair {
            Some(pair) => pair,
            None => {
                debug!("pair unresolved; skipping sellability check");
                return findings;
            }
        };

        let (reserve0, reserve1) = match self.pair_reserves(ctx, pair).await {
            Ok(reserves) => reserves,
            Err(reason) => {
                findings.push(RiskFinding::new(reason, SCORE_HIGH));
                return findings;
            }
        };
        if reserve0.is_zero() && reserve1.is_zero() {
            debug!("pair reserves empty; skipping sellability check");
            return findings;
        }
        if reserve0.is_zero() || reserve1.is_zero() {
            findings.push(RiskFinding::new("pair reserves incomplete", SCORE_HIGH));
            return findings;
        }

        let amounts_out = match self
            .router_amounts_out(
                ctx,
                self.sellability_amount_base,
                vec![ctx.base_token, ctx.token],
            )
            .await
        {
            Ok(amounts) => amounts,
            Err(reason) => {
                findings.push(RiskFinding::new(reason, SCORE_HIGH));
                return findings;
            }
        };
        if amounts_out.len() < 2 {
            findings.push(RiskFinding::new(
                "router quote returned insufficient hop count",
                SCORE_HIGH,
            ));
            return findings;
        }
        let token_out = amounts_out[amounts_out.len() - 1];
        if token_out.is_zero() {
            findings.push(RiskFinding::new(
                "router quote returned zero token output",
                SCORE_HIGH,
            ));
            return findings;
        }

        let amounts_back = match self
            .router_amounts_out(ctx, token_out, vec![ctx.token, ctx.base_token])
            .await
        {
            Ok(amounts) => amounts,
            Err(reason) => {
                findings.push(RiskFinding::new(reason, SCORE_HIGH));
                return findings;
            }
        };
        if amounts_back.len() < 2 {
            findings.push(RiskFinding::new(
                "router reverse quote returned insufficient hop count",
                SCORE_HIGH,
            ));
            return findings;
        }
        let expected_base_out = amounts_back[amounts_back.len() - 1];
        if expected_base_out.is_zero() {
            findings.push(RiskFinding::new(
                "router reverse quote returned zero base output",
                SCORE_HIGH,
            ));
            return findings;
        }

        let simulated_out = match self
            .simulate_sell(ctx, token_out, vec![ctx.token, ctx.base_token])
            .await
        {
            Ok(SellSimulationResult::Amount(out)) => out,
            Ok(SellSimulationResult::Skipped(reason)) => {
                warn!(reason = %reason, "sell simulation skipped; tax check unavailable");
                return findings;
            }
            Err(reason) => {
                findings.push(RiskFinding::new(reason, SCORE_HIGH));
                return findings;
            }
        };
        if simulated_out.is_zero() {
            findings.push(RiskFinding::new(
                "sell simulation returned zero base output",
                SCORE_HIGH,
            ));
            return findings;
        }

        if let Some(loss_bps) = loss_bps(expected_base_out, simulated_out) {
            if is_tax_excessive(self.cfg.max_tax_bps, loss_bps) {
                findings.push(RiskFinding::new(
                    format!(
                        "tax estimate too high (expected vs simulated): {loss_bps} bps > {} bps",
                        self.cfg.max_tax_bps
                    ),
                    SCORE_HIGH,
                ));
            }
        }

        findings
    }

    async fn erc20_decimals(&self, ctx: &RiskContext<'_>) -> Result<u8, String> {
        let call = IERC20::decimalsCall {};
        let tx = TransactionRequest {
            to: Some(TxKind::Call(ctx.token)),
            input: TransactionInput::new(call.abi_encode().into()),
            ..Default::default()
        };
        let data = self
            .with_timeout("token decimals", async { ctx.provider.call(tx).await })
            .await?;
        IERC20::decimalsCall::abi_decode_returns(&data)
            .map_err(|err| format!("token decimals decode failed: {err}"))
    }

    async fn erc20_name(&self, ctx: &RiskContext<'_>) -> Result<String, String> {
        let call = IERC20::nameCall {};
        let tx = TransactionRequest {
            to: Some(TxKind::Call(ctx.token)),
            input: TransactionInput::new(call.abi_encode().into()),
            ..Default::default()
        };
        let data = self
            .with_timeout("token name", async { ctx.provider.call(tx).await })
            .await?;
        let name = IERC20::nameCall::abi_decode_returns(&data)
            .map_err(|err| format!("token name decode failed: {err}"))?;
        if name.is_empty() {
            return Err("token name is empty".to_string());
        }
        if name.len() > 64 {
            return Err(format!("token name too long: {} chars", name.len()));
        }
        Ok(name)
    }

    async fn erc20_symbol(&self, ctx: &RiskContext<'_>) -> Result<String, String> {
        let call = IERC20::symbolCall {};
        let tx = TransactionRequest {
            to: Some(TxKind::Call(ctx.token)),
            input: TransactionInput::new(call.abi_encode().into()),
            ..Default::default()
        };
        let data = self
            .with_timeout("token symbol", async { ctx.provider.call(tx).await })
            .await?;
        let symbol = IERC20::symbolCall::abi_decode_returns(&data)
            .map_err(|err| format!("token symbol decode failed: {err}"))?;
        if symbol.is_empty() {
            return Err("token symbol is empty".to_string());
        }
        if symbol.len() > 32 {
            return Err(format!("token symbol too long: {} chars", symbol.len()));
        }
        Ok(symbol)
    }

    async fn router_amounts_out(
        &self,
        ctx: &RiskContext<'_>,
        amount_in: U256,
        path: Vec<Address>,
    ) -> Result<Vec<U256>, String> {
        let call = IUniswapV2Router02::getAmountsOutCall { amountIn: amount_in, path };
        let tx = TransactionRequest {
            to: Some(TxKind::Call(ctx.router)),
            input: TransactionInput::new(call.abi_encode().into()),
            ..Default::default()
        };
        let data = self
            .with_timeout("router getAmountsOut", async { ctx.provider.call(tx).await })
            .await?;
        IUniswapV2Router02::getAmountsOutCall::abi_decode_returns(&data)
            .map_err(|err| format!("router getAmountsOut decode failed: {err}"))
    }

    async fn simulate_sell(
        &self,
        ctx: &RiskContext<'_>,
        amount_in: U256,
        path: Vec<Address>,
    ) -> Result<SellSimulationResult, String> {
        let call = IUniswapV2Router02::swapExactTokensForTokensCall {
            amountIn: amount_in,
            amountOutMin: U256::from(0u64),
            path: path.clone(),
            to: SIMULATION_SENDER,
            deadline: U256::from(SIMULATION_DEADLINE_SECS),
        };
        let tx = TransactionRequest {
            from: Some(SIMULATION_SENDER),
            to: Some(TxKind::Call(ctx.router)),
            input: TransactionInput::new(call.abi_encode().into()),
            ..Default::default()
        };
        let overrides = sell_simulation_overrides(ctx.token, SIMULATION_SENDER, ctx.router, amount_in);
        let data = self
            .with_timeout("router swapExactTokensForTokens (simulated)", async {
                ctx.provider.call(tx).overrides(overrides).await
            })
            .await;
        let data = match data {
            Ok(data) => data,
            Err(reason) => {
                if self.sell_simulation_mode == SellSimulationMode::BestEffort
                    && is_fee_on_transfer_revert(&reason)
                {
                    match self
                        .simulate_sell_supporting_fee_on_transfer(ctx, amount_in, path.clone())
                        .await
                    {
                        Ok(()) => {
                            return Ok(SellSimulationResult::Skipped(
                                "fee-on-transfer swap simulated".to_string(),
                            ));
                        }
                        Err(fee_reason) => {
                            if should_skip_override_error(
                                &fee_reason,
                                self.sell_simulation_override_mode,
                            ) {
                                return Ok(SellSimulationResult::Skipped(override_skip_message(
                                    &fee_reason,
                                    self.sell_simulation_override_mode,
                                )));
                            }
                            return Err(fee_reason);
                        }
                    }
                }
                if self.sell_simulation_mode == SellSimulationMode::BestEffort
                    && should_skip_override_error(&reason, self.sell_simulation_override_mode)
                {
                    return Ok(SellSimulationResult::Skipped(override_skip_message(
                        &reason,
                        self.sell_simulation_override_mode,
                    )));
                }
                return Err(reason);
            }
        };
        let amounts = IUniswapV2Router02::swapExactTokensForTokensCall::abi_decode_returns(&data)
            .map_err(|err| format!("router swapExactTokensForTokens decode failed: {err}"))?;
        if amounts.len() < 2 {
            return Err("router swap returned insufficient hop count".to_string());
        }
        Ok(SellSimulationResult::Amount(amounts[amounts.len() - 1]))
    }

    async fn simulate_sell_supporting_fee_on_transfer(
        &self,
        ctx: &RiskContext<'_>,
        amount_in: U256,
        path: Vec<Address>,
    ) -> Result<(), String> {
        let call =
            IUniswapV2Router02::swapExactTokensForTokensSupportingFeeOnTransferTokensCall {
                amountIn: amount_in,
                amountOutMin: U256::from(0u64),
                path,
                to: SIMULATION_SENDER,
                deadline: U256::from(SIMULATION_DEADLINE_SECS),
            };
        let tx = TransactionRequest {
            from: Some(SIMULATION_SENDER),
            to: Some(TxKind::Call(ctx.router)),
            input: TransactionInput::new(call.abi_encode().into()),
            ..Default::default()
        };
        let overrides = sell_simulation_overrides(ctx.token, SIMULATION_SENDER, ctx.router, amount_in);
        self.with_timeout(
            "router swapExactTokensForTokensSupportingFeeOnTransferTokens (simulated)",
            async { ctx.provider.call(tx).overrides(overrides).await },
        )
        .await?;
        Ok(())
    }

    async fn pair_reserves(
        &self,
        ctx: &RiskContext<'_>,
        pair: Address,
    ) -> Result<(U256, U256), String> {
        let call = crate::abi::IUniswapV2Pair::getReservesCall {};
        let tx = TransactionRequest {
            to: Some(TxKind::Call(pair)),
            input: TransactionInput::new(call.abi_encode().into()),
            ..Default::default()
        };
        let data = self
            .with_timeout("pair getReserves", async { ctx.provider.call(tx).await })
            .await?;
        let ret = crate::abi::IUniswapV2Pair::getReservesCall::abi_decode_returns(&data)
            .map_err(|err| format!("pair getReserves decode failed: {err}"))?;
        Ok((U256::from(ret.reserve0), U256::from(ret.reserve1)))
    }

    async fn with_timeout<T, Fut, E>(&self, label: &str, fut: Fut) -> std::result::Result<T, String>
    where
        Fut: Future<Output = std::result::Result<T, E>>,
        E: Display,
    {
        match timeout(self.call_timeout, fut).await {
            Ok(Ok(value)) => Ok(value),
            Ok(Err(err)) => Err(format!("{label} failed: {err}")),
            Err(_) => Err(format!(
                "{label} timed out after {}ms",
                self.call_timeout.as_millis()
            )),
        }
    }
}

fn loss_bps(amount_in: U256, amount_out: U256) -> Option<U256> {
    if amount_in.is_zero() {
        return None;
    }
    if amount_out >= amount_in {
        return Some(U256::from(0u64));
    }
    let loss = amount_in - amount_out;
    Some(loss * U256::from(10_000u64) / amount_in)
}

fn is_tax_excessive(max_tax_bps: u32, loss_bps: U256) -> bool {
    loss_bps > U256::from(max_tax_bps as u64)
}

fn sell_simulation_overrides(
    token: Address,
    owner: Address,
    spender: Address,
    amount_in: U256,
) -> StateOverride {
    // Assumes standard ERC20 storage layout: balances at slot 0, allowances at slot 1.
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

fn is_state_override_unsupported(reason: &str) -> bool {
    let lower = reason.to_ascii_lowercase();
    if lower.contains("state override")
        || lower.contains("stateoverride")
        || lower.contains("stateoverrides")
    {
        return true;
    }
    (lower.contains("unknown field") || lower.contains("unsupported field"))
        && lower.contains("overrides")
}

fn should_skip_override_error(reason: &str, mode: SellSimulationOverrideMode) -> bool {
    match mode {
        SellSimulationOverrideMode::Detect => is_state_override_unsupported(reason),
        SellSimulationOverrideMode::SkipAny => is_probable_override_error(reason),
    }
}

fn override_skip_message(reason: &str, mode: SellSimulationOverrideMode) -> String {
    match mode {
        SellSimulationOverrideMode::Detect => "state overrides unsupported".to_string(),
        SellSimulationOverrideMode::SkipAny => {
            format!("sell simulation failed (override error): {reason}")
        }
    }
}

fn is_probable_override_error(reason: &str) -> bool {
    if is_state_override_unsupported(reason) {
        return true;
    }
    let lower = reason.to_ascii_lowercase();
    if lower.contains("revert") {
        return false;
    }
    let patterns = [
        "invalid params",
        "invalid param",
        "invalid argument",
        "unknown field",
        "unsupported field",
        "unexpected field",
        "unrecognized field",
        "invalid request",
        "missing field",
        "extra field",
        "invalid type",
        "failed to deserialize",
        "failed to parse",
    ];
    patterns.iter().any(|pattern| lower.contains(pattern))
}

fn is_fee_on_transfer_revert(reason: &str) -> bool {
    let upper = reason.to_ascii_uppercase();
    if upper.contains("INSUFFICIENT_INPUT_AMOUNT") {
        return true;
    }
    upper.contains("UNISWAPV2: K") || upper.contains("PANCAKE: K")
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
    use super::{
        is_tax_excessive, loss_bps, RiskContext, RiskEngine, SCORE_HIGH, SCORE_MEDIUM,
    };
    use alloy::primitives::{address, Bytes, Uint, U256};
    use alloy::providers::{Provider, ProviderBuilder};
    use alloy::sol_types::SolCall;
    use alloy::transports::mock::Asserter;
    use sonic_core::config::RiskConfig;

    use crate::abi::{IERC20, IUniswapV2Pair, IUniswapV2Router02};

    #[test]
    fn loss_bps_zero_on_gain() {
        let loss = loss_bps(U256::from(100u64), U256::from(110u64)).unwrap();
        assert_eq!(loss, U256::from(0u64));
    }

    #[test]
    fn loss_bps_computes_expected() {
        let loss = loss_bps(U256::from(100u64), U256::from(95u64)).unwrap();
        assert_eq!(loss, U256::from(500u64));
    }

    #[test]
    fn tax_excessive_when_loss_above_threshold() {
        let loss = U256::from(501u64);
        assert!(is_tax_excessive(500, loss));
        assert!(!is_tax_excessive(600, loss));
    }

    #[tokio::test]
    async fn assess_rejects_on_excessive_round_trip_loss() {
        let asserter = Asserter::new();
        let provider = ProviderBuilder::new()
            .connect_mocked_client(asserter.clone())
            .erased();
        let cfg = RiskConfig {
            sellability_amount_base: "1000".to_string(),
            max_tax_bps: 100,
            erc20_call_timeout_ms: 500,
            sell_simulation_mode: "strict".to_string(),
            sell_simulation_override_mode: "detect".to_string(),
        };
        let engine = RiskEngine::new(cfg).unwrap();
        let ctx = RiskContext {
            provider: &provider,
            router: address!("0x1000000000000000000000000000000000000001"),
            base_token: address!("0x2000000000000000000000000000000000000002"),
            token: address!("0x3000000000000000000000000000000000000003"),
            pair: Some(address!("0x4000000000000000000000000000000000000004")),
        };

        asserter.push_success(&Bytes::from(vec![1u8]));
        push_bytes(&asserter, IERC20::decimalsCall::abi_encode_returns(&18u8));
        push_bytes(
            &asserter,
            IERC20::nameCall::abi_encode_returns(&"TestToken".to_string()),
        );
        push_bytes(
            &asserter,
            IERC20::symbolCall::abi_encode_returns(&"TST".to_string()),
        );

        type U112 = Uint<112, 2>;
        type ReservesReturn = <IUniswapV2Pair::getReservesCall as SolCall>::Return;
        let reserves = ReservesReturn {
            reserve0: U112::from(1_000u64),
            reserve1: U112::from(2_000u64),
            blockTimestampLast: 1u32,
        };
        push_bytes(
            &asserter,
            IUniswapV2Pair::getReservesCall::abi_encode_returns(&reserves),
        );

        let forward = vec![U256::from(1000u64), U256::from(900u64)];
        push_bytes(
            &asserter,
            IUniswapV2Router02::getAmountsOutCall::abi_encode_returns(&forward),
        );
        let backward = vec![U256::from(900u64), U256::from(800u64)];
        push_bytes(
            &asserter,
            IUniswapV2Router02::getAmountsOutCall::abi_encode_returns(&backward),
        );
        let simulated = vec![U256::from(900u64), U256::from(700u64)];
        push_bytes(
            &asserter,
            IUniswapV2Router02::swapExactTokensForTokensCall::abi_encode_returns(&simulated),
        );

        let decision = engine.assess(&ctx).await.unwrap();
        assert!(!decision.pass);
        assert!(decision.score >= SCORE_HIGH);
        assert!(decision.reasons.iter().any(|reason| {
            reason.contains("tax estimate too high (expected vs simulated)")
        }));
        assert!(asserter.read_q().is_empty());
    }

    #[tokio::test]
    async fn assess_rejects_on_erc20_symbol_failure() {
        let asserter = Asserter::new();
        let provider = ProviderBuilder::new()
            .connect_mocked_client(asserter.clone())
            .erased();
        let cfg = RiskConfig {
            sellability_amount_base: "0".to_string(),
            max_tax_bps: 100,
            erc20_call_timeout_ms: 500,
            sell_simulation_mode: "strict".to_string(),
            sell_simulation_override_mode: "detect".to_string(),
        };
        let engine = RiskEngine::new(cfg).unwrap();
        let ctx = RiskContext {
            provider: &provider,
            router: address!("0x1000000000000000000000000000000000000001"),
            base_token: address!("0x2000000000000000000000000000000000000002"),
            token: address!("0x3000000000000000000000000000000000000003"),
            pair: None,
        };

        asserter.push_success(&Bytes::from(vec![1u8]));
        push_bytes(&asserter, IERC20::decimalsCall::abi_encode_returns(&18u8));
        push_bytes(
            &asserter,
            IERC20::nameCall::abi_encode_returns(&"TestToken".to_string()),
        );
        asserter.push_failure_msg("symbol revert");

        let decision = engine.assess(&ctx).await.unwrap();
        assert!(!decision.pass);
        assert!(decision.score >= SCORE_MEDIUM);
        assert!(decision
            .reasons
            .iter()
            .any(|reason| reason.contains("token symbol")));
        assert!(asserter.read_q().is_empty());
    }

    #[tokio::test]
    async fn assess_passes_when_sellability_disabled() {
        let asserter = Asserter::new();
        let provider = ProviderBuilder::new()
            .connect_mocked_client(asserter.clone())
            .erased();
        let cfg = RiskConfig {
            sellability_amount_base: "0".to_string(),
            max_tax_bps: 100,
            erc20_call_timeout_ms: 500,
            sell_simulation_mode: "strict".to_string(),
            sell_simulation_override_mode: "detect".to_string(),
        };
        let engine = RiskEngine::new(cfg).unwrap();
        let ctx = RiskContext {
            provider: &provider,
            router: address!("0x1000000000000000000000000000000000000001"),
            base_token: address!("0x2000000000000000000000000000000000000002"),
            token: address!("0x3000000000000000000000000000000000000003"),
            pair: None,
        };

        asserter.push_success(&Bytes::from(vec![1u8]));
        push_bytes(&asserter, IERC20::decimalsCall::abi_encode_returns(&18u8));
        push_bytes(
            &asserter,
            IERC20::nameCall::abi_encode_returns(&"TestToken".to_string()),
        );
        push_bytes(
            &asserter,
            IERC20::symbolCall::abi_encode_returns(&"TST".to_string()),
        );

        let decision = engine.assess(&ctx).await.unwrap();
        assert!(decision.pass);
        assert_eq!(decision.score, 0);
        assert!(decision.reasons.is_empty());
        assert!(asserter.read_q().is_empty());
    }

    #[tokio::test]
    async fn assess_passes_when_pair_reserves_empty() {
        let asserter = Asserter::new();
        let provider = ProviderBuilder::new()
            .connect_mocked_client(asserter.clone())
            .erased();
        let cfg = RiskConfig {
            sellability_amount_base: "1000".to_string(),
            max_tax_bps: 100,
            erc20_call_timeout_ms: 500,
            sell_simulation_mode: "strict".to_string(),
            sell_simulation_override_mode: "detect".to_string(),
        };
        let engine = RiskEngine::new(cfg).unwrap();
        let ctx = RiskContext {
            provider: &provider,
            router: address!("0x1000000000000000000000000000000000000001"),
            base_token: address!("0x2000000000000000000000000000000000000002"),
            token: address!("0x3000000000000000000000000000000000000003"),
            pair: Some(address!("0x4000000000000000000000000000000000000004")),
        };

        asserter.push_success(&Bytes::from(vec![1u8]));
        push_bytes(&asserter, IERC20::decimalsCall::abi_encode_returns(&18u8));
        push_bytes(
            &asserter,
            IERC20::nameCall::abi_encode_returns(&"TestToken".to_string()),
        );
        push_bytes(
            &asserter,
            IERC20::symbolCall::abi_encode_returns(&"TST".to_string()),
        );

        type U112 = Uint<112, 2>;
        type ReservesReturn = <IUniswapV2Pair::getReservesCall as SolCall>::Return;
        let reserves = ReservesReturn {
            reserve0: U112::from(0u64),
            reserve1: U112::from(0u64),
            blockTimestampLast: 1u32,
        };
        push_bytes(
            &asserter,
            IUniswapV2Pair::getReservesCall::abi_encode_returns(&reserves),
        );

        let decision = engine.assess(&ctx).await.unwrap();
        assert!(decision.pass);
        assert_eq!(decision.score, 0);
        assert!(decision.reasons.is_empty());
        assert!(asserter.read_q().is_empty());
    }

    #[tokio::test]
    async fn assess_passes_when_state_overrides_unsupported_in_best_effort() {
        let asserter = Asserter::new();
        let provider = ProviderBuilder::new()
            .connect_mocked_client(asserter.clone())
            .erased();
        let cfg = RiskConfig {
            sellability_amount_base: "1000".to_string(),
            max_tax_bps: 100,
            erc20_call_timeout_ms: 500,
            sell_simulation_mode: "best_effort".to_string(),
            sell_simulation_override_mode: "detect".to_string(),
        };
        let engine = RiskEngine::new(cfg).unwrap();
        let ctx = RiskContext {
            provider: &provider,
            router: address!("0x1000000000000000000000000000000000000001"),
            base_token: address!("0x2000000000000000000000000000000000000002"),
            token: address!("0x3000000000000000000000000000000000000003"),
            pair: Some(address!("0x4000000000000000000000000000000000000004")),
        };

        asserter.push_success(&Bytes::from(vec![1u8]));
        push_bytes(&asserter, IERC20::decimalsCall::abi_encode_returns(&18u8));
        push_bytes(
            &asserter,
            IERC20::nameCall::abi_encode_returns(&"TestToken".to_string()),
        );
        push_bytes(
            &asserter,
            IERC20::symbolCall::abi_encode_returns(&"TST".to_string()),
        );

        type U112 = Uint<112, 2>;
        type ReservesReturn = <IUniswapV2Pair::getReservesCall as SolCall>::Return;
        let reserves = ReservesReturn {
            reserve0: U112::from(1_000u64),
            reserve1: U112::from(2_000u64),
            blockTimestampLast: 1u32,
        };
        push_bytes(
            &asserter,
            IUniswapV2Pair::getReservesCall::abi_encode_returns(&reserves),
        );

        let forward = vec![U256::from(1000u64), U256::from(900u64)];
        push_bytes(
            &asserter,
            IUniswapV2Router02::getAmountsOutCall::abi_encode_returns(&forward),
        );
        let backward = vec![U256::from(900u64), U256::from(800u64)];
        push_bytes(
            &asserter,
            IUniswapV2Router02::getAmountsOutCall::abi_encode_returns(&backward),
        );
        asserter.push_failure_msg("state overrides not supported");

        let decision = engine.assess(&ctx).await.unwrap();
        assert!(decision.pass);
        assert!(decision.reasons.is_empty());
        assert!(asserter.read_q().is_empty());
    }

    #[tokio::test]
    async fn assess_passes_when_fee_on_transfer_simulated_in_best_effort() {
        let asserter = Asserter::new();
        let provider = ProviderBuilder::new()
            .connect_mocked_client(asserter.clone())
            .erased();
        let cfg = RiskConfig {
            sellability_amount_base: "1000".to_string(),
            max_tax_bps: 100,
            erc20_call_timeout_ms: 500,
            sell_simulation_mode: "best_effort".to_string(),
            sell_simulation_override_mode: "detect".to_string(),
        };
        let engine = RiskEngine::new(cfg).unwrap();
        let ctx = RiskContext {
            provider: &provider,
            router: address!("0x1000000000000000000000000000000000000001"),
            base_token: address!("0x2000000000000000000000000000000000000002"),
            token: address!("0x3000000000000000000000000000000000000003"),
            pair: Some(address!("0x4000000000000000000000000000000000000004")),
        };

        asserter.push_success(&Bytes::from(vec![1u8]));
        push_bytes(&asserter, IERC20::decimalsCall::abi_encode_returns(&18u8));
        push_bytes(
            &asserter,
            IERC20::nameCall::abi_encode_returns(&"TestToken".to_string()),
        );
        push_bytes(
            &asserter,
            IERC20::symbolCall::abi_encode_returns(&"TST".to_string()),
        );

        type U112 = Uint<112, 2>;
        type ReservesReturn = <IUniswapV2Pair::getReservesCall as SolCall>::Return;
        let reserves = ReservesReturn {
            reserve0: U112::from(1_000u64),
            reserve1: U112::from(2_000u64),
            blockTimestampLast: 1u32,
        };
        push_bytes(
            &asserter,
            IUniswapV2Pair::getReservesCall::abi_encode_returns(&reserves),
        );

        let forward = vec![U256::from(1000u64), U256::from(900u64)];
        push_bytes(
            &asserter,
            IUniswapV2Router02::getAmountsOutCall::abi_encode_returns(&forward),
        );
        let backward = vec![U256::from(900u64), U256::from(800u64)];
        push_bytes(
            &asserter,
            IUniswapV2Router02::getAmountsOutCall::abi_encode_returns(&backward),
        );

        asserter.push_failure_msg("INSUFFICIENT_INPUT_AMOUNT");
        asserter.push_success(&Bytes::from(vec![]));

        let decision = engine.assess(&ctx).await.unwrap();
        assert!(decision.pass);
        assert!(decision.reasons.is_empty());
        assert!(asserter.read_q().is_empty());
    }

    #[tokio::test]
    async fn assess_rejects_when_swap_failure_not_fee_on_transfer() {
        let asserter = Asserter::new();
        let provider = ProviderBuilder::new()
            .connect_mocked_client(asserter.clone())
            .erased();
        let cfg = RiskConfig {
            sellability_amount_base: "1000".to_string(),
            max_tax_bps: 100,
            erc20_call_timeout_ms: 500,
            sell_simulation_mode: "best_effort".to_string(),
            sell_simulation_override_mode: "detect".to_string(),
        };
        let engine = RiskEngine::new(cfg).unwrap();
        let ctx = RiskContext {
            provider: &provider,
            router: address!("0x1000000000000000000000000000000000000001"),
            base_token: address!("0x2000000000000000000000000000000000000002"),
            token: address!("0x3000000000000000000000000000000000000003"),
            pair: Some(address!("0x4000000000000000000000000000000000000004")),
        };

        asserter.push_success(&Bytes::from(vec![1u8]));
        push_bytes(&asserter, IERC20::decimalsCall::abi_encode_returns(&18u8));
        push_bytes(
            &asserter,
            IERC20::nameCall::abi_encode_returns(&"TestToken".to_string()),
        );
        push_bytes(
            &asserter,
            IERC20::symbolCall::abi_encode_returns(&"TST".to_string()),
        );

        type U112 = Uint<112, 2>;
        type ReservesReturn = <IUniswapV2Pair::getReservesCall as SolCall>::Return;
        let reserves = ReservesReturn {
            reserve0: U112::from(1_000u64),
            reserve1: U112::from(2_000u64),
            blockTimestampLast: 1u32,
        };
        push_bytes(
            &asserter,
            IUniswapV2Pair::getReservesCall::abi_encode_returns(&reserves),
        );

        let forward = vec![U256::from(1000u64), U256::from(900u64)];
        push_bytes(
            &asserter,
            IUniswapV2Router02::getAmountsOutCall::abi_encode_returns(&forward),
        );
        let backward = vec![U256::from(900u64), U256::from(800u64)];
        push_bytes(
            &asserter,
            IUniswapV2Router02::getAmountsOutCall::abi_encode_returns(&backward),
        );

        asserter.push_failure_msg("TRANSFER_FAILED");

        let decision = engine.assess(&ctx).await.unwrap();
        assert!(!decision.pass);
        assert!(decision
            .reasons
            .iter()
            .any(|reason| reason.contains("TRANSFER_FAILED")));
        assert!(asserter.read_q().is_empty());
    }

    #[tokio::test]
    async fn assess_passes_when_override_mode_skips_any_error() {
        let asserter = Asserter::new();
        let provider = ProviderBuilder::new()
            .connect_mocked_client(asserter.clone())
            .erased();
        let cfg = RiskConfig {
            sellability_amount_base: "1000".to_string(),
            max_tax_bps: 100,
            erc20_call_timeout_ms: 500,
            sell_simulation_mode: "best_effort".to_string(),
            sell_simulation_override_mode: "skip_any".to_string(),
        };
        let engine = RiskEngine::new(cfg).unwrap();
        let ctx = RiskContext {
            provider: &provider,
            router: address!("0x1000000000000000000000000000000000000001"),
            base_token: address!("0x2000000000000000000000000000000000000002"),
            token: address!("0x3000000000000000000000000000000000000003"),
            pair: Some(address!("0x4000000000000000000000000000000000000004")),
        };

        asserter.push_success(&Bytes::from(vec![1u8]));
        push_bytes(&asserter, IERC20::decimalsCall::abi_encode_returns(&18u8));
        push_bytes(
            &asserter,
            IERC20::nameCall::abi_encode_returns(&"TestToken".to_string()),
        );
        push_bytes(
            &asserter,
            IERC20::symbolCall::abi_encode_returns(&"TST".to_string()),
        );

        type U112 = Uint<112, 2>;
        type ReservesReturn = <IUniswapV2Pair::getReservesCall as SolCall>::Return;
        let reserves = ReservesReturn {
            reserve0: U112::from(1_000u64),
            reserve1: U112::from(2_000u64),
            blockTimestampLast: 1u32,
        };
        push_bytes(
            &asserter,
            IUniswapV2Pair::getReservesCall::abi_encode_returns(&reserves),
        );

        let forward = vec![U256::from(1000u64), U256::from(900u64)];
        push_bytes(
            &asserter,
            IUniswapV2Router02::getAmountsOutCall::abi_encode_returns(&forward),
        );
        let backward = vec![U256::from(900u64), U256::from(800u64)];
        push_bytes(
            &asserter,
            IUniswapV2Router02::getAmountsOutCall::abi_encode_returns(&backward),
        );

        asserter.push_failure_msg("invalid params: invalid argument 2");

        let decision = engine.assess(&ctx).await.unwrap();
        assert!(decision.pass);
        assert!(decision.reasons.is_empty());
        assert!(asserter.read_q().is_empty());
    }

    #[tokio::test]
    async fn assess_rejects_when_override_mode_skip_any_sees_contract_revert() {
        let asserter = Asserter::new();
        let provider = ProviderBuilder::new()
            .connect_mocked_client(asserter.clone())
            .erased();
        let cfg = RiskConfig {
            sellability_amount_base: "1000".to_string(),
            max_tax_bps: 100,
            erc20_call_timeout_ms: 500,
            sell_simulation_mode: "best_effort".to_string(),
            sell_simulation_override_mode: "skip_any".to_string(),
        };
        let engine = RiskEngine::new(cfg).unwrap();
        let ctx = RiskContext {
            provider: &provider,
            router: address!("0x1000000000000000000000000000000000000001"),
            base_token: address!("0x2000000000000000000000000000000000000002"),
            token: address!("0x3000000000000000000000000000000000000003"),
            pair: Some(address!("0x4000000000000000000000000000000000000004")),
        };

        asserter.push_success(&Bytes::from(vec![1u8]));
        push_bytes(&asserter, IERC20::decimalsCall::abi_encode_returns(&18u8));
        push_bytes(
            &asserter,
            IERC20::nameCall::abi_encode_returns(&"TestToken".to_string()),
        );
        push_bytes(
            &asserter,
            IERC20::symbolCall::abi_encode_returns(&"TST".to_string()),
        );

        type U112 = Uint<112, 2>;
        type ReservesReturn = <IUniswapV2Pair::getReservesCall as SolCall>::Return;
        let reserves = ReservesReturn {
            reserve0: U112::from(1_000u64),
            reserve1: U112::from(2_000u64),
            blockTimestampLast: 1u32,
        };
        push_bytes(
            &asserter,
            IUniswapV2Pair::getReservesCall::abi_encode_returns(&reserves),
        );

        let forward = vec![U256::from(1000u64), U256::from(900u64)];
        push_bytes(
            &asserter,
            IUniswapV2Router02::getAmountsOutCall::abi_encode_returns(&forward),
        );
        let backward = vec![U256::from(900u64), U256::from(800u64)];
        push_bytes(
            &asserter,
            IUniswapV2Router02::getAmountsOutCall::abi_encode_returns(&backward),
        );

        asserter.push_failure_msg("TRANSFER_FAILED");

        let decision = engine.assess(&ctx).await.unwrap();
        assert!(!decision.pass);
        assert!(decision
            .reasons
            .iter()
            .any(|reason| reason.contains("TRANSFER_FAILED")));
        assert!(asserter.read_q().is_empty());
    }

    fn push_bytes(asserter: &Asserter, data: Vec<u8>) {
        asserter.push_success(&Bytes::from(data));
    }
}
