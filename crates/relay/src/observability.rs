use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use serde::Serialize;
use std::{net::SocketAddr, sync::Arc, time::Instant};
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;
use tracing::info;

use tokio::net::TcpListener;

use crate::relay_metrics::RelayMetrics;
use crate::mqtt::MqttRuntimeState;
use crate::resource_monitor::ResourceMonitor;
use crate::security_metrics::{SecurityMetrics, SecurityMetricsSnapshot};
use crate::state::RelayState;
use crate::stream::StreamRouter;
use crate::{config::HealthConfig, AppError, Result};

#[derive(Clone)]
pub struct HealthState {
    started_at: Instant,
    version: &'static str,
    security_metrics: SecurityMetrics,
    resource_monitor: ResourceMonitor,
    mqtt_runtime: MqttRuntimeState,
    relay_state: Arc<RelayState>,
    stream_router: StreamRouter,
    metrics: RelayMetrics,
    startup_complete: bool,
}

impl HealthState {
    pub fn new(
        version: &'static str,
        security_metrics: SecurityMetrics,
        resource_monitor: ResourceMonitor,
        mqtt_runtime: MqttRuntimeState,
        relay_state: Arc<RelayState>,
        stream_router: StreamRouter,
        metrics: RelayMetrics,
        startup_complete: bool,
    ) -> Self {
        Self {
            started_at: Instant::now(),
            version,
            security_metrics,
            resource_monitor,
            mqtt_runtime,
            relay_state,
            stream_router,
            metrics,
            startup_complete,
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
    relay_state: Arc<RelayState>,
    stream_router: StreamRouter,
    metrics: RelayMetrics,
    startup_complete: bool,
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

    let state = HealthState::new(
        version,
        security_metrics,
        resource_monitor,
        mqtt_runtime,
        relay_state,
        stream_router,
        metrics,
        startup_complete,
    );
    let app = Router::new()
        .route(&config.path, get(health))
        .route("/health/live", get(live))
        .route("/health/ready", get(ready))
        .route("/health/startup", get(startup))
        .route("/metrics", get(metrics_handler))
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
    Json(build_health_response(&state))
}

async fn live() -> impl IntoResponse {
    StatusCode::OK
}

async fn ready(
    axum::extract::State(state): axum::extract::State<HealthState>,
) -> Response {
    let response = build_health_response(&state);
    if response.status == "unhealthy" {
        (StatusCode::SERVICE_UNAVAILABLE, Json(response)).into_response()
    } else {
        (StatusCode::OK, Json(response)).into_response()
    }
}

async fn startup(
    axum::extract::State(state): axum::extract::State<HealthState>,
) -> Response {
    if state.startup_complete {
        (StatusCode::OK, Json(build_health_response(&state))).into_response()
    } else {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(build_health_response(&state)),
        )
            .into_response()
    }
}

async fn metrics_handler(
    axum::extract::State(state): axum::extract::State<HealthState>,
) -> Response {
    refresh_runtime_metrics(&state);
    match state.metrics.encode() {
        Ok(encoded) => (
            StatusCode::OK,
            [("content-type", "text/plain; version=0.0.4")],
            encoded,
        )
            .into_response(),
        Err(err) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": err.to_string() })),
        )
            .into_response(),
    }
}

fn build_health_response(state: &HealthState) -> HealthResponse {
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
            message: "QUIC listener runtime is not active in this crate",
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
            active_device_connections: state.relay_state.sessions_by_device_id.len() as u64,
            active_controller_connections: state.relay_state.controller_connection_count(),
            active_streams: state.stream_router.total_active_streams() as u64,
            cpu_usage_percent: state.resource_monitor.cpu_usage_percent(),
            memory_usage_percent: state.resource_monitor.memory_usage_percent(),
        },
    };

    refresh_runtime_metrics_with_response(state, &response);
    response
}

async fn security_metrics_handler(
    axum::extract::State(state): axum::extract::State<HealthState>,
) -> Json<SecurityMetricsSnapshot> {
    Json(state.security_metrics.snapshot())
}

fn refresh_runtime_metrics(state: &HealthState) {
    let _response = build_health_response(state);
}

