# SonicEdge Architecture

## Overview
SonicEdge is a mempool-driven liquidity-add sniper for Sonic mainnet (EVM). It listens to pending transactions, detects V2 liquidity adds, runs fast risk checks, and submits an atomic buy via a minimal on-chain executor contract.

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
  - UniswapV2 ABIs, calldata decoders, pair helpers.
- `crates/risk`
  - Modular risk filters and decision model (sellability, tax, ERC20 sanity).
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
- Primary: wait for addLiquidity to be mined, then immediately submit buy transaction.
- Optional: same-block attempt (guarded by `maxBlockNumber`).
- Uses aggressive fee strategy with bump/replace policy (to be implemented).

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
- Late inclusion: `maxBlockNumber` guard on-chain.
- Pair resolution: router->factory mapping avoids cross-DEX pair mismatches; CREATE2 derivation uses factory init code hashes when `getPair` misses; negative cache TTL keeps new pools discoverable.

## Extension Points
- Solidly decoder + pair helpers are supported; V3/CLMM remains future work.
- Add private relay integration if available on Sonic.
- Extend risk engine with configurable heuristics and scoring.
