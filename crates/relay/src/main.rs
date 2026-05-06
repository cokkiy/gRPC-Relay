use clap::Parser;
use relay::{config::AppConfig, logging, observability, Result};
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

    info!("relay skeleton started");
    tokio::select! {
        result = health_server => {
            result.map_err(relay::AppError::HealthTask)??;
        }
        signal = tokio::signal::ctrl_c() => {
            signal.map_err(relay::AppError::ShutdownSignal)?;
            info!("shutdown signal received");
        }
    }

    Ok(())
}
