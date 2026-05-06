use axum::{routing::get, Json, Router};
use serde::Serialize;
use std::{net::SocketAddr, time::Instant};
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
struct HealthResponse {
    status: &'static str,
    uptime_seconds: u64,
    version: &'static str,
}

pub async fn serve_health(config: HealthConfig, version: &'static str) -> Result<()> {
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
    Json(HealthResponse {
        status: "healthy",
        uptime_seconds: state.started_at.elapsed().as_secs(),
        version: state.version,
    })
}
