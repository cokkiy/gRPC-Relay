use crate::config::{LoggingConfig, TracingConfig};
use opentelemetry::trace::TracerProvider as _;
use opentelemetry::KeyValue;
use opentelemetry_otlp::WithExportConfig;
use opentelemetry_sdk::trace::{Config, Sampler, TracerProvider};
use opentelemetry_sdk::Resource;
use tracing_subscriber::{
    fmt,
    layer::{Layer, SubscriberExt},
    util::SubscriberInitExt,
    EnvFilter,
};

pub fn init(config: &LoggingConfig, tracing_config: &TracingConfig) -> Option<TracerProvider> {
    let filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(&config.level));

    let fmt_layer = if config.format.eq_ignore_ascii_case("json") {
        fmt::layer().json().boxed()
    } else {
        fmt::layer().boxed()
    };

    let registry = tracing_subscriber::registry().with(filter).with(fmt_layer);

    if tracing_config.enabled {
        match build_tracer_provider(tracing_config) {
            Ok(provider) => {
                let tracer = provider.tracer(tracing_config.service_name.clone());
                let otel_layer = tracing_opentelemetry::layer().with_tracer(tracer);
                if let Err(err) = registry.with(otel_layer).try_init() {
                    eprintln!("failed to initialize global tracing subscriber: {err}");
                    return None;
                }
                return Some(provider);
            }
            Err(err) => {
                eprintln!("failed to initialize OpenTelemetry provider: {err}");
            }
        }
    }

    if let Err(err) = registry.try_init() {
        eprintln!("failed to initialize global tracing subscriber: {err}");
    }

    None
}

fn build_tracer_provider(
    tracing_config: &TracingConfig,
) -> Result<TracerProvider, opentelemetry::trace::TraceError> {
    if !tracing_config.exporter.eq_ignore_ascii_case("otlp") {
        return Err(opentelemetry::trace::TraceError::Other(Box::new(
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!(
                    "unsupported tracing exporter '{}'; only 'otlp' is supported",
                    tracing_config.exporter
                ),
            ),
        )));
    }

    let resource = Resource::new(vec![
        KeyValue::new("service.name", tracing_config.service_name.clone()),
        KeyValue::new("service.version", env!("CARGO_PKG_VERSION")),
    ]);
    let trace_config = Config::default()
        .with_sampler(Sampler::ParentBased(Box::new(Sampler::TraceIdRatioBased(
            tracing_config.sampling_rate.clamp(0.0, 1.0),
        ))))
        .with_resource(resource);

    let exporter = opentelemetry_otlp::new_exporter().tonic().with_endpoint(
        tracing_config
            .otlp_endpoint
            .clone()
            .unwrap_or_else(|| "http://localhost:4317".to_string()),
    );

    let provider = opentelemetry_otlp::new_pipeline()
        .tracing()
        .with_exporter(exporter)
        .with_trace_config(trace_config)
        .install_batch(opentelemetry_sdk::runtime::Tokio)?;
    opentelemetry::global::set_tracer_provider(provider.clone());
    Ok(provider)
}
