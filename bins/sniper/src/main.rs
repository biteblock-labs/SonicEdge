use alloy::primitives::{keccak256, Address, B256};
use alloy::providers::Provider;
use anyhow::{anyhow, Result};
use clap::{Parser, Subcommand};
use sonic_bot::Bot;
use sonic_chain::{NodeClient, TxFetcher};
use sonic_core::config::AppConfig;
use sonic_core::utils::parse_address;
use sonic_dex::decode_router_calldata;
use std::path::PathBuf;
use std::str::FromStr;
use tracing::{info, warn};
use tracing_subscriber::{prelude::*, EnvFilter};

#[derive(Parser)]
#[command(name = "sniper", version, about = "SonicEdge mempool sniper")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    Run {
        #[arg(short, long, default_value = "config/sonic.alchemy.toml")]
        config: String,
    },
    DeployContract {
        #[arg(short, long, default_value = "config/sonic.alchemy.toml")]
        config: String,
    },
    TestDecode {
        #[arg(short, long, default_value = "config/sonic.alchemy.toml")]
        config: String,
        #[arg(long)]
        tx: String,
    },
    Replay {
        #[arg(short, long, default_value = "samples/pending_txs.json")]
        file: String,
    },
    PrintConfig {
        #[arg(short, long, default_value = "config/sonic.alchemy.toml")]
        config: String,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    dotenvy::dotenv().ok();
    let cli = Cli::parse();
    match cli.command {
        Commands::Run { config } => {
            let cfg = AppConfig::load(&config)?;
            init_tracing(
                &cfg.observability.log_level,
                true,
                &cfg.observability.log_format,
            );
            for warning in cfg.validate_for_run()? {
                warn!("{warning}");
            }
            let mut bot = Bot::new(cfg).await?;
            bot.run().await?;
        }
        Commands::DeployContract { config } => {
            let cfg = AppConfig::load(&config)?;
            init_tracing(
                &cfg.observability.log_level,
                false,
                &cfg.observability.log_format,
            );
            let abi_path = PathBuf::from("contracts/abi/SonicSniperExecutor.json");
            let abi = std::fs::read_to_string(&abi_path)?;
            let abi_json: serde_json::Value = serde_json::from_str(&abi)?;
            let abi_canonical = serde_json::to_string(&abi_json)?;
            let abi_hash = keccak256(abi_canonical.as_bytes());
            println!("ABI:\n{abi}");
            println!("ABI keccak256 (canonical): 0x{}", hex::encode(abi_hash));

            let executor_address = match parse_address(cfg.executor.executor_contract.trim()) {
                Ok(address) => {
                    println!("Configured executor address: {address}");
                    if address == Address::ZERO {
                        println!("Configured executor address is zero; skipping verification.");
                        None
                    } else {
                        Some(address)
                    }
                }
                Err(err) => {
                    println!("Configured executor address: <invalid>");
                    println!("Address parse error: {err}");
                    None
                }
            };

            let bytecode_path = PathBuf::from("contracts/bytecode/SonicSniperExecutor.hex");
            let local_bytecode_hash = if bytecode_path.exists() {
                let raw = std::fs::read_to_string(bytecode_path)?;
                let bytes = hex::decode(raw.trim().trim_start_matches("0x"))?;
                let hash = keccak256(bytes);
                println!("Bytecode keccak256 (local): 0x{}", hex::encode(hash));
                Some(hash)
            } else {
                println!("Bytecode keccak256 (local): <unavailable>");
                None
            };

            if let Some(address) = executor_address {
                let client = NodeClient::connect(&cfg.chain).await?;
                let code = client.http.get_code_at(address).await?;
                if code.is_empty() {
                    println!("Bytecode keccak256 (on-chain): <empty>");
                    if local_bytecode_hash.is_some() {
                        println!("Bytecode match: false");
                    } else {
                        println!("Bytecode match: <skipped>");
                    }
                } else {
                    let onchain_hash = keccak256(code.as_ref());
                    println!(
                        "Bytecode keccak256 (on-chain): 0x{}",
                        hex::encode(onchain_hash)
                    );
                    if let Some(local_hash) = local_bytecode_hash {
                        println!("Bytecode match: {}", local_hash == onchain_hash);
                    } else {
                        println!("Bytecode match: <skipped>");
                    }
                }
            }
        }
        Commands::TestDecode { config, tx } => {
            let cfg = AppConfig::load(&config)?;
            init_tracing(
                &cfg.observability.log_level,
                false,
                &cfg.observability.log_format,
            );
            let client = sonic_chain::NodeClient::connect(&cfg.chain).await?;
            let fetcher = TxFetcher::new(client.http, cfg.mempool.tx_fetch_timeout_ms);
            let hash = B256::from_str(&tx).map_err(|_| anyhow!("invalid tx hash"))?;
            match fetcher.fetch(hash).await? {
                Some(tx) => {
                    if let Some(call) = decode_router_calldata(&tx.input)? {
                        println!("Decoded: {call:?}");
                    } else {
                        println!("No router call detected");
                    }
                }
                None => println!("Transaction not found"),
            }
        }
        Commands::Replay { file } => {
            init_tracing("info", false, "pretty");
            let data = std::fs::read_to_string(file)?;
            let entries: Vec<ReplayEntry> = serde_json::from_str(&data)?;
            for entry in entries {
                let input = hex::decode(entry.input.trim_start_matches("0x"))?;
                if let Some(call) = decode_router_calldata(&input)? {
                    println!("{} -> {call:?}", entry.hash);
                } else {
                    warn!("no decode for {}", entry.hash);
                }
            }
        }
        Commands::PrintConfig { config } => {
            let cfg = AppConfig::load(&config)?;
            init_tracing(
                &cfg.observability.log_level,
                false,
                &cfg.observability.log_format,
            );
            let json = serde_json::to_string_pretty(&cfg)?;
            println!("{json}");
        }
    }

    info!("done");
    Ok(())
}

