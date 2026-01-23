#!/usr/bin/env bash
set -euo pipefail

FULLNODE_HTTP="${FULLNODE_HTTP:-http://127.0.0.1:8545}"
ANVIL_PORT="${ANVIL_PORT:-9555}"
ANVIL_HTTP="http://127.0.0.1:${ANVIL_PORT}"
ANVIL_WS="ws://127.0.0.1:${ANVIL_PORT}"
ANVIL_CHAIN_ID="${ANVIL_CHAIN_ID:-146}"
ANVIL_BLOCK_TIME="${ANVIL_BLOCK_TIME:-1}"
CONFIG_FILE="${CONFIG_FILE:-config/sonic.anvil.toml}"
ANVIL_LOG="${ANVIL_LOG:-logs/anvil.log}"

SOLC_BIN="${SOLC_BIN:-solc}"
FORGE_BIN="${FORGE_BIN:-forge}"
ANVIL_BIN="${ANVIL_BIN:-anvil}"
CAST_BIN="${CAST_BIN:-cast}"
PYTHON_BIN="${PYTHON_BIN:-python3}"
VIA_IR="${VIA_IR:-1}"

DEFAULT_ANVIL_PK="0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80"
DEPLOYER_PK="${DEPLOYER_PK:-$DEFAULT_ANVIL_PK}"
if [[ -z "${SNIPER_PK:-}" ]]; then
  export SNIPER_PK="$DEPLOYER_PK"
fi

for cmd in "$ANVIL_BIN" "$CAST_BIN" "$PYTHON_BIN"; do
  if ! command -v "$cmd" >/dev/null 2>&1; then
    echo "missing required command: $cmd" >&2
    exit 1
  fi
done

use_forge=0
if ! command -v "$SOLC_BIN" >/dev/null 2>&1; then
  if command -v "$FORGE_BIN" >/dev/null 2>&1; then
    use_forge=1
    echo "solc not found; using forge to compile" >&2
  else
    echo "missing required command: $SOLC_BIN (or $FORGE_BIN)" >&2
    exit 1
  fi
fi

if [[ ! -f "$CONFIG_FILE" ]]; then
  echo "missing config file: $CONFIG_FILE" >&2
  exit 1
fi

build_dir="$(mktemp -d)"
cleanup() {
  rm -rf "$build_dir"
  if [[ -n "${ANVIL_PID:-}" ]]; then
    kill "$ANVIL_PID" >/dev/null 2>&1 || true
  fi
}
trap cleanup EXIT

mkdir -p contracts/bytecode contracts/abi logs

VIA_IR_FLAG=""
if [[ "$VIA_IR" == "1" ]]; then
  VIA_IR_FLAG="--via-ir"
fi

if [[ "$use_forge" -eq 0 ]]; then
  "$SOLC_BIN" $VIA_IR_FLAG --optimize --optimize-runs 200 --bin --bin-runtime --abi \
    contracts/SonicSniperExecutor.sol -o "$build_dir"

  cp "$build_dir/SonicSniperExecutor.abi" contracts/abi/SonicSniperExecutor.json
  cp "$build_dir/SonicSniperExecutor.bin-runtime" contracts/bytecode/SonicSniperExecutor.hex

  bytecode_file="$build_dir/SonicSniperExecutor.bin"
  if [[ ! -s "$bytecode_file" ]]; then
    echo "missing bytecode output: $bytecode_file" >&2
    exit 1
  fi
  bytecode="0x$(tr -d '\n' < "$bytecode_file" | sed 's/^0x//')"
else
  forge_out="$build_dir/forge"
  "$FORGE_BIN" build --root . --contracts contracts --out "$forge_out" --optimizer-runs 200 $VIA_IR_FLAG --silent
  forge_json="$forge_out/SonicSniperExecutor.sol/SonicSniperExecutor.json"
  if [[ ! -f "$forge_json" ]]; then
    echo "missing forge output: $forge_json" >&2
    exit 1
  fi
  bytecode="$("$PYTHON_BIN" - "$forge_json" <<'PY'
