use crate::error::{ControllerSdkError, Result};
use serde::Deserialize;
use std::sync::Arc;

pub trait ControllerTokenProvider: Send + Sync {
    fn token(&self) -> Result<String>;
}

#[derive(Debug, Clone)]
pub struct StaticTokenProvider {
    token: String,
}

impl StaticTokenProvider {
    pub fn new(token: impl Into<String>) -> Self {
        Self {
            token: token.into(),
        }
    }
}

impl ControllerTokenProvider for StaticTokenProvider {
    fn token(&self) -> Result<String> {
        Ok(self.token.clone())
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct ControllerSdkConfig {
    /// Relay server address for Controller -> Relay gRPC over HTTP/2 (TLS).
    ///
    /// Examples:
    /// - "relay.example.com:50051" (will be normalized to "https://...")
    /// - "https://relay.example.com:50051" (used as-is)
    pub relay_address: String,

    pub controller_id: String,

    /// Token provider (MVP: static token; refresh/rotation can be added later).
    pub token: String,

    #[serde(default = "default_max_payload_bytes")]
    pub max_payload_bytes: usize,
}

impl ControllerSdkConfig {
    pub fn validate(&self) -> Result<()> {
        if self.relay_address.trim().is_empty() {
            return Err(ControllerSdkError::InvalidConfig(
                "relay_address must not be empty".into(),
            ));
        }
        if self.controller_id.trim().is_empty() {
            return Err(ControllerSdkError::InvalidConfig(
                "controller_id must not be empty".into(),
            ));
        }
        if self.token.trim().is_empty() {
            return Err(ControllerSdkError::InvalidConfig(
                "token must not be empty".into(),
            ));
        }
        if self.max_payload_bytes == 0 {
            return Err(ControllerSdkError::InvalidConfig(
                "max_payload_bytes must be > 0".into(),
            ));
        }
        Ok(())
    }

    pub fn token_provider(&self) -> Arc<dyn ControllerTokenProvider> {
        Arc::new(StaticTokenProvider::new(self.token.clone()))
    }

    pub fn normalized_endpoint(&self) -> Result<String> {
        // tonic Endpoint needs scheme
        let addr = self.relay_address.trim();
        if addr.starts_with("http://") || addr.starts_with("https://") {
            Ok(addr.to_string())
        } else {
            Ok(format!("https://{addr}"))
        }
    }

    pub fn from_env() -> Result<Self> {
        let relay_address = std::env::var("RELAY_ADDRESS")
            .map_err(|_| ControllerSdkError::InvalidConfig("RELAY_ADDRESS is required".into()))?;
        let controller_id = std::env::var("CONTROLLER_ID")
            .map_err(|_| ControllerSdkError::InvalidConfig("CONTROLLER_ID is required".into()))?;
        let token = std::env::var("CONTROLLER_TOKEN").map_err(|_| {
            ControllerSdkError::InvalidConfig("CONTROLLER_TOKEN is required".into())
        })?;

        let max_payload_bytes = std::env::var("MAX_PAYLOAD_BYTES")
            .ok()
            .and_then(|v| v.parse::<usize>().ok())
            .unwrap_or(default_max_payload_bytes());

        let cfg = Self {
            relay_address,
            controller_id,
            token,
            max_payload_bytes,
        };
        cfg.validate()?;
        Ok(cfg)
    }
}

fn default_max_payload_bytes() -> usize {
    10 * 1024 * 1024 // 10MB
}
