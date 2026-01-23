# SonicEdge Roadmap

Developer roadmap with checklist items aligned to the original scope.

## Phase 0: Repository Scaffolding
- [x] Workspace layout with core/chain/dex/risk/executor/bot crates and `sniper` bin
- [x] Example config `config/sonic.example.toml`
- [x] Justfile for fmt/clippy/test/run
- [x] Minimal README with architecture + operational notes
- [x] Solidity executor contract + ABI JSON
- [x] UniswapV2 router/factory/pair ABI JSONs
- [x] Unit test: decode V2 addLiquidity
- [x] Unit test: encode executor buyV2

## Phase 1: Mempool Ingestion (V1)
- [x] WS pending tx hash subscription (`newPendingTransactions`)
- [x] WS new head subscription (`newHeads`)
- [x] HTTP tx fetcher with timeout
- [x] Txpool backfill loop (`txpool_content`) toggle via config
- [x] Dedup layer for tx hashes + expiration window
- [x] Backpressure metrics (queue depth, drop count)
- [x] Resubscribe/reconnect strategy on WS disconnect

## Phase 2: V2 Liquidity-Add Detection (V1)
- [x] Router calldata decoder for `addLiquidity`/`addLiquidityETH`
- [x] Base token allowlist + min base amount filter
- [x] Resolve pair address via factory.getPair (router->factory mapping)
- [x] Cache pair/token metadata (LRU with TTL + short negative TTL)
- [x] CREATE2 pair derivation fallback using factory init code hashes
- [x] Router->factory mapping support in config
- [x] Solidly addLiquidity decode + stable flag handling
- [ ] Add Sonic-native DEX registry entries (Spooky V2, Equalizer Solidly, DeFive V2) after on-chain verification
- [ ] Factory PairCreated watcher + token watchlist (optional discovery feed)
- [x] Launch-only liquidity gate (pair created or zero reserves in prior block)
- [x] Launch gate strict/best-effort modes with historical-state handling
- [x] Track candidate lifecycle in state machine

## Phase 3: Risk Filters (V1)
- [x] Risk engine scaffolding with decision struct
- [x] ERC20 sanity checks (code size, decimals/name/symbol timeouts)
- [x] Sellability simulation (base->token->base via eth_call)
- [x] Tax estimation (expected vs simulated output)
- [x] Risk score and reason propagation to decision logs

## Phase 4: Execution Engine (V1)
- [x] Executor tx builder for `buyV2`
- [x] Fee strategy module (EIP-1559/legacy)
- [x] Nonce manager stub
- [x] Sender stub for broadcast
- [x] Solidly executor path (`buySolidly`)
- [x] Nonce resync on startup + periodic refresh
- [x] Gas bump/replace policy with `bump_pct` + interval
- [x] Confirm/receipt tracking with retry windows
- [x] Enforce maxBlockNumber guard in live flow

## Phase 5: Strategy Orchestration (V1)
- [x] Detect → qualify → execute loop wired
- [x] Wait-for-mine logic on addLiquidity receipt (primary execution path)
- [x] Optional same-block attempt strategy with guardrails
- [x] Position tracking + basic exit loop (TP/SL/max hold)
- [x] Position persistence across restarts
- [x] Router sellability recheck timer
- [x] Emergency triggers (reserve drop, sell sim failure)

## Phase 6: Observability (V1)
- [x] Tracing logging scaffold
- [x] Metrics scaffold feature flag
- [x] Prometheus counters/gauges (ingestion queue depth/drops, dedup hits)
- [x] Prometheus counters/gauges (candidates, exec, failures)
- [x] Structured JSON logs option

## Phase 7: Hardening & Ops
- [x] Key management guide (env, HSM, file perms)
- [x] Config validation on startup (required fields + bounds)
- [x] CLI command: `sniper deploy-contract` verify address + ABI hash
- [ ] Replay harness with richer sample dataset
- [x] Decide and document full-pipeline verification approach (anvil fork) with acceptance criteria
- [ ] Integration tests with local sonic devnet
- [ ] End-to-end pipeline smoke test harness on anvil/local chain (liquidity add → execution → exits → restart)
- [ ] Token zoo risk harness on anvil (standard/FOT/honeypot/bad metadata/non-standard storage) + one-command runner
- [ ] Telegram notifier for alerts and visibility (candidates, risk rejects, executions, PnL snapshots)
- [ ] Optional Telegram control commands (restricted allowlist + audit log)

## Phase 8: Extensions (Post‑V1)
- [x] Solidly router decoders + pair helpers
- [ ] V3/CLMM decoders + pool math
- [ ] Expand DEX coverage beyond V2/Solidly (CLMM/Balancer/Curve) after protocol support lands (deferred)
- [ ] Multi-hop quoting helpers
- [ ] MEV private relay integration (if Sonic relays appear)
- [ ] Config-driven knobs for current hard-coded behaviors (risk scoring, head freshness, reserve/slippage guards, revert pattern lists)
