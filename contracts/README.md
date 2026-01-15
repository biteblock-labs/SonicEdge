# SonicSniperExecutor

Minimal on-chain executor for atomic UniswapV2-style swaps with safety guards.

## Build

- Compile with Solidity `0.8.20+`.
- Example (solc):

```
solc --optimize --optimize-runs 200 --bin --abi SonicSniperExecutor.sol -o build
```

## Deploy

1. Deploy the contract from the hot wallet used by the bot (this becomes `owner`).
2. (Optional) Call `setUseFeeOnTransfer(true)` if you want to use the supporting-fee router method.
3. Approve the executor to spend base tokens on the owner wallet.

## Usage

- `buyV2` performs an atomic UniswapV2-style swap with optional reserve and block guards.
- `buySolidly` performs a Solidly-style swap (route with stable/volatile flag) with the same guards.
- Use `rescueToken` and `rescueETH` for emergency recovery.

## Notes

- The contract is **not** upgradeable.
- Keep the owner key offline except for bot operations.
- Any ABI change (like adding `buySolidly`) requires redeploying the executor and updating `contracts/abi/`.
