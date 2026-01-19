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
- Start from `config/sonic.example.toml`.
- Set `dex.routers`, `dex.factories`, and `dex.base_tokens`.
- Prefer `dex.router_factories` to pin each router to its factory and avoid mismatches.
- Populate `dex.factory_pair_code_hashes` to enable CREATE2 pair derivation on `getPair` misses.
- Keep `dex.allow_execution_without_pair = false` unless you intentionally want to skip reserve guards.
- Keep `dex.launch_only_liquidity_gate = true` to ignore non-launch liquidity adds.
- Set `dex.launch_only_liquidity_gate_mode` to `strict` (default) or `best_effort` if your node can't serve prior-block state.
- Set `dex.wrapped_native` (wS on Sonic) and include `0x0000000000000000000000000000000000000000` in `dex.base_tokens` to enable native-base execution.
- Some Solidly routers (e.g. SwapX RouterV2) do not expose `WETH()/weth()` getters; verify the wrapped base token via docs or transfer traces (wS on Sonic mainnet).
- Set `executor.executor_contract` to your deployed address.
- Keep `min_base_amount` in raw base units.
- Set `risk.sell_simulation_mode` (`strict`/`best_effort`) and `risk.sell_simulation_override_mode` (`detect`/`skip_any`) to tune sell simulation enforcement.

## Secrets
- Export the hot wallet key:

```
export SNIPER_PK=0x<hex_private_key>
```

- Avoid storing keys in shell history or world-readable files.

## Deploy Contract
- Compile and deploy `contracts/SonicSniperExecutor.sol` from your hot wallet.
- Approve the executor to spend base token for the owner wallet.
- Optional: call `setUseFeeOnTransfer(true)` for fee-on-transfer routers.
- If you need Solidly/native execution, redeploy after updating the ABI to include `buySolidly`/`buySolidlyETH`/`buyV2ETH`.

## Run
```
just run
```

## Health Checks
- Verify WS subscriptions are live (logs show pending hashes and new heads).
- Confirm txpool backfill polling is active when `mempool.mode = "ws+txpool"`.
- Validate bot can resolve routers and decode addLiquidity calls.
- If metrics are enabled, confirm `/metrics` responds on `observability.metrics_bind`.

## Monitoring
- Tail logs for decode/risk/execute decisions.
- If metrics enabled, scrape the Prometheus endpoint from `observability.metrics_bind`.
- Metrics include queue depth, drop counts, and dedup hits for mempool ingestion.

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