fn init_tracing(log_level: &str, log_to_file: bool, log_format: &str) {
    let filter = match std::env::var("RUST_LOG") {
        Ok(value) => EnvFilter::try_new(value).unwrap_or_else(|_| EnvFilter::new("info")),
        Err(_) => EnvFilter::try_new(log_level).unwrap_or_else(|_| EnvFilter::new("info")),
    };
    let use_json = log_format.eq_ignore_ascii_case("json");

    if use_json {
        let stdout_layer = tracing_subscriber::fmt::layer()
            .json()
            .with_ansi(false)
            .with_filter(filter.clone());
        let subscriber = tracing_subscriber::registry().with(stdout_layer);

        if log_to_file {
            let log_path = std::path::Path::new("logs/sniper.log");
            if let Some(parent) = log_path.parent() {
                if let Err(err) = std::fs::create_dir_all(parent) {
                    eprintln!(
                        "failed to create log directory {}: {}",
                        parent.display(),
                        err
                    );
                }
            }
            match std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(log_path)
            {
                Ok(file) => {
                    let file_layer = tracing_subscriber::fmt::layer()
                        .json()
                        .with_ansi(false)
                        .with_writer(file)
                        .with_filter(filter);
                    subscriber.with(file_layer).init();
                    return;
                }
                Err(err) => {
                    eprintln!("failed to open log file {}: {}", log_path.display(), err);
                }
            }
        }

        subscriber.init();
        return;
    }

    let stdout_layer = tracing_subscriber::fmt::layer().with_filter(filter.clone());
    let subscriber = tracing_subscriber::registry().with(stdout_layer);

    if log_to_file {
        let log_path = std::path::Path::new("logs/sniper.log");
        if let Some(parent) = log_path.parent() {
            if let Err(err) = std::fs::create_dir_all(parent) {
                eprintln!(
                    "failed to create log directory {}: {}",
                    parent.display(),
                    err
                );
            }
        }
        match std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(log_path)
        {
            Ok(file) => {
                let file_layer = tracing_subscriber::fmt::layer()
                    .with_ansi(false)
                    .with_writer(file)
                    .with_filter(filter);
                subscriber.with(file_layer).init();
                return;
            }
            Err(err) => {
                eprintln!("failed to open log file {}: {}", log_path.display(), err);
            }
        }
    }

    subscriber.init();
}

#[derive(serde::Deserialize)]
struct ReplayEntry {
    hash: String,
    input: String,
}