fn refresh_runtime_metrics_with_response(state: &HealthState, response: &HealthResponse) {
    state
        .metrics
        .active_device_connections
        .set(response.metrics.active_device_connections as i64);
    state
        .metrics
        .active_controller_connections
        .set(response.metrics.active_controller_connections as i64);
    state
        .metrics
        .active_streams
        .set(response.metrics.active_streams as i64);
    state
        .metrics
        .cpu_usage_percent
        .set(response.metrics.cpu_usage_percent);
    state
        .metrics
        .memory_usage_percent
        .set(response.metrics.memory_usage_percent);
    state.metrics.memory_used_bytes.set(
        (state.resource_monitor.used_memory_mb() * 1024 * 1024) as f64,
    );
    state
        .metrics
        .mqtt_connected
        .set(if state.mqtt_runtime.is_connected() { 1 } else { 0 });
    state
        .metrics
        .mqtt_reconnect_total
        .set(state.mqtt_runtime.reconnect_count() as i64);
    state
        .metrics
        .mqtt_dropped_total
        .set(state.mqtt_runtime.dropped_total() as i64);
    state
        .metrics
        .mqtt_queue_pending
        .set(state.mqtt_runtime.queue_pending() as i64);
    state.metrics.health_status.set(match response.status {
        "healthy" => 2,
        "degraded" => 1,
        _ => 0,
    });
    set_component_metric(&state.metrics, "grpc_server", response.components.grpc_server.status);
    set_component_metric(&state.metrics, "quic_listener", response.components.quic_listener.status);
    set_component_metric(&state.metrics, "mqtt_client", response.components.mqtt_client.status);
    set_component_metric(&state.metrics, "auth_service", response.components.auth_service.status);
    set_component_metric(
        &state.metrics,
        "metrics_collector",
        response.components.metrics_collector.status,
    );
}

fn set_component_metric(metrics: &RelayMetrics, component: &str, status: &str) {
    metrics
        .component_health
        .with_label_values(&[component])
        .set(match status {
            "healthy" => 2.0,
            "degraded" => 1.0,
            _ => 0.0,
        });
}

#[cfg(test)]
mod tests {
    use super::{derive_overall_status, health, ComponentHealth, HealthComponents, HealthState};
    use crate::mqtt::MqttRuntimeState;
    use crate::relay_metrics::RelayMetrics;
    use crate::resource_monitor::ResourceMonitor;
    use crate::security_metrics::SecurityMetrics;
    use crate::state::RelayState;
    use crate::stream::StreamRouter;
    use axum::{extract::State, Json};
    use std::sync::Arc;

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
        let relay_state = Arc::new(RelayState::new());
        let stream_router = StreamRouter::new(&crate::config::StreamConfig::default());
        let metrics = RelayMetrics::new().unwrap();
        let state = HealthState::new(
            "test",
            SecurityMetrics::default(),
            ResourceMonitor::new(&crate::config::RateLimitConfig::default()),
            MqttRuntimeState::new(false),
            relay_state,
            stream_router,
            metrics,
            true,
        );

        let Json(response) = health(State(state)).await;
        let value = serde_json::to_value(response).unwrap();

        assert_eq!(value["components"]["mqtt_client"]["status"], "healthy");
        assert_eq!(value["status"], "healthy");
    }

    #[tokio::test]
    async fn health_reports_mqtt_disconnected_as_degraded_component() {
        let relay_state = Arc::new(RelayState::new());
        let stream_router = StreamRouter::new(&crate::config::StreamConfig::default());
        let metrics = RelayMetrics::new().unwrap();
        let state = HealthState::new(
            "test",
            SecurityMetrics::default(),
            ResourceMonitor::new(&crate::config::RateLimitConfig::default()),
            MqttRuntimeState::new(true),
            relay_state,
            stream_router,
            metrics,
            true,
        );

        let Json(response) = health(State(state)).await;
        let value = serde_json::to_value(response).unwrap();

        assert_eq!(value["components"]["mqtt_client"]["status"], "degraded");
        assert_eq!(value["status"], "degraded");
    }
}
