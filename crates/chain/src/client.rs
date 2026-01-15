use alloy::providers::{DynProvider, Provider, ProviderBuilder};
use anyhow::Result;
use sonic_core::config::ChainConfig;

#[derive(Clone)]
pub struct NodeClient {
    pub ws: DynProvider,
    pub http: DynProvider,
}

impl NodeClient {
    pub async fn connect(cfg: &ChainConfig) -> Result<Self> {
        let ws = ProviderBuilder::new().connect(&cfg.rpc_ws).await?.erased();
        let http = ProviderBuilder::new()
            .connect(&cfg.rpc_http)
            .await?
            .erased();
        Ok(Self { ws, http })
    }
}
