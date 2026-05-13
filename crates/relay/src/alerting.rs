use crate::config::{AlertChannelConfig, AlertRuleConfig, AlertingConfig, AlertingSeverity};
use crate::mqtt::MqttRuntimeState;
use crate::relay_metrics::RelayMetrics;
use crate::resource_monitor::ResourceMonitor;
use crate::state::RelayState;
use crate::stream::StreamRouter;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::task::JoinHandle;

#[derive(Clone)]
pub struct AlertingRuntime {
    config: AlertingConfig,
}

impl AlertingRuntime {
    pub fn new(config: AlertingConfig) -> Self {
        Self { config }
    }

    pub fn spawn(
        &self,
        relay_id: String,
        state: Arc<RelayState>,
        stream_router: StreamRouter,
        resource_monitor: ResourceMonitor,
        mqtt_runtime: MqttRuntimeState,
        metrics: RelayMetrics,
    ) -> Option<JoinHandle<()>> {
        if !self.config.enabled {
            return None;
        }

        let config = self.config.clone();
        Some(tokio::spawn(async move {
            let interval = std::time::Duration::from_secs(config.evaluation_interval_seconds.max(1));
            let mut ticker = tokio::time::interval(interval);
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            let mut last_sent: HashMap<String, std::time::Instant> = HashMap::new();
            let mut last_state: HashMap<String, AlertingSeverity> = HashMap::new();
            let mut condition_active_since: HashMap<String, std::time::Instant> = HashMap::new();

            loop {
                ticker.tick().await;

                let context = AlertContext {
                    active_device_connections: state.sessions_by_device_id.len() as f64,
                    active_streams: stream_router.total_active_streams() as f64,
                    cpu_usage_percent: resource_monitor.cpu_usage_percent(),
                    memory_usage_percent: resource_monitor.memory_usage_percent(),
                    mqtt_connected: mqtt_runtime.is_connected(),
                    auth_failures_total: metrics.auth_failure_total.get() as f64,
                };

                for rule in &config.rules {
                    let Some(severity) = evaluate_rule(rule, &context) else {
                        // Condition cleared – reset tracking for this rule
                        condition_active_since.remove(&rule.name);
                        last_state.remove(&rule.name);
                        continue;
                    };

                    let now = std::time::Instant::now();

                    // Track when the condition first became true
                    let active_since = *condition_active_since
                        .entry(rule.name.clone())
                        .or_insert(now);

                    // If a duration is configured, the condition must hold for that long
                    if let Some(duration_secs) = rule.duration_seconds {
                        let required = std::time::Duration::from_secs(duration_secs);
                        if now.saturating_duration_since(active_since) < required {
                            continue;
                        }
                    }

                    let suppress_until = last_sent.get(&rule.name).copied();
                    let min_interval =
                        std::time::Duration::from_secs(config.suppression.min_interval_seconds);
                    if suppress_until
                        .map(|value| now.saturating_duration_since(value) < min_interval)
                        .unwrap_or(false)
                    {
                        continue;
                    }

                    let previous = last_state.get(&rule.name).copied();
                    if previous == Some(severity) {
                        continue;
                    }

                    emit_alert(&relay_id, &config.channels, rule, severity, &context);
                    last_sent.insert(rule.name.clone(), now);
                    last_state.insert(rule.name.clone(), severity);
                }
            }
        }))
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct AlertContext {
    active_device_connections: f64,
    active_streams: f64,
    cpu_usage_percent: f64,
    memory_usage_percent: f64,
    mqtt_connected: bool,
    auth_failures_total: f64,
}

fn evaluate_rule(rule: &AlertRuleConfig, context: &AlertContext) -> Option<AlertingSeverity> {
    if evaluate_condition(&rule.condition, context) {
        Some(rule.severity)
    } else {
        None
    }
}

fn evaluate_condition(condition: &str, context: &AlertContext) -> bool {
    let normalized = condition.replace(' ', "");
    if let Some((metric, threshold)) = normalized.split_once(">=") {
        return read_metric(metric, context)
            .map(|value| value >= threshold.parse::<f64>().unwrap_or(f64::MAX))
            .unwrap_or(false);
    }
    if let Some((metric, threshold)) = normalized.split_once('>') {
        return read_metric(metric, context)
            .map(|value| value > threshold.parse::<f64>().unwrap_or(f64::MAX))
            .unwrap_or(false);
    }
    if let Some((metric, expected)) = normalized.split_once("==") {
        if metric == "mqtt_connected" {
            return context.mqtt_connected == (expected == "true");
        }
    }
    false
}

fn read_metric(metric: &str, context: &AlertContext) -> Option<f64> {
    match metric {
        "active_device_connections" => Some(context.active_device_connections),
        "active_streams" => Some(context.active_streams),
        "cpu_usage_percent" => Some(context.cpu_usage_percent),
        "memory_usage_percent" => Some(context.memory_usage_percent),
        "auth_failures_total" => Some(context.auth_failures_total),
        _ => None,
    }
}

fn emit_alert(
    relay_id: &str,
    channels: &[AlertChannelConfig],
    rule: &AlertRuleConfig,
    severity: AlertingSeverity,
    context: &AlertContext,
) {
    tracing::warn!(
        event = "alert_transition",
        relay_id = %relay_id,
        alert_name = %rule.name,
        severity = %severity.as_str(),
        condition = %rule.condition,
        message = %rule.message,
        active_device_connections = context.active_device_connections,
        active_streams = context.active_streams,
        cpu_usage_percent = context.cpu_usage_percent,
        memory_usage_percent = context.memory_usage_percent,
        mqtt_connected = context.mqtt_connected,
        channels = %channels_summary(channels),
        "alert triggered"
    );
}

fn channels_summary(channels: &[AlertChannelConfig]) -> String {
    channels
        .iter()
        .map(|channel| channel.channel_type.clone())
        .collect::<Vec<_>>()
        .join(",")
}
