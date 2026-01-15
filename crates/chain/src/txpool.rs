use crate::channel::{tracked_channel, TrackedReceiver};
use crate::metrics::ChannelMetrics;
use alloy::network::TransactionResponse;
use alloy::providers::ext::TxPoolApi;
use alloy::providers::DynProvider;
use anyhow::Result;
use tokio::sync::mpsc::error::TrySendError;
use tokio::time::{interval, Duration};
use tracing::{error, warn};

pub struct TxpoolBackfill {
    provider: DynProvider,
    poll_interval: Duration,
    channel_size: usize,
    metrics: Option<ChannelMetrics>,
}

impl TxpoolBackfill {
    pub fn new(
        provider: DynProvider,
        poll_ms: u64,
        channel_size: usize,
        metrics: Option<ChannelMetrics>,
    ) -> Self {
        Self {
            provider,
            poll_interval: Duration::from_millis(poll_ms),
            channel_size,
            metrics,
        }
    }

    pub async fn spawn(self) -> Result<TrackedReceiver<alloy::primitives::B256>> {
        let (tx, rx) = tracked_channel(self.channel_size, self.metrics.clone());
        let provider = self.provider;
        let mut ticker = interval(self.poll_interval);

        tokio::spawn(async move {
            loop {
                ticker.tick().await;
                let content = match provider.txpool_content().await {
                    Ok(content) => content,
                    Err(err) => {
                        warn!(?err, "txpool_content failed");
                        continue;
                    }
                };

                for (_addr, txs) in content
                    .pending
                    .into_iter()
                    .chain(content.queued.into_iter())
                {
                    for (_nonce, tx_obj) in txs {
                        match tx.try_send(tx_obj.tx_hash()) {
                            Ok(()) => {}
                            Err(TrySendError::Full(_)) => {}
                            Err(TrySendError::Closed(_)) => {
                                error!("txpool backfill receiver dropped");
                                return;
                            }
                        }
                    }
                }
            }
        });

        Ok(rx)
    }
}
