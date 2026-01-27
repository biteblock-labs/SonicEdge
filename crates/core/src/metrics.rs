#[cfg(feature = "metrics")]
pub struct Metrics {
    registry: prometheus::Registry,
}

#[cfg(feature = "metrics")]
impl Metrics {
    pub fn new() -> Self {
        Self {
            registry: prometheus::Registry::new(),
        }
    }

    pub fn registry(&self) -> &prometheus::Registry {
        &self.registry
    }

    pub fn gather(&self) -> String {
        use prometheus::Encoder;

        let encoder = prometheus::TextEncoder::new();
        let mf = self.registry.gather();
        let mut buf = Vec::new();
        let _ = encoder.encode(&mf, &mut buf);
        String::from_utf8_lossy(&buf).to_string()
    }
}

#[cfg(feature = "metrics")]
impl Default for Metrics {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(not(feature = "metrics"))]
pub struct Metrics;

#[cfg(not(feature = "metrics"))]
impl Metrics {
    pub fn new() -> Self {
        Metrics
    }

    pub fn gather(&self) -> String {
        String::new()
    }
}

#[cfg(not(feature = "metrics"))]
impl Default for Metrics {
    fn default() -> Self {
        Metrics
    }
}
