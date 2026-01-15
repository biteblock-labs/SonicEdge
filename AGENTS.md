# Repository Guidelines

## Project Structure & Module Organization
- `crates/` contains Rust libraries: `core` (config/types), `chain` (RPC clients), `dex` (decoder logic), `risk` (filters), `executor` (tx builder), `bot` (orchestrator).
- `bins/sniper/` is the CLI binary crate used to run the bot.
- `config/` holds example configuration (`sonic.example.toml`); copy it for local runs.
- `contracts/` holds `SonicSniperExecutor.sol` and `abi/` artifacts.
- `samples/` contains sample payloads (for example `pending_txs.json`).
- Tests live next to code inside `crates/**` with `#[cfg(test)]`; `target/` is generated output.

## Build, Test, and Development Commands
- `just run` runs the sniper with the example config.
- `just test` runs the full workspace test suite (`cargo test --all`).
- `just fmt` applies rustfmt; `just clippy` runs clippy with warnings denied.
- If `just` is not installed, use:
  - `cargo run -p sniper -- run --config config/sonic.example.toml`
  - `cargo fmt --all`, `cargo clippy --all --all-targets -- -D warnings`
  - `cargo build --all`

## Coding Style & Naming Conventions
- Rust 2021 edition; default rustfmt (4-space indent, trailing commas).
- Naming: `snake_case` for functions/modules, `CamelCase` for types, `SCREAMING_SNAKE_CASE` for constants.
- Keep modules focused by crate; prefer small, testable helpers over large orchestrator methods.
- Maintain modularity: avoid oversized files and “god” modules by splitting logic into focused submodules.

## Testing Guidelines
- Use `cargo test --all` or `just test`.
- Add unit tests near the affected module (`#[cfg(test)]`), especially for decoders, risk checks, and tx building.
- No explicit coverage target; prioritize deterministic inputs and edge cases.
- Maintain proper testing for each important code path and module.

## Commit & Pull Request Guidelines
- Git history is not available here; use clear, imperative summaries (optionally `type: summary`, for example `feat: add risk guard`).
- PRs should include: problem statement, how to reproduce/verify, and test results.
- Call out config changes, chain-specific assumptions, and any key/secret handling impacts.
- If modifying contracts, update `contracts/abi/` and note deployment implications.

## Security & Configuration Tips
- Keep `SNIPER_PK` out of shell history and logs; prefer env files or a secrets manager.
- Run against a local Sonic node with WS + HTTP enabled, as described in `README.md`.
- Do not commit real keys or RPC endpoints.
- Prefer deterministic configuration over hard-coded or derived behavior; expose tunables in config explicitly.
- Never assume on-chain verification is available during development; if something cannot be verified on-chain, ask the user to confirm by searching the web.
