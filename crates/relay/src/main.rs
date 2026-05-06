use clap::Parser;
use relay::{config::AppConfig, logging, Result};
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
    logging::init();

    let config = AppConfig::load(&cli.config)?;
    info!(
        relay_id = %config.relay.id,
        relay_address = %config.relay.address,
        "relay configuration loaded"
    );

    info!("relay skeleton started");
    Ok(())
}
