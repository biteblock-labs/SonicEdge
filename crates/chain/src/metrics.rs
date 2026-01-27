use anyhow::Result;
use prometheus::{IntCounter, IntGauge, Opts, Registry};

#[derive(Clone)]
pub struct ChannelMetrics {
    queue_depth: IntGauge,
    dropped_total: IntCounter,
}

impl ChannelMetrics {
    pub fn new(registry: &Registry, name: &str) -> Result<Self> {
        let queue_depth = IntGauge::with_opts(Opts::new(
            format!("sonic_mempool_{name}_queue_depth"),
            "Current queue depth for the stream",
        ))?;
        let dropped_total = IntCounter::with_opts(Opts::new(
            format!("sonic_mempool_{name}_dropped_total"),
            "Total dropped items due to backpressure",
        ))?;

        registry.register(Box::new(queue_depth.clone()))?;
        registry.register(Box::new(dropped_total.clone()))?;

        Ok(Self {
            queue_depth,
            dropped_total,
        })
    }

    pub fn set_queue_depth(&self, depth: usize) {
        self.queue_depth.set(depth as i64);
    }

    pub fn inc_dropped(&self) {
        self.dropped_total.inc();
    }
}
