# SonicEdge

Sonic mainnet mempool liquidity-add sniper bot with on-chain executor for atomic execution.

## Architecture

```
            +-----------------+          +-------------------+
WS pending  | PendingTxStream |--------->|   TxFetcher       |
WS heads    +-----------------+          +-------------------+
Txpool poll | TxpoolBackfill  |----+             |
            +-----------------+    |             v
                                   |      +-------------+
                                   +----->| Dex Decoder |
                                          +-------------+
                                                  |
                                                  v
                                          +-------------+
                                          | Risk Engine |
                                          +-------------+
                                                  |
                                                  v
                                          +---------------------+
                                          | Executor Tx Builder |
                                          +---------------------+
                                                  |
                                                  v
                                          +---------------------+
                                          |  On-chain Executor  |
                                          +---------------------+
```

## Quick Start

1. Run a local `go-ethereum-sonic` full node with:
   - WebSocket enabled for subscriptions
   - txpool enabled for `txpool_content`
   - HTTP RPC enabled for calls and receipts
2. Copy and edit config:
   - `config/sonic.example.toml` (template)
   - `config/sonic.alchemy.toml` (mainnet-ready DEX allowlists; update RPC + executor)
3. Create a `.env` file (see `env.example`) and set `SNIPER_PK`.
4. Run:

```
just run
```

`just run` uses `config/sonic.alchemy.toml` by default (see `justfile`).

## Pending Subscriptions

- Uses `eth_subscribe` for `newPendingTransactions` and `newHeads` on the WS endpoint.
- Falls back to HTTP for `eth_call`, `eth_getTransactionByHash`, and receipts.

## Mempool Caveats

- WebSocket pending hashes can drop during reconnects or RPC pressure.
- Subscriptions auto-reconnect with backoff; expect short gaps during resubscribe.
- `txpool_content` backfill helps catch missed txs, but increases RPC load.
- Expect occasional duplicates; the pipeline dedupes hashes with a TTL window.

## Security Notes

- The bot is designed for local signing + raw sends; avoid RPC personal accounts.
- Keep `SNIPER_PK` in a secure environment and avoid shell history leaks.
- The CLI loads `.env` automatically; prefer an env file with `0600` permissions or a secrets manager (systemd `EnvironmentFile` works well).
- Optional: set `TELEGRAM_BOT_TOKEN` and `TELEGRAM_CHAT_ID` in `.env` to enable Telegram alerts.
- Telegram entry/exit notifications include SonicScan tx links for quick tracking.
- Never store private keys in config files or logs.
- The executor contract is `Ownable` and NOT upgradeable.
- Approvals should be tight and reset to zero after use.
- If you plan to trade fee-on-transfer tokens, set `useFeeOnTransfer` on the executor.

## Contract Verification

- Run `sniper deploy-contract` to print the canonical ABI hash and compare the configured executor's on-chain bytecode against `contracts/bytecode/SonicSniperExecutor.hex` when present.
- ABI and bytecode artifacts can be regenerated with Foundry (`forge inspect ...`).

## Contract Tests (Foundry)

```
forge test
```

## Production Service (systemd + logrotate)

- Systemd unit: `scripts/systemd/sonicedge.service`
- Logrotate config: `scripts/logrotate/sonicedge`
- Build the release binary first: `cargo build --release -p sniper`

## Testing (Anvil)

- For deterministic risk-path coverage, use a small “token zoo” on an anvil fork (standard ERC20, fee-on-transfer, honeypot/blacklist, bad metadata, non-standard storage). See `docs/ops-runbook.md` for the full checklist and recommended risk settings.
- Local limit-check zoo tokens (addresses reset on anvil restart): ZOK `0xfbC22278A96299D91d41C453234d97b4F5Eb9B2d` (pass), ZST `0x46b142DD1E924FAb83eCc3c08e4D46E82f005e0E` (fail maxTx/maxWallet), ZCD `0xC9a43158891282A2B1475592D5719c001986Aaec` (fail cooldown).
- Anvil is the chosen full-pipeline verification approach before mainnet runs.

## Limitations (v1)

