use axum::{routing::get, Json, Router};
use serde::Serialize;
use std::{net::SocketAddr, time::Instant};
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;
use tracing::info;

use crate::{config::HealthConfig, AppError, Result};

#[derive(Clone)]
pub struct HealthState {
    started_at: Instant,
    version: &'static str,
}

impl HealthState {
    pub fn new(version: &'static str) -> Self {
        Self {
            started_at: Instant::now(),
            version,
        }
    }
}

#[derive(Serialize)]
struct ComponentHealth {
    status: &'static str,
    message: &'static str,
}

#[derive(Serialize)]
struct HealthMetrics {
    active_device_connections: u64,
    active_controller_connections: u64,
    active_streams: u64,
    cpu_usage_percent: f64,
    memory_usage_percent: f64,
}

#[derive(Serialize)]
struct HealthResponse {
    status: &'static str,
    timestamp: String,
    uptime_seconds: u64,
    version: &'static str,
    components: HealthComponents,
    metrics: HealthMetrics,
}

#[derive(Serialize)]
struct HealthComponents {
    grpc_server: ComponentHealth,
    quic_listener: ComponentHealth,
    mqtt_client: ComponentHealth,
    auth_service: ComponentHealth,
    metrics_collector: ComponentHealth,
}

impl HealthComponents {
    fn statuses(&self) -> [&'static str; 5] {
        [
            self.grpc_server.status,
            self.quic_listener.status,
            self.mqtt_client.status,
            self.auth_service.status,
            self.metrics_collector.status,
        ]
    }
}

fn derive_overall_status(components: &HealthComponents) -> &'static str {
    let mut has_degraded = false;

    for status in components.statuses() {
        match status {
            "healthy" => {}
            "degraded" => has_degraded = true,
            "unhealthy" => return "unhealthy",
            _ => return "unhealthy",
        }
    }

    if has_degraded { "degraded" } else { "healthy" }
}

pub async fn serve_health(config: HealthConfig, version: &'static str) -> Result<()> {
    if !config.enabled {
        info!("health server disabled");
        return Ok(());
    }

    let address = config
        .address
        .parse::<SocketAddr>()
        .map_err(|source| AppError::InvalidSocketAddress {
            address: config.address.clone(),
            source,
        })?;

    let state = HealthState::new(version);
    let app = Router::new()
        .route(&config.path, get(health))
        .with_state(state);

    info!(
        health_address = %address,
        health_path = %config.path,
        "health server listening"
    );

    axum::Server::try_bind(&address)
        .map_err(|source| AppError::HealthBind { address, source })?
        .serve(app.into_make_service())
        .await?;

    Ok(())
}

async fn health(
    axum::extract::State(state): axum::extract::State<HealthState>,
) -> Json<HealthResponse> {
    // MVP skeleton：目前只有 health 服务本身，其他组件尚未落地。
    // 为了满足协议契约，这里返回结构完整的 response，并在字段里标明未实现/不可用。
    let timestamp = OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_string());

    let components = HealthComponents {
        grpc_server: ComponentHealth {
            status: "unhealthy",
            message: "gRPC server not implemented (MVP skeleton)",
        },
        quic_listener: ComponentHealth {
            status: "unhealthy",
            message: "QUIC listener not implemented (MVP skeleton)",
        },
        mqtt_client: ComponentHealth {
            status: "unhealthy",
            message: "MQTT client not implemented (MVP skeleton)",
        },
        auth_service: ComponentHealth {
            status: "unhealthy",
            message: "auth service not implemented (MVP skeleton)",
        },
        metrics_collector: ComponentHealth {
            status: "unhealthy",
            message: "metrics collector not implemented (MVP skeleton)",
        },
    };

    let response = HealthResponse {
        status: derive_overall_status(&components),
        timestamp,
        uptime_seconds: state.started_at.elapsed().as_secs(),
        version: state.version,
        components,
        metrics: HealthMetrics {
            active_device_connections: 0,
            active_controller_connections: 0,
            active_streams: 0,
            cpu_usage_percent: 0.0,
            memory_usage_percent: 0.0,
        },
    };

    Json(response)
}

#[cfg(test)]
mod tests {
    use super::{derive_overall_status, ComponentHealth, HealthComponents};

    fn component(status: &'static str) -> ComponentHealth {
        ComponentHealth {
            status,
            message: "test",
        }
    }

    fn components(status: &'static str) -> HealthComponents {
        HealthComponents {
            grpc_server: component(status),
            quic_listener: component(status),
            mqtt_client: component(status),
            auth_service: component(status),
            metrics_collector: component(status),
        }
    }

    #[test]
    fn derive_status_is_unhealthy_when_any_component_is_unhealthy() {
        let mut value = components("healthy");
        value.mqtt_client = component("unhealthy");

        assert_eq!(derive_overall_status(&value), "unhealthy");
    }

    #[test]
    fn derive_status_is_degraded_when_no_component_is_unhealthy() {
        let mut value = components("healthy");
        value.auth_service = component("degraded");

        assert_eq!(derive_overall_status(&value), "degraded");
    }

    #[test]
    fn derive_status_is_healthy_when_all_components_are_healthy() {
        let value = components("healthy");

        assert_eq!(derive_overall_status(&value), "healthy");
    }

    #[test]
    fn derive_status_treats_unknown_component_status_as_unhealthy() {
        let mut value = components("healthy");
        value.grpc_server = component("unknown");

        assert_eq!(derive_overall_status(&value), "unhealthy");
    }
}
