use crate::channel::{tracked_channel, TrackedReceiver};
use crate::metrics::ChannelMetrics;
use crate::reconnect::{next_backoff, ReconnectConfig};
use alloy::primitives::B256;
use alloy::providers::{DynProvider, Provider};
use anyhow::Result;
use futures_util::StreamExt;
use tokio::sync::mpsc::error::TrySendError;
use tokio::time::sleep;
use tracing::{error, info, warn};

pub struct PendingTxStream {
    provider: DynProvider,
    channel_size: usize,
    reconnect: ReconnectConfig,
    metrics: Option<ChannelMetrics>,
}

impl PendingTxStream {
    pub fn new(
        provider: DynProvider,
        channel_size: usize,
        reconnect: ReconnectConfig,
        metrics: Option<ChannelMetrics>,
    ) -> Self {
        Self { provider, channel_size, reconnect, metrics }
    }

    pub async fn spawn(self) -> Result<TrackedReceiver<B256>> {
        let (tx, rx) = tracked_channel(self.channel_size, self.metrics.clone());
        let provider = self.provider;
        let reconnect = self.reconnect;

        tokio::spawn(async move {
            let mut backoff = reconnect.base_delay;
            loop {
                let sub = match provider.subscribe_pending_transactions().await {
                    Ok(sub) => {
                        backoff = reconnect.base_delay;
                        sub
                    }
                    Err(err) => {
                        error!(?err, "pending subscription failed");
                        sleep(backoff).await;
                        backoff = next_backoff(backoff, reconnect.max_delay);
                        continue;
                    }
                };

                let mut stream = sub.into_stream();
                while let Some(hash) = stream.next().await {
                    match tx.try_send(hash) {
                        Ok(()) => {}
                        Err(TrySendError::Full(_)) => {}
                        Err(TrySendError::Closed(_)) => {
                            warn!("pending stream receiver dropped");
                            return;
                        }
                    }
                }

                info!("pending subscription ended; reconnecting");
                sleep(backoff).await;
                backoff = next_backoff(backoff, reconnect.max_delay);
            }
        });

        Ok(rx)
    }
}