- No bundle relay integration (public mempool only).
- Sandwich risk persists on public mempool.
- V2 + Solidly (single-hop) routers supported; V3/CLMM extensions are future work.

## Config Notes

- `dex.min_base_amount` is treated as **raw base units**.
- For native Sonic in `addLiquidityETH`, use `0x0000000000000000000000000000000000000000` as the base token placeholder.
- Set `dex.wrapped_native` to the wrapped native token (wS on Sonic) if you want native-base execution.
- Use `dex.router_factories` to pin each router to its factory and avoid cross-DEX pair mismatches.
- `dex.pair_cache_negative_ttl_ms` controls how quickly newly created pairs can be rediscovered after a miss.
- `dex.sellability_recheck_interval_ms` periodically re-checks disabled routers (0 disables).
- `dex.factory_pair_code_hashes` enables CREATE2 pair derivation when `getPair` misses (use factory `pairCodeHash` or known init code hash).
- `dex.allow_execution_without_pair` lets you execute without a pair/reserve guard (unsafe; for ultra-aggressive snipes).
- `dex.launch_only_liquidity_gate` filters out adds where the pair already had liquidity in the prior block.
- `dex.launch_only_liquidity_gate_mode` (`strict`/`best_effort`) controls behavior when the gate can't be evaluated.
- `dex.verification_tokens` (optional) provides tokens used only for router sellability verification; it does not affect which pairs are traded.
- `strategy.position_store_path` persists open positions and exit signals across restarts.
- `strategy.emergency_reserve_drop_bps` exits if pair reserves drop below the entry baseline by the given bps (0 disables).
- `strategy.emergency_sell_sim_failures` exits after N consecutive sell simulation failures (0 disables).
- `strategy.wait_for_mine` waits for the addLiquidity receipt before execution; tune `strategy.wait_for_mine_poll_interval_ms` and `strategy.wait_for_mine_timeout_ms`.
- `strategy.same_block_attempt` attempts a pre-mine execution before waiting for the addLiquidity receipt; unresolved pairs still defer unless `dex.allow_execution_without_pair = true`.
- `strategy.same_block_requires_reserves` requires non-zero pair reserves before same-block attempts (safer; defaults true).
- `observability.log_format` controls log output format (`pretty` default, `json` for structured logs).
- `observability.base_usd_price` and `observability.base_decimals` (optional) improve human-readable notifier PnL output; leave unset to disable USD formatting.
- `sniper run` validates config (non-empty lists, address formats, bps bounds) and fails fast on invalid settings; it only warns (does not fail startup) if the private key env var or `executor.executor_contract` are missing (executions will be skipped).
- `risk.sell_simulation_mode` (`strict`/`best_effort`) controls whether sell simulation must succeed; best-effort may skip tax checks on override or fee-on-transfer fallback.
- `risk.sell_simulation_override_mode` (`detect`/`skip_any`) controls how override-related RPC errors are classified.
- `risk.trading_control_check` enables paused/trading-enabled/start-time probes; `risk.trading_control_fail_closed` turns block-timestamp failures into high-severity rejects (default false).
- `risk.max_tx_min_supply_bps` and `risk.max_wallet_min_supply_bps` enforce minimum maxTx/maxWallet as bps of total supply; `risk.max_cooldown_secs` caps transfer cooldown seconds (0 disables each).
- `risk.min_lp_burn_bps` / `risk.min_lp_lock_bps` enforce minimum LP burn/lock bps; configure `risk.lp_lockers` with known locker addresses. `risk.lp_lock_burn_mode` controls `any` (default OR) vs `all` (AND) semantics, and `risk.lp_lock_check_mode` controls strict vs best-effort behavior when the pair is unresolved.
- Some Solidly routers (e.g. SwapX RouterV2) do not expose `WETH()/weth()` getters; rely on docs/on-chain transfer traces for the wrapped base token and include wS `0x039e2fB66102314Ce7b64Ce5Ce3E5183bc94aD38` in `dex.base_tokens`.
- Solidly addLiquidity includes a `stable` flag; the bot carries this into pair resolution and execution.
