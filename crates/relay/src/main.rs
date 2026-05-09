use clap::Parser;
use relay::{
    config::AppConfig, grpc_service::RelayGrpcService, logging, observability, state::RelayState,
    Result,
};
use tonic::transport::Server;
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
    logging::init(&config.observability.logging);

    info!(
        relay_id = %config.relay.id,
        relay_address = %config.relay.address,
        "relay configuration loaded"
    );

    let health_config = config.observability.health.clone();
    let health_server = tokio::spawn(observability::serve_health(
        health_config,
        env!("CARGO_PKG_VERSION"),
    ));

    let relay_state = std::sync::Arc::new(RelayState::new());
    let grpc_service = RelayGrpcService::new(relay_state, &config);
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
        let res = Server::builder()
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

    Ok(())
}
