use clap::Parser;
use relay::{
    alerting::AlertingRuntime, config::AppConfig, grpc_service::RelayGrpcService, logging, mqtt,
    observability, relay_metrics::RelayMetrics, resource_monitor::ResourceMonitor,
    state::RelayState, Result,
};
use tonic::transport::{Identity, Server, ServerTlsConfig};
use tracing::info;

#[derive(Debug, Parser)]
#[command(name = "relay", about = "gRPC-Relay server")]
struct Cli {
    #[arg(long, default_value = "config/relay.yaml")]
    config: String,
}

#[tokio::main]
async fn main() {
    if let Err(err) = run().await {
        eprintln!("{err}");
        std::process::exit(1);
    }
}

async fn run() -> Result<()> {
    let cli = Cli::parse();
    let config = AppConfig::load(&cli.config)?;
    let security_metrics = relay::security_metrics::SecurityMetrics::default();
    let relay_metrics = RelayMetrics::new()
        .map_err(|err| relay::AppError::Validation(format!("metrics init failed: {err}")))?;
    security_metrics.attach_relay_metrics(relay_metrics.clone());
    let tracer_provider =
        logging::init(&config.observability.logging, &config.observability.tracing);

    info!(
        relay_id = %config.relay.id,
        relay_address = %config.relay.address,
        "relay configuration loaded"
    );

    let resource_monitor = ResourceMonitor::new(&config.relay.rate_limiting);
    let relay_state = std::sync::Arc::new(RelayState::new());
    let stream_router = relay::stream::StreamRouter::new(&config.relay.stream);
    let mqtt_runtime = mqtt::MqttRuntimeState::new(config.relay.mqtt.enabled);
    let health_config = config.observability.health.clone();
    let health_security_metrics = security_metrics.clone();
    let health_resource_monitor = resource_monitor.clone();
    let health_mqtt_runtime = mqtt_runtime.clone();
    let health_state = relay_state.clone();
    let health_stream_router = stream_router.clone();
    let health_metrics = relay_metrics.clone();
    let health_server = tokio::spawn(observability::serve_health(
        health_config,
        env!("CARGO_PKG_VERSION"),
        health_security_metrics,
        health_resource_monitor,
        health_mqtt_runtime,
        health_state,
        health_stream_router,
        health_metrics,
        true,
    ));

    let audit_logger =
        relay::audit::AuditLogger::new(&config.observability.audit, config.relay.id.clone());

    let mqtt_publisher = if config.relay.mqtt.enabled {
        let handles = mqtt::spawn_mqtt_publisher(
            config.relay.mqtt.clone(),
            config.relay.id.clone(),
            config.relay.address.clone(),
            relay_state.clone(),
            resource_monitor.clone(),
            mqtt_runtime.clone(),
        );
        Some(handles.publisher)
    } else {
        None
    };

    let _alerting = AlertingRuntime::new(config.observability.alerting.clone()).spawn(
        config.relay.id.clone(),
        relay_state.clone(),
        stream_router.clone(),
        resource_monitor.clone(),
        mqtt_runtime.clone(),
        relay_metrics.clone(),
    );

    let grpc_service = RelayGrpcService::new(
        relay_state,
        &config,
        security_metrics,
        resource_monitor,
        mqtt_publisher,
        audit_logger,
        relay_metrics,
    );
    let stale_stream_cleanup = grpc_service.spawn_stale_stream_cleanup();
    let grpc_addr = config
        .relay
        .address
        .parse::<std::net::SocketAddr>()
        .map_err(|source| relay::AppError::InvalidSocketAddress {
            address: config.relay.address.clone(),
            source,
        })?;

    let grpc_server = tokio::spawn(async move {
        info!(%grpc_addr, "starting tonic gRPC server");
        let mut builder = Server::builder();
        if config.relay.tls.enabled {
            builder = builder.tls_config(load_tls_config(&config.relay.tls)?)?;
        }
        let res = builder
            .add_service(
                relay_proto::relay::v1::relay_service_server::RelayServiceServer::new(grpc_service),
            )
            .serve(grpc_addr)
            .await;
        res.map_err(relay::AppError::GrpcServer)
    });

    info!("relay skeleton started");

    tokio::select! {
        result = health_server => {
            let _join = result.map_err(relay::AppError::HealthTask)?;
            _join?;
        }
        result = grpc_server => {
            let _join = result.map_err(relay::AppError::GrpcTask)?;
            _join?;
        }
        result = stale_stream_cleanup => {
            result.map_err(relay::AppError::GrpcTask)?;
        }
        signal = tokio::signal::ctrl_c() => {
            signal.map_err(relay::AppError::ShutdownSignal)?;
            info!("shutdown signal received");
        }
    }

    if tracer_provider.is_some() {
        opentelemetry::global::shutdown_tracer_provider();
    }

    Ok(())
}

fn load_tls_config(config: &relay::config::TlsConfig) -> Result<ServerTlsConfig> {
    let cert_path = config
        .cert_path
        .as_deref()
        .ok_or_else(|| relay::AppError::InvalidTlsConfig("missing cert_path".to_string()))?;
    let key_path = config
        .key_path
        .as_deref()
        .ok_or_else(|| relay::AppError::InvalidTlsConfig("missing key_path".to_string()))?;

    let cert = std::fs::read(cert_path).map_err(|source| relay::AppError::Io {
        path: cert_path.to_string(),
        source,
    })?;
    let key = std::fs::read(key_path).map_err(|source| relay::AppError::Io {
        path: key_path.to_string(),
        source,
    })?;

    let mut tls = ServerTlsConfig::new().identity(Identity::from_pem(cert, key));
    if let Some(client_ca_path) = config.client_ca_path.as_deref() {
        let client_ca = std::fs::read(client_ca_path).map_err(|source| relay::AppError::Io {
            path: client_ca_path.to_string(),
            source,
        })?;
        tls = tls.client_ca_root(tonic::transport::Certificate::from_pem(client_ca));
    }

    Ok(tls)
}
