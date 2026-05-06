use crate::{AppError, Result};
use serde::Deserialize;
use std::{fs, path::Path};

#[derive(Debug, Clone, Deserialize)]
pub struct AppConfig {
    pub relay: RelayConfig,
    #[serde(default)]
    pub observability: ObservabilityConfig,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RelayConfig {
    pub id: String,
    pub address: String,
    #[serde(default = "default_quic_address")]
    pub quic_address: String,
    #[serde(default = "default_max_device_connections")]
    pub max_device_connections: u32,
    #[serde(default = "default_heartbeat_interval_seconds")]
    pub heartbeat_interval_seconds: u64,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct ObservabilityConfig {
    #[serde(default)]
    pub logging: LoggingConfig,
}

#[derive(Debug, Clone, Deserialize)]
pub struct LoggingConfig {
    #[serde(default = "default_log_level")]
    pub level: String,
    #[serde(default = "default_log_format")]
    pub format: String,
}

impl Default for LoggingConfig {
    fn default() -> Self {
        Self {
            level: default_log_level(),
            format: default_log_format(),
        }
    }
}

impl AppConfig {
    pub fn load(path: impl AsRef<Path>) -> Result<Self> {
        let path_ref = path.as_ref();
        let content = fs::read_to_string(path_ref).map_err(|source| AppError::Io {
            path: path_ref.display().to_string(),
            source,
        })?;

        let config = config::Config::builder()
            .add_source(config::File::from_str(&content, config::FileFormat::Yaml))
            .build()?;

        Ok(config.try_deserialize()?)
    }
}

fn default_quic_address() -> String {
    "0.0.0.0:50052".to_string()
}

fn default_max_device_connections() -> u32 {
    10_000
}

fn default_heartbeat_interval_seconds() -> u64 {
    30
}

fn default_log_level() -> String {
    "info".to_string()
}

fn default_log_format() -> String {
    "json".to_string()
}