import json, sys
path = sys.argv[1]
data = json.load(open(path))

def normalize(value):
    if isinstance(value, dict):
        value = value.get("object", "")
    if not isinstance(value, str):
        return ""
    return value

abi = data.get("abi")
creation = normalize(data.get("bytecode", ""))
runtime = normalize(data.get("deployedBytecode", ""))

if not creation or not runtime or abi is None:
    raise SystemExit("missing abi/bytecode in forge output")

def prefix(value):
    return value if value.startswith("0x") else "0x" + value

open("contracts/abi/SonicSniperExecutor.json", "w").write(json.dumps(abi))
open("contracts/bytecode/SonicSniperExecutor.hex", "w").write(prefix(runtime))
print(prefix(creation))
PY
)"
fi

ANVIL_BLOCK_TIME_FLAG=()
if [[ "$ANVIL_BLOCK_TIME" != "0" ]]; then
  ANVIL_BLOCK_TIME_FLAG=(--block-time "$ANVIL_BLOCK_TIME")
fi

"$ANVIL_BIN" --fork-url "$FULLNODE_HTTP" --port "$ANVIL_PORT" --chain-id "$ANVIL_CHAIN_ID" \
  "${ANVIL_BLOCK_TIME_FLAG[@]}" \
  >"$ANVIL_LOG" 2>&1 &
ANVIL_PID=$!

ready=0
chain_hex=""
for _ in {1..120}; do
  if chain_hex="$("$CAST_BIN" rpc --rpc-url "$ANVIL_HTTP" eth_chainId 2>/dev/null)"; then
    chain_hex="${chain_hex//\"/}"
    if [[ -n "$chain_hex" ]]; then
      ready=1
      break
    fi
  fi
  sleep 0.25
done

if [[ "$ready" -ne 1 ]]; then
  echo "anvil did not become ready on $ANVIL_HTTP" >&2
  if [[ -f "$ANVIL_LOG" ]]; then
    tail -n 50 "$ANVIL_LOG" >&2
  fi
  exit 1
fi

if ! chain_id="$("$PYTHON_BIN" - "$chain_hex" <<'PY'
import sys
value = sys.argv[1].strip()
print(int(value, 16))
PY
)"; then
  echo "failed to parse chain id: '$chain_hex'" >&2
  exit 1
fi
if [[ "$chain_id" != "$ANVIL_CHAIN_ID" ]]; then
  echo "anvil chain-id mismatch: expected $ANVIL_CHAIN_ID, got $chain_id" >&2
  exit 1
fi

deploy_json="$("$CAST_BIN" send --json --rpc-url "$ANVIL_HTTP" --private-key "$DEPLOYER_PK" --create "$bytecode")"
executor_address="$("$PYTHON_BIN" - "$deploy_json" <<'PY'
import json, sys
data = json.loads(sys.argv[1])
addr = data.get("contractAddress") or data.get("contract_address")
if not addr:
    raise SystemExit("missing contractAddress in deployment receipt")
print(addr)
PY
)"

"$PYTHON_BIN" - "$CONFIG_FILE" "$executor_address" <<PY
import re, sys
path, addr = sys.argv[1], sys.argv[2]
text = open(path).read()
replacement = f'executor_contract = "{addr}"'
updated, count = re.subn(r'executor_contract\\s*=\\s*\"0x[0-9a-fA-F]{40}\"', replacement, text, count=1)
if count != 1:
    raise SystemExit("failed to update executor_contract in config")
open(path, "w").write(updated)
PY

echo "anvil http: $ANVIL_HTTP"
echo "anvil ws:   $ANVIL_WS"
echo "executor deployed: $executor_address"
echo "config updated: $CONFIG_FILE"
echo "run bot: cargo run -p sniper -- run --config $CONFIG_FILE"
echo "submit liquidity: scripts/submit_liquidity_eth.sh (see script for env vars)"

if [[ "${KEEP_ANVIL:-1}" == "1" ]]; then
  echo "anvil running (pid $ANVIL_PID); press Ctrl+C to stop"
  wait "$ANVIL_PID"
fi
