use axum::{routing::get, Json, Router};
use serde::Serialize;
use std::{net::SocketAddr, time::Instant};
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;
use tracing::info;

use tokio::net::TcpListener;

use crate::mqtt::MqttRuntimeState;
use crate::resource_monitor::ResourceMonitor;
use crate::security_metrics::{SecurityMetrics, SecurityMetricsSnapshot};
use crate::{config::HealthConfig, AppError, Result};

#[derive(Clone)]
pub struct HealthState {
    started_at: Instant,
    version: &'static str,
    security_metrics: SecurityMetrics,
    resource_monitor: ResourceMonitor,
    mqtt_runtime: MqttRuntimeState,
}

impl HealthState {
    pub fn new(
        version: &'static str,
        security_metrics: SecurityMetrics,
        resource_monitor: ResourceMonitor,
        mqtt_runtime: MqttRuntimeState,
    ) -> Self {
        Self {
            started_at: Instant::now(),
            version,
            security_metrics,
            resource_monitor,
            mqtt_runtime,
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

    if has_degraded {
        "degraded"
    } else {
        "healthy"
    }
}

pub async fn serve_health(
    config: HealthConfig,
    version: &'static str,
    security_metrics: SecurityMetrics,
    resource_monitor: ResourceMonitor,
    mqtt_runtime: MqttRuntimeState,
) -> Result<()> {
    if !config.enabled {
        info!("health server disabled");
        return Ok(());
    }

    let address =
        config
            .address
            .parse::<SocketAddr>()
            .map_err(|source| AppError::InvalidSocketAddress {
                address: config.address.clone(),
                source,
            })?;

    let state = HealthState::new(version, security_metrics, resource_monitor, mqtt_runtime);
    let app = Router::new()
        .route(&config.path, get(health))
        .route("/metrics/security", get(security_metrics_handler))
        .with_state(state);

    info!(
        health_address = %address,
        health_path = %config.path,
        "health server listening"
    );

    let listener = TcpListener::bind(address)
        .await
        .map_err(|source| AppError::HealthBind { address, source })?;

    axum::serve(listener, app).await?;

    Ok(())
}

async fn health(
    axum::extract::State(state): axum::extract::State<HealthState>,
) -> Json<HealthResponse> {
    let timestamp = OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_string());

    let mqtt_client = if !state.mqtt_runtime.enabled() {
        ComponentHealth {
            status: "healthy",
            message: "MQTT disabled by config",
        }
    } else if state.mqtt_runtime.is_connected() {
        ComponentHealth {
            status: "healthy",
            message: "MQTT connected",
        }
    } else {
        ComponentHealth {
            status: "degraded",
            message: "MQTT disconnected; gRPC discovery fallback required",
        }
    };

    let components = HealthComponents {
        grpc_server: ComponentHealth {
            status: "healthy",
            message: "gRPC server running",
        },
        quic_listener: ComponentHealth {
            status: "degraded",
            message: "QUIC listener not implemented",
        },
        mqtt_client,
        auth_service: ComponentHealth {
            status: "healthy",
            message: "auth service running",
        },
        metrics_collector: ComponentHealth {
            status: "healthy",
            message: "metrics collector running",
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
            cpu_usage_percent: state.resource_monitor.cpu_usage_percent(),
            memory_usage_percent: state.resource_monitor.memory_usage_percent(),
        },
    };

    Json(response)
}

async fn security_metrics_handler(
    axum::extract::State(state): axum::extract::State<HealthState>,
) -> Json<SecurityMetricsSnapshot> {
    Json(state.security_metrics.snapshot())
}

#[cfg(test)]
mod tests {
    use super::{derive_overall_status, health, ComponentHealth, HealthComponents, HealthState};
    use crate::mqtt::MqttRuntimeState;
    use crate::resource_monitor::ResourceMonitor;
    use crate::security_metrics::SecurityMetrics;
    use axum::{extract::State, Json};

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

    #[tokio::test]
    async fn health_reports_mqtt_disabled_as_healthy_component() {
        let state = HealthState::new(
            "test",
            SecurityMetrics::default(),
            ResourceMonitor::new(&crate::config::RateLimitConfig::default()),
            MqttRuntimeState::new(false),
        );

        let Json(response) = health(State(state)).await;
        let value = serde_json::to_value(response).unwrap();

        assert_eq!(value["components"]["mqtt_client"]["status"], "healthy");
        assert_eq!(value["status"], "degraded");
    }

    #[tokio::test]
    async fn health_reports_mqtt_disconnected_as_degraded_component() {
        let state = HealthState::new(
            "test",
            SecurityMetrics::default(),
            ResourceMonitor::new(&crate::config::RateLimitConfig::default()),
            MqttRuntimeState::new(true),
        );

        let Json(response) = health(State(state)).await;
        let value = serde_json::to_value(response).unwrap();

        assert_eq!(value["components"]["mqtt_client"]["status"], "degraded");
        assert_eq!(value["status"], "degraded");
    }
}
