# SonicSniperExecutor

Minimal on-chain executor for atomic UniswapV2-style swaps with safety guards.

## Build

- Compile with Solidity `0.8.20+`.
- Example (solc):

```
solc --optimize --optimize-runs 200 --bin --bin-runtime --abi SonicSniperExecutor.sol -o build
```

- The runtime bytecode (`*.bin-runtime`) is what `sniper deploy-contract` compares against on-chain code; copy it to `contracts/bytecode/SonicSniperExecutor.hex` if you want local match checks.
- `sniper deploy-contract` prints a canonical ABI hash using `contracts/abi/SonicSniperExecutor.json`.

## Deploy

1. Deploy the contract from the hot wallet used by the bot (this becomes `owner`).
2. (Optional) Call `setUseFeeOnTransfer(true)` if you want to use the supporting-fee router method.
3. Approve the executor to spend base tokens on the owner wallet.

## Usage

- `buyV2` performs an atomic UniswapV2-style swap with optional reserve and block guards.
- `buyV2ETH` performs a native-asset swap using `msg.value` (expects `amountIn == msg.value`).
- `buySolidly` performs a Solidly-style swap (route with stable/volatile flag) with the same guards.
- `buySolidlyETH` performs a Solidly native-asset swap using `msg.value` (expects `amountIn == msg.value`).
- Use `rescueToken` and `rescueETH` for emergency recovery.

## Notes

- The contract is **not** upgradeable.
- Keep the owner key offline except for bot operations.
- Any ABI change (like adding `buySolidly`) requires redeploying the executor and updating `contracts/abi/`.

## Test Tokens (Anvil)

- `ZooLimitToken.sol` is used to exercise maxTx/maxWallet/cooldown checks.
- Current zoo limit tokens (addresses reset when Anvil restarts):
  - ZOK `0xfbC22278A96299D91d41C453234d97b4F5Eb9B2d` (pass)
  - ZST `0x46b142DD1E924FAb83eCc3c08e4D46E82f005e0E` (fail maxTx/maxWallet)
  - ZCD `0xC9a43158891282A2B1475592D5719c001986Aaec` (fail cooldown)
