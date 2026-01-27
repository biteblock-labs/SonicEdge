# SonicEdge Ops Runbook

Operational guide for running the SonicEdge bot safely and reliably.

## Prerequisites
- Sonic full node (`go-ethereum-sonic`) with:
  - WebSocket enabled for pubsub
  - HTTP RPC enabled for calls
  - Txpool API enabled
- A funded hot wallet for execution
- Deployed `SonicSniperExecutor` contract

## Node Configuration (Example)
Adjust to your environment and security requirements.

```
geth \
  --http --http.addr 127.0.0.1 --http.port 8545 \
  --http.api eth,net,web3,txpool \
  --ws --ws.addr 127.0.0.1 --ws.port 8546 \
  --ws.api eth,net,web3,txpool \
  --txpool.globalslots 4096 \
  --txpool.globalqueue 2048
```

## Configuration
- Start from `config/sonic.example.toml` (template) or `config/sonic.alchemy.toml` (mainnet-ready allowlists).
- Set `dex.routers`, `dex.factories`, and `dex.base_tokens`.
- Prefer `dex.router_factories` to pin each router to its factory and avoid mismatches.
- Populate `dex.factory_pair_code_hashes` to enable CREATE2 pair derivation on `getPair` misses.
- Set `dex.sellability_recheck_interval_ms` to periodically re-check disabled routers (0 disables).
- Keep `dex.allow_execution_without_pair = false` unless you intentionally want to skip reserve guards.
- Keep `dex.launch_only_liquidity_gate = true` to ignore non-launch liquidity adds.
- Set `dex.launch_only_liquidity_gate_mode` to `strict` (default) or `best_effort` if your node can't serve prior-block state.
- Set `strategy.position_store_path` to persist open positions and exit signals across restarts.
- Keep `strategy.wait_for_mine = true` for the primary execution path; tune `strategy.wait_for_mine_poll_interval_ms` and `strategy.wait_for_mine_timeout_ms` for responsiveness.
- Set `strategy.same_block_attempt = true` to attempt same-block execution before the receipt path; pairs unresolved pre-mine still defer unless `dex.allow_execution_without_pair = true`.
- Keep `strategy.same_block_requires_reserves = true` to avoid pre-mine buys before reserves are set (safer default).
- Set `strategy.emergency_reserve_drop_bps` to exit if reserves drop below the entry baseline (0 disables).
- Set `strategy.emergency_sell_sim_failures` to exit after consecutive sell simulation failures (0 disables).
- Set `strategy.buy_amount_mode` to `liquidity`, `fixed`, or `wallet_pct`, then tune `strategy.buy_amount_min`/`max` and `strategy.buy_amount_wallet_bps` to control per-trade sizing; for native-base buys, set `strategy.buy_amount_native_reserve` to keep gas available.
- Set `strategy.position_log_interval_ms` to emit periodic position snapshots (entry/current price, TP/SL bands, PnL direction).
- Set `strategy.buy_amount_unavailable_retry_ttl_ms` to cap retries after balance RPC errors (0 disables).
- Set `observability.log_format` to `json` for structured logs (`pretty` default).
- Bind `observability.metrics_bind` to an internal interface only (for example `127.0.0.1:9102`) or disable metrics.
- Set `dex.wrapped_native` (wS on Sonic) and include `0x0000000000000000000000000000000000000000` in `dex.base_tokens` to enable native-base execution.
- Some Solidly routers (e.g. SwapX RouterV2) do not expose `WETH()/weth()` getters; verify the wrapped base token via docs or transfer traces (wS on Sonic mainnet).
- Set `executor.executor_contract` to your deployed address.
- Set `executor.gas_limit_buffer_bps` to add a safety margin to gas estimates (0 disables).
- `sniper run` validates required lists, address formats, and bps bounds (invalid config fails fast); it only warns (does not fail startup) if the private key env var or `executor.executor_contract` are missing (executions will be skipped).
- Keep `min_base_amount` in raw base units.
- Set `risk.sell_simulation_mode` (`strict`/`best_effort`) and `risk.sell_simulation_override_mode` (`detect`/`skip_any`) to tune sell simulation enforcement.
- Set `risk.trading_control_check = true` to reject tokens with paused/disabled trading or future start-time gates; `risk.trading_control_fail_closed = true` makes start-time failures fatal (default false).
- Set `risk.max_tx_min_supply_bps`, `risk.max_wallet_min_supply_bps`, and `risk.max_cooldown_secs` to enforce maxTx/maxWallet/cooldown thresholds (0 disables each).
- Set `risk.min_lp_burn_bps` / `risk.min_lp_lock_bps` to enforce minimum LP burn/lock bps; configure `risk.lp_lockers` with known locker addresses. Use `risk.lp_lock_burn_mode = "any"` (default, OR) or `"all"` (AND), and `risk.lp_lock_check_mode = "best_effort"` if you want to skip the check when the pair is unresolved.
- Raise `risk.max_tax_bps` if you intend to trade fee-on-transfer tokens with higher taxes.
- For illiquid launches, consider loosening the limits (lower `max_tx_min_supply_bps`/`max_wallet_min_supply_bps`, higher `max_cooldown_secs`, or `trading_control_fail_closed = false`).
- Use `risk.token_override_slots` to define non-standard ERC20 storage layouts (balance/allowance slots) so sell simulation and quote overrides work on tokens like USDC.

