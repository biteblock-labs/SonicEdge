# SonicEdge Architecture

## Overview
SonicEdge is a mempool-driven liquidity-add sniper for Sonic mainnet (EVM). It listens to pending transactions, detects V2/Solidly liquidity adds, applies a launch-only gate, runs fast risk checks (ERC20 sanity + sell simulation), and submits an atomic buy via a minimal on-chain executor contract.

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
```

## Crate Responsibilities
- `crates/core`
  - Typed config, shared types, errors, utilities, metrics scaffold.
- `crates/chain`
  - WS/HTTP node clients, pending tx stream, new head stream, txpool reader, fetcher.
- `crates/dex`
  - UniswapV2 ABIs, calldata decoders, pair helpers (CREATE2 + block-scoped reserves).
- `crates/risk`
  - Modular risk filters and decision model (ERC20 sanity, sell simulation, tax estimation, scoring).
- `crates/executor`
  - Fee strategy, nonce manager, transaction builder, sender.
- `crates/bot`
  - Orchestration state machine: detect -> qualify -> execute -> manage.
- `bins/sniper`
  - CLI entrypoint and commands.

## On-Chain Executor
- `contracts/SonicSniperExecutor.sol`
  - Ownable, minimal storage.
  - `buyV2` for atomic swaps with reserve and block guards.
  - Optional fee-on-transfer swap path.
  - Rescue methods for tokens and ETH.

## Execution Strategy (V1)
- Current: submit a buy immediately after pending addLiquidity detection + risk pass.
- Planned: wait-for-mine primary path, with optional same-block attempt and guardrails.
- Fees: aggressive fee strategy with bump/replace policy.

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

## Failure Modes and Guards
- Missed pending txs: txpool backfill + dedupe window.
- Slow decoding: fast selector gating, minimal allocations.
- Scam/tax tokens: risk engine simulation checks.
- Late inclusion: `maxBlockNumber` guard enforced in live flow to avoid late inclusion.
- Pair resolution: router->factory mapping avoids cross-DEX pair mismatches; CREATE2 derivation uses factory init code hashes when `getPair` misses; negative cache TTL keeps new pools discoverable.
- Launch-only gate: checks pair code/reserves at the prior block; strict/best-effort modes decide how to handle missing historical state.

## Extension Points
- Solidly decoder + pair helpers are supported; V3/CLMM remains future work.
- Add private relay integration if available on Sonic.
- Extend risk engine with per-router heuristics and quote methods.
