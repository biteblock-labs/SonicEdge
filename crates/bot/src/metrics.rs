use anyhow::Result;
use prometheus::{IntCounter, Opts};
use sonic_chain::ChannelMetrics;
use sonic_core::metrics::Metrics;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
use std::thread;
use tracing::{info, warn};

pub struct BotMetrics {
    metrics: Metrics,
    pub pending: ChannelMetrics,
    pub txpool: ChannelMetrics,
    pub heads: ChannelMetrics,
    pub dedup_hits: IntCounter,
}

impl BotMetrics {
    pub fn new() -> Result<Self> {
        let metrics = Metrics::new();
        let registry = metrics.registry();
        let pending = ChannelMetrics::new(registry, "pending")?;
        let txpool = ChannelMetrics::new(registry, "txpool")?;
        let heads = ChannelMetrics::new(registry, "heads")?;
        let dedup_hits = IntCounter::with_opts(Opts::new(
            "sonic_mempool_dedup_hits_total",
            "Total duplicate tx hashes filtered by the deduper",
        ))?;
        registry.register(Box::new(dedup_hits.clone()))?;

        Ok(Self {
            metrics,
            pending,
            txpool,
            heads,
            dedup_hits,
        })
    }

    pub fn gather(&self) -> String {
        self.metrics.gather()
    }
}

pub fn spawn_metrics_server(bind: &str, metrics: Arc<BotMetrics>) -> Result<()> {
    let listener = TcpListener::bind(bind)?;
    let bind = bind.to_string();
    thread::spawn(move || {
        info!(%bind, "metrics server listening");
        for stream in listener.incoming() {
            match stream {
                Ok(stream) => {
                    if let Err(err) = handle_connection(stream, &metrics) {
                        warn!(?err, "metrics server connection failed");
                    }
                }
                Err(err) => {
                    warn!(?err, "metrics server accept failed");
                }
            }
        }
    });
    Ok(())
}

fn handle_connection(mut stream: TcpStream, metrics: &BotMetrics) -> Result<()> {
    let mut buffer = [0u8; 512];
    let _ = stream.read(&mut buffer);
    let body = metrics.gather();
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/plain; version=0.0.4\r\nContent-Length: {}\r\n\r\n{}",
        body.len(),
        body
    );
    stream.write_all(response.as_bytes())?;
    Ok(())
}