## Secrets
- Create a `.env` file and set the hot wallet key:

```
SNIPER_PK=0x<hex_private_key>
```

- Optional: enable Telegram alerts:

```
TELEGRAM_BOT_TOKEN=<bot_token>
TELEGRAM_CHAT_ID=<chat_id>
```

- The CLI loads `.env` automatically. Prefer `0600` permissions (or a secrets manager) and reference it from systemd; never store private keys in config files or logs.

## Foundry Tests (Contracts)
```
forge test
```

## Deploy Contract
- Compile and deploy `contracts/SonicSniperExecutor.sol` from your hot wallet.
- To refresh artifacts from Foundry:
  - `forge inspect SonicSniperExecutor abi --json > contracts/abi/SonicSniperExecutor.json`
  - `forge inspect SonicSniperExecutor deployedBytecode > contracts/bytecode/SonicSniperExecutor.hex`
- Approve the executor to spend base token for the owner wallet.
- Optional: set `executor.auto_approve_mode = "max"` (or `"exact"`) to let the bot send base-token approvals on demand (the bot will reset allowance to 0 first when a nonzero allowance exists).
- Buys send tokens to the executor contract; no extra approval is needed to sell those positions.
- Optional: call `setUseFeeOnTransfer(true)` for fee-on-transfer routers.
- If you need Solidly/native execution, redeploy after updating the ABI to include `buySolidly`/`buySolidlyETH`/`buyV2ETH`.
- Run `sniper deploy-contract` to print the canonical ABI hash and verify the configured executor address bytecode against `contracts/bytecode/SonicSniperExecutor.hex` when available.
- ABI hashes still change if the ABI entry order changes; treat the hash as a local fingerprint, not a cross-tool guarantee.

## Local Fork Pipeline Test
- Run an anvil fork on a different port than the full node (example uses `9555`) and keep chain id `146`:

```
anvil --fork-url http://127.0.0.1:8545 --port 9555 --chain-id 146
```

- Use `config/sonic.anvil.toml` so HTTP points at `http://127.0.0.1:9555` and WS points at `ws://127.0.0.1:9555`.
- Keep `mempool.mode = "ws"` on the fork (anvil does not support `txpool_content`).
- Deploy the executor on the fork, fund the hot wallet, and run the bot with the anvil config to exercise the full flow.
- Optional helper scripts: `scripts/anvil_pipeline.sh` (compile/deploy + in-place config update) and `scripts/submit_liquidity_eth.sh` (send a forked addLiquidityETH tx).
- `scripts/submit_liquidity_eth.sh` defaults to the second anvil account as the LP wallet (`LP_PK`), and still needs a forked `WHALE` holder address to transfer tokens in.
- This anvil flow is the chosen full-pipeline verification approach before mainnet runs.

