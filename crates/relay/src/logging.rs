use crate::config::LoggingConfig;
use tracing_subscriber::{fmt, EnvFilter};

pub fn init(config: &LoggingConfig) {
    let filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(&config.level));

    let builder = fmt::Subscriber::builder().with_env_filter(filter);
    let result = if config.format.eq_ignore_ascii_case("json") {
        tracing::subscriber::set_global_default(builder.json().finish())
    } else {
        tracing::subscriber::set_global_default(builder.finish())
    };

    let _ = result;
}
