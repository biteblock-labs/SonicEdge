use std::time::Duration;

#[derive(Debug, Clone, Copy)]
pub struct ReconnectConfig {
    pub base_delay: Duration,
    pub max_delay: Duration,
}

impl ReconnectConfig {
    pub fn new(base_ms: u64, max_ms: u64) -> Self {
        Self { base_delay: Duration::from_millis(base_ms), max_delay: Duration::from_millis(max_ms) }
    }
}

pub fn next_backoff(current: Duration, max: Duration) -> Duration {
    let next_ms = current.as_millis().saturating_mul(2) as u64;
    let max_ms = max.as_millis() as u64;
    Duration::from_millis(next_ms.min(max_ms))
}