## Token Zoo Risk Harness (Anvil)
- Use a small set of test tokens to deterministically hit risk paths: standard ERC20, fee-on-transfer, honeypot/blacklist (revert on transfer), bad metadata (empty/long name/symbol), and a non-standard storage token (USDC-style).
- Deploy tokens to the fork, mint to the LP account, then add liquidity via `scripts/submit_liquidity_eth.sh` (Shadow router).
- Recommended risk settings for clear signals: `risk.sell_simulation_mode = "strict"`, `risk.sellability_amount_base = "1000000000000000"`, and `risk.max_tax_bps = 500`.
- To exercise trading toggles and limit heuristics, enable `risk.trading_control_check = true` and set thresholds like `risk.max_tx_min_supply_bps = 200`, `risk.max_wallet_min_supply_bps = 300`, and `risk.max_cooldown_secs = 60`.
- Expected outcomes: standard passes + buy/exit; fee-on-transfer rejected on tax; honeypot rejected on sell simulation; bad metadata rejected on ERC20 sanity; non-standard storage passes when `risk.token_override_slots` is configured.
- Zoo limit tokens (ZOK/ZST/ZCD) are listed below with expected behavior and liquidity commands.

### Zoo limit tokens (local Anvil)
These are simple ERC20s with explicit limit getters so we can trigger the new maxTx/maxWallet/cooldown checks. They are deployed on the local Anvil fork; addresses reset when Anvil restarts.

- ZOK (should pass): `0xfbC22278A96299D91d41C453234d97b4F5Eb9B2d`
  - totalSupply=1e24, maxTx=5e22 (5%), maxWallet=1e23 (10%), cooldown=30s
- ZST (should fail maxTx/maxWallet): `0x46b142DD1E924FAb83eCc3c08e4D46E82f005e0E`
  - totalSupply=1e24, maxTx=5e21 (0.5%), maxWallet=1e22 (1%), cooldown=30s
- ZCD (should fail cooldown): `0xC9a43158891282A2B1475592D5719c001986Aaec`
  - totalSupply=1e24, maxTx=5e22 (5%), maxWallet=1e23 (10%), cooldown=120s

Example liquidity adds (Shadow router):
```
export ROUTER=0x1D368773735ee1E678950B7A97bcA2CafB330CDc
export WHALE=0xf39fd6e51aad88f6f4ce6ab8827279cfffb92266
export TOKEN_AMOUNT=10000000000000000000000
export ETH_AMOUNT=1000000000000000000

TOKEN=0xfbC22278A96299D91d41C453234d97b4F5Eb9B2d scripts/submit_liquidity_eth.sh
TOKEN=0x46b142DD1E924FAb83eCc3c08e4D46E82f005e0E scripts/submit_liquidity_eth.sh
TOKEN=0xC9a43158891282A2B1475592D5719c001986Aaec scripts/submit_liquidity_eth.sh
```

## Run
```
just run
```
`just run` uses `config/sonic.alchemy.toml` by default (see `justfile`).

## Production Service (systemd + logrotate)
- Systemd unit template: `scripts/systemd/sonicedge.service`
- Logrotate config: `scripts/logrotate/sonicedge`
- Build the release binary before enabling the service:
  - `cargo build --release -p sniper`

## Health Checks
- Verify WS subscriptions are live (logs show pending hashes and new heads).
- Confirm txpool backfill polling is active when `mempool.mode = "ws+txpool"`.
- Validate bot can resolve routers and decode addLiquidity calls.
- If metrics are enabled, confirm `/metrics` responds on `observability.metrics_bind`.

## Monitoring
- Tail logs for decode/risk/execute decisions.
- If metrics enabled, scrape the Prometheus endpoint from `observability.metrics_bind`.
- Metrics include queue depth, drop counts, and dedup hits for mempool ingestion.
- Telegram notifications include SonicScan tx links for entry/exit confirmations (when enabled).

## Common Issues
- **No pending txs:** check WS endpoint, node WS config, or firewall.
- **Tx fetch timeouts:** increase `tx_fetch_timeout_ms`.
- **Missing liquidity adds:** ensure router list is correct, enable txpool backfill.
- **Launch gate skips:** if you see historical state errors, set `dex.launch_only_liquidity_gate_mode = "best_effort"` or run a node that can serve prior-block state.
- **Execution failures:** check approvals, contract address, gas settings.

## Emergency Actions
- Stop the bot process immediately.
- Revoke token approvals from the executor contract if compromised.
- Call `rescueToken` / `rescueETH` to recover funds.
- Rotate `SNIPER_PK` and redeploy executor if necessary.

## Maintenance
- Periodically resync nonce and balance checks.
- Update router/factory allowlists as DEX deployments change.
- Review risk parameters after market conditions shift.
