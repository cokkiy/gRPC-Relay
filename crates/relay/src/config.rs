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
    #[serde(default)]
    pub health: HealthConfig,
}

#[derive(Debug, Clone, Deserialize)]
pub struct LoggingConfig {
    #[serde(default = "default_log_level")]
    pub level: String,
    #[serde(default = "default_log_format")]
    pub format: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct HealthConfig {
    #[serde(default = "default_health_enabled")]
    pub enabled: bool,
    #[serde(default = "default_health_address")]
    pub address: String,
    #[serde(default = "default_health_path")]
    pub path: String,
}

impl Default for LoggingConfig {
    fn default() -> Self {
        Self {
            level: default_log_level(),
            format: default_log_format(),
        }
    }
}

impl Default for HealthConfig {
    fn default() -> Self {
        Self {
            enabled: default_health_enabled(),
            address: default_health_address(),
            path: default_health_path(),
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
            .add_source(config::Environment::default().separator("__"))
            .build()?;

        let mut app_config: Self = config.try_deserialize()?;
        app_config.apply_legacy_env_overrides();

        Ok(app_config)
    }

    fn apply_legacy_env_overrides(&mut self) {
        if let Ok(relay_id) = std::env::var("RELAY_ID") {
            self.relay.id = relay_id;
        }

        if let Ok(log_level) = std::env::var("RUST_LOG") {
            self.observability.logging.level = log_level;
        }
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

fn default_health_enabled() -> bool {
    true
}

fn default_health_address() -> String {
    "0.0.0.0:8080".to_string()
}

fn default_health_path() -> String {
    "/health".to_string()
}
