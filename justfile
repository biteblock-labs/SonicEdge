fmt:
	cargo fmt --all

clippy:
	cargo clippy --all --all-targets -- -D warnings

test:
	cargo test --all

run:
	cargo run -p sniper -- run --config config/sonic.alchemy.toml
