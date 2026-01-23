#!/usr/bin/env bash
set -euo pipefail

CAST_BIN="${CAST_BIN:-cast}"
ANVIL_HTTP="${ANVIL_HTTP:-http://127.0.0.1:9555}"
PYTHON_BIN="${PYTHON_BIN:-python3}"

DEFAULT_LP_PK="0x59c6995e998f97a5a0044966f0945389dc9e86dae88c7a8412f4603b6b78690d"
LP_PK="${LP_PK:-$DEFAULT_LP_PK}"
LP_ADDRESS="${LP_ADDRESS:-}"
LP_ETH="${LP_ETH:-0x56BC75E2D63100000}"
WHALE_ETH="${WHALE_ETH:-0x56BC75E2D63100000}"

if [[ -z "${ROUTER:-}" ]]; then
  echo "missing ROUTER (router address)" >&2
  exit 1
fi
if [[ -z "${TOKEN:-}" ]]; then
  echo "missing TOKEN (ERC20 address)" >&2
  exit 1
fi
if [[ -z "${WHALE:-}" ]]; then
  echo "missing WHALE (token holder address for impersonation)" >&2
  exit 1
fi
if [[ -z "${TOKEN_AMOUNT:-}" ]]; then
  echo "missing TOKEN_AMOUNT (raw token units)" >&2
  exit 1
fi
if [[ -z "${ETH_AMOUNT:-}" ]]; then
  echo "missing ETH_AMOUNT (raw wei)" >&2
  exit 1
fi

MIN_TOKEN="${MIN_TOKEN:-0}"
MIN_ETH="${MIN_ETH:-0}"
DEADLINE="${DEADLINE:-}"
STABLE="${STABLE:-false}"
SOLIDLY="${SOLIDLY:-0}"

for cmd in "$CAST_BIN" "$PYTHON_BIN"; do
  if ! command -v "$cmd" >/dev/null 2>&1; then
    echo "missing required command: $cmd" >&2
    exit 1
  fi
done

if [[ -z "$DEADLINE" ]]; then
  DEADLINE="$("$PYTHON_BIN" - <<PY
import time
print(int(time.time()) + 300)
PY
)"
fi

if [[ -z "$LP_ADDRESS" ]]; then
  LP_ADDRESS="$("$CAST_BIN" wallet address --private-key "$LP_PK")"
fi

"$CAST_BIN" rpc --rpc-url "$ANVIL_HTTP" anvil_setBalance "$WHALE" "$WHALE_ETH" >/dev/null
"$CAST_BIN" rpc --rpc-url "$ANVIL_HTTP" anvil_setBalance "$LP_ADDRESS" "$LP_ETH" >/dev/null
"$CAST_BIN" rpc --rpc-url "$ANVIL_HTTP" anvil_impersonateAccount "$WHALE" >/dev/null
"$CAST_BIN" send --rpc-url "$ANVIL_HTTP" --from "$WHALE" --unlocked \
  "$TOKEN" "transfer(address,uint256)" "$LP_ADDRESS" "$TOKEN_AMOUNT"
"$CAST_BIN" rpc --rpc-url "$ANVIL_HTTP" anvil_stopImpersonatingAccount "$WHALE" >/dev/null

"$CAST_BIN" send --rpc-url "$ANVIL_HTTP" --private-key "$LP_PK" \
  "$TOKEN" "approve(address,uint256)" "$ROUTER" "$TOKEN_AMOUNT"

if [[ "$SOLIDLY" == "1" ]]; then
  "$CAST_BIN" send --rpc-url "$ANVIL_HTTP" --private-key "$LP_PK" \
    "$ROUTER" "addLiquidityETH(address,bool,uint256,uint256,uint256,address,uint256)" \
    "$TOKEN" "$STABLE" "$TOKEN_AMOUNT" "$MIN_TOKEN" "$MIN_ETH" "$LP_ADDRESS" "$DEADLINE" \
    --value "$ETH_AMOUNT"
else
  "$CAST_BIN" send --rpc-url "$ANVIL_HTTP" --private-key "$LP_PK" \
    "$ROUTER" "addLiquidityETH(address,uint256,uint256,uint256,address,uint256)" \
    "$TOKEN" "$TOKEN_AMOUNT" "$MIN_TOKEN" "$MIN_ETH" "$LP_ADDRESS" "$DEADLINE" \
    --value "$ETH_AMOUNT"
fi
