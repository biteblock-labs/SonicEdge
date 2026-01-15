use crate::metrics::ChannelMetrics;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio::sync::mpsc::error::TrySendError;

pub struct TrackedSender<T> {
    sender: mpsc::Sender<T>,
    len: Arc<AtomicUsize>,
    metrics: Option<ChannelMetrics>,
}

pub struct TrackedReceiver<T> {
    receiver: mpsc::Receiver<T>,
    len: Arc<AtomicUsize>,
    metrics: Option<ChannelMetrics>,
}

pub fn tracked_channel<T>(
    capacity: usize,
    metrics: Option<ChannelMetrics>,
) -> (TrackedSender<T>, TrackedReceiver<T>) {
    let (sender, receiver) = mpsc::channel(capacity);
    let len = Arc::new(AtomicUsize::new(0));
    let sender = TrackedSender { sender, len: len.clone(), metrics: metrics.clone() };
    let receiver = TrackedReceiver { receiver, len, metrics };
    (sender, receiver)
}

impl<T> TrackedSender<T> {
    pub fn try_send(&self, value: T) -> Result<(), TrySendError<T>> {
        match self.sender.try_send(value) {
            Ok(()) => {
                let len = self.len.fetch_add(1, Ordering::SeqCst) + 1;
                if let Some(metrics) = &self.metrics {
                    metrics.set_queue_depth(len);
                }
                Ok(())
            }
            Err(err) => {
                if matches!(err, TrySendError::Full(_)) {
                    if let Some(metrics) = &self.metrics {
                        metrics.inc_dropped();
                    }
                }
                Err(err)
            }
        }
    }
}

impl<T> TrackedReceiver<T> {
    pub async fn recv(&mut self) -> Option<T> {
        let item = self.receiver.recv().await;
        if item.is_some() {
            let _ = self.len.fetch_update(Ordering::SeqCst, Ordering::SeqCst, |value| {
                value.checked_sub(1)
            });
        }
        let len = self.len.load(Ordering::SeqCst);
        if let Some(metrics) = &self.metrics {
            metrics.set_queue_depth(len);
        }
        item
    }

    pub fn len(&self) -> usize {
        self.len.load(Ordering::SeqCst)
    }
}

#[cfg(test)]
mod tests {
    use super::tracked_channel;

    #[tokio::test]
    async fn tracked_channel_updates_len() {
        let (tx, mut rx) = tracked_channel(2, None);
        assert_eq!(rx.len(), 0);

        tx.try_send(1).unwrap();
        assert_eq!(rx.len(), 1);

        let _ = rx.recv().await;
        assert_eq!(rx.len(), 0);
    }
}
