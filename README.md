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
   - `config/sonic.example.toml`
3. Run:

```
just run
```

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
- The executor contract is `Ownable` and NOT upgradeable.
- Approvals should be tight and reset to zero after use.

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
- `dex.factory_pair_code_hashes` enables CREATE2 pair derivation when `getPair` misses (use factory `pairCodeHash` or known init code hash).
- `dex.allow_execution_without_pair` lets you execute without a pair/reserve guard (unsafe; for ultra-aggressive snipes).
- `dex.launch_only_liquidity_gate` filters out adds where the pair already had liquidity in the prior block.
- `dex.launch_only_liquidity_gate_mode` (`strict`/`best_effort`) controls behavior when the gate can't be evaluated.
- `risk.sell_simulation_mode` (`strict`/`best_effort`) controls whether sell simulation must succeed; best-effort may skip tax checks on override or fee-on-transfer fallback.
- `risk.sell_simulation_override_mode` (`detect`/`skip_any`) controls how override-related RPC errors are classified.
- Some Solidly routers (e.g. SwapX RouterV2) do not expose `WETH()/weth()` getters; rely on docs/on-chain transfer traces for the wrapped base token and include wS `0x039e2fB66102314Ce7b64Ce5Ce3E5183bc94aD38` in `dex.base_tokens`.
- Solidly addLiquidity includes a `stable` flag; the bot carries this into pair resolution and execution.
