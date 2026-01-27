use crate::channel::{tracked_channel, TrackedReceiver};
use crate::metrics::ChannelMetrics;
use crate::reconnect::{next_backoff, ReconnectConfig};
use alloy::providers::{DynProvider, Provider};
use anyhow::Result;
use futures_util::StreamExt;
use tokio::sync::mpsc::error::TrySendError;
use tokio::time::sleep;
use tracing::{error, info, warn};

pub struct NewHeadStream {
    provider: DynProvider,
    channel_size: usize,
    reconnect: ReconnectConfig,
    metrics: Option<ChannelMetrics>,
}

impl NewHeadStream {
    pub fn new(
        provider: DynProvider,
        channel_size: usize,
        reconnect: ReconnectConfig,
        metrics: Option<ChannelMetrics>,
    ) -> Self {
        Self {
            provider,
            channel_size,
            reconnect,
            metrics,
        }
    }

    pub async fn spawn(self) -> Result<TrackedReceiver<u64>> {
        let (tx, rx) = tracked_channel(self.channel_size, self.metrics.clone());
        let provider = self.provider;
        let reconnect = self.reconnect;

        tokio::spawn(async move {
            let mut backoff = reconnect.base_delay;
            loop {
                let sub = match provider.subscribe_blocks().await {
                    Ok(sub) => {
                        backoff = reconnect.base_delay;
                        sub
                    }
                    Err(err) => {
                        error!(?err, "new heads subscription failed");
                        sleep(backoff).await;
                        backoff = next_backoff(backoff, reconnect.max_delay);
                        continue;
                    }
                };

                let mut stream = sub.into_stream();
                while let Some(header) = stream.next().await {
                    let number = header.inner.number;
                    match tx.try_send(number) {
                        Ok(()) => {}
                        Err(TrySendError::Full(_)) => {}
                        Err(TrySendError::Closed(_)) => {
                            warn!("new heads receiver dropped");
                            return;
                        }
                    }
                }

                info!("new heads subscription ended; reconnecting");
                sleep(backoff).await;
                backoff = next_backoff(backoff, reconnect.max_delay);
            }
        });

        Ok(rx)
    }
}
