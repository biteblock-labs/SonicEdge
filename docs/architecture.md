# SonicEdge Architecture

## Overview
SonicEdge is a mempool-driven liquidity-add sniper for Sonic mainnet (EVM). It listens to pending transactions, detects V2/Solidly liquidity adds, applies a launch-only gate, runs fast risk checks (ERC20 sanity, trading controls, limit heuristics, sell simulation/tax), and submits an atomic buy via a minimal on-chain executor contract. Buy sizing supports fixed, liquidity-capped, or wallet-percentage modes with configurable min/max caps and a native-gas reserve. It also tracks open positions for TP/SL/max-hold exits with optional emergency triggers, persists position state across restarts, and can emit periodic position snapshots for live monitoring.

## High-Level Data Flow
```
WS newPendingTransactions
        |
        v
+-----------------+       +------------------+
| PendingTxStream |-----> |    TxFetcher     |
+-----------------+       +------------------+
        |                         |
        | (optional)              v
        +---------+        +--------------+
Txpool backfill   |------> | V2 Decoder   |
                  |        +--------------+
                  |                |
                  v                v
               Dedupe       +-------------+
                             | Launch Gate |
                             +-------------+
                                     |
                                     v
                             +-------------+
                             | Risk Engine |
                             +-------------+
                                     |
                                     v
                            +-------------------+
                            | Executor Tx Build |
                            +-------------------+
                                     |
                                     v
                            +-------------------+
                            | On-chain Executor |
                            +-------------------+
                                     |
                                     v
                            +-------------------+
                            | Position Tracking |
                            +-------------------+
```

## Crate Responsibilities
- `crates/core`
  - Typed config, shared types, errors, utilities, metrics scaffold.
- `crates/chain`
  - WS/HTTP node clients, pending tx stream, new head stream, txpool reader, fetcher.
- `crates/dex`
  - UniswapV2 ABIs, calldata decoders, pair helpers (CREATE2 + block-scoped reserves).
- `crates/risk`
  - Modular risk filters and decision model (ERC20 sanity, trading controls, maxTx/maxWallet/cooldown checks, sell simulation, tax estimation, scoring).
- `crates/executor`
  - Fee strategy, nonce manager, transaction builder, sender.
- `crates/bot`
  - Orchestration state machine: detect -> qualify -> execute -> manage.
- `bins/sniper`
  - CLI entrypoint and commands (`run`, `deploy-contract`, `test-decode`, `replay`, `print-config`).

## On-Chain Executor
- `contracts/SonicSniperExecutor.sol`
  - Ownable, minimal storage.
  - `buyV2` for atomic swaps with reserve and block guards.
  - Optional fee-on-transfer swap path (toggle via `setUseFeeOnTransfer`).
  - Rescue methods for tokens and ETH.

## Execution Strategy (V1)
- Primary: wait for addLiquidity receipt, then evaluate risk and execute.
- Optional: same-block attempt pre-mine with fallback to the receipt path; default behavior requires non-zero reserves first.
- Fees: aggressive fee strategy with bump/replace policy and a configurable `estimateGas` buffer.
- Nonce handling: strip nonce for gas estimation; retry on nonce too low/high with synced nonce; bump on underpriced; txpool lookup for already-known when payload matches.
- Buy sizing: fixed/liquidity/wallet-pct modes, with min/max caps, optional max-liquidity bps, and a native reserve applied before wallet-pct sizing.
- Balance-aware execution: transient balance RPC failures defer candidates; insufficient funds are terminal.
- Positions are managed in a periodic exit loop with TP/SL/max-hold and optional emergency triggers (reserve drop, repeated sell simulation failures).
- Open positions and exit signals are persisted to disk when configured, and snapshot logs can be emitted on a configurable interval.

## Mempool Ingestion
- WS subscriptions:
  - `eth_subscribe` `newPendingTransactions` for pending hashes.
  - `eth_subscribe` `newHeads` for block timing.
- Subscriptions auto-reconnect with backoff when the WS stream drops.
- HTTP fallback:
  - `eth_getTransactionByHash`
  - `eth_call`
  - `eth_getTransactionReceipt`
- Txpool backfill:
  - `txpool_content` polling to catch missed hashes.

## Notifications & Observability
- Telegram notifications can be enabled with env vars; entry/exit messages include SonicScan tx links.
- Prometheus metrics can be exposed on a configurable bind address.

## Failure Modes and Guards
- Missed pending txs: txpool backfill + dedupe window.
- Slow decoding: fast selector gating, minimal allocations.
- Scam/tax tokens: risk engine checks (trading toggles, maxTx/maxWallet/cooldown heuristics, sell simulation/tax).
- LP safety: optional LP burn/lock heuristics based on LP token balances at burn/locker addresses; configurable OR/AND semantics and strict/best-effort when pairs are unresolved.
- Late inclusion: `maxBlockNumber` guard enforced in live flow to avoid late inclusion.
- Pair resolution: router->factory mapping avoids cross-DEX pair mismatches; CREATE2 derivation uses factory init code hashes when `getPair` misses; negative cache TTL keeps new pools discoverable.
- Launch-only gate: checks pair code/reserves at the prior block; strict/best-effort modes decide how to handle missing historical state.
- Router sellability recheck: periodically re-tests disabled routers to avoid permanent disablement from transient RPC errors.
- Balance lookup failures: candidates are deferred and retried with a configurable retry TTL; insufficient funds drops the candidate.
- Exits: TP/SL uses router quotes with spot-price fallback; signals are pruned after a TTL if exit transactions cannot be sent.

## Extension Points
- Solidly decoder + pair helpers are supported; V3/CLMM remains future work.
- Add private relay integration if available on Sonic.
- Extend risk engine with per-router heuristics and quote methods.
