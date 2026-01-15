use alloy::primitives::{keccak256, B256};
use anyhow::{anyhow, Result};
use clap::{Parser, Subcommand};
use sonic_bot::Bot;
use sonic_chain::TxFetcher;
use sonic_core::config::AppConfig;
use sonic_core::utils::parse_address;
use sonic_dex::decode_router_calldata;
use std::path::PathBuf;
use std::str::FromStr;
use tracing::{info, warn};
use tracing_subscriber::EnvFilter;

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
    let cli = Cli::parse();
    match cli.command {
        Commands::Run { config } => {
            let cfg = AppConfig::load(&config)?;
            init_tracing(&cfg.observability.log_level);
            let mut bot = Bot::new(cfg).await?;
            bot.run().await?;
        }
        Commands::DeployContract { config } => {
            let cfg = AppConfig::load(&config)?;
            init_tracing(&cfg.observability.log_level);
            let abi_path = PathBuf::from("contracts/abi/SonicSniperExecutor.json");
            let abi = std::fs::read_to_string(&abi_path)?;
            println!("ABI:\n{abi}");

            if let Ok(address) = parse_address(&cfg.executor.executor_contract) {
                println!("Configured executor address: {address}");
            } else {
                println!("Configured executor address: <invalid>");
            }

            let bytecode_path = PathBuf::from("contracts/bytecode/SonicSniperExecutor.hex");
            if bytecode_path.exists() {
                let raw = std::fs::read_to_string(bytecode_path)?;
                let bytes = hex::decode(raw.trim_start_matches("0x"))?;
                let hash = keccak256(bytes);
                println!("Bytecode keccak256: 0x{}", hex::encode(hash));
            } else {
                println!("Bytecode hash: <unavailable>");
            }
        }
        Commands::TestDecode { config, tx } => {
            let cfg = AppConfig::load(&config)?;
            init_tracing(&cfg.observability.log_level);
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
            init_tracing("info");
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
            init_tracing(&cfg.observability.log_level);
            let json = serde_json::to_string_pretty(&cfg)?;
            println!("{json}");
        }
    }

    info!("done");
    Ok(())
}

fn init_tracing(log_level: &str) {
    let filter = match std::env::var("RUST_LOG") {
        Ok(value) => EnvFilter::try_new(value).unwrap_or_else(|_| EnvFilter::new("info")),
        Err(_) => EnvFilter::try_new(log_level).unwrap_or_else(|_| EnvFilter::new("info")),
    };
    tracing_subscriber::fmt().with_env_filter(filter).init();
}

#[derive(serde::Deserialize)]
struct ReplayEntry {
    hash: String,
    input: String,
}
