use crate::config::RateLimitConfig;
use std::sync::{Arc, Mutex};
use sysinfo::{CpuRefreshKind, MemoryRefreshKind, RefreshKind, System};

/// Monitors system resource usage and enforces CPU/memory thresholds.
///
/// Used to reject new connections when the system is overloaded.
#[derive(Debug, Clone)]
pub struct ResourceMonitor {
    cpu_threshold: f64,
    memory_threshold_mb: u64,
    sys: Arc<Mutex<System>>,
}

impl ResourceMonitor {
    pub fn new(config: &RateLimitConfig) -> Self {
        let sys = System::new_with_specifics(
            RefreshKind::new()
                .with_cpu(CpuRefreshKind::everything())
                .with_memory(MemoryRefreshKind::everything()),
        );
        Self {
            cpu_threshold: config.cpu_threshold_percent,
            memory_threshold_mb: config.memory_threshold_mb,
            sys: Arc::new(Mutex::new(sys)),
        }
    }

    /// Returns true if system resources are within healthy thresholds.
    pub fn is_healthy(&self) -> bool {
        let mut sys = self.sys.lock().unwrap();
        sys.refresh_cpu_all();
        sys.refresh_memory();

        // Average CPU usage across all cores
        let cpu_usage = Self::cpu_usage_percent_from_raw(sys.global_cpu_usage());
        if cpu_usage > self.cpu_threshold {
            tracing::warn!(
                cpu_usage_percent = %cpu_usage,
                cpu_threshold = %self.cpu_threshold,
                "resource monitor: cpu threshold exceeded"
            );
            return false;
        }

        let used_memory_mb = sys.used_memory() / (1024 * 1024);
        if used_memory_mb > self.memory_threshold_mb {
            tracing::warn!(
                used_memory_mb = %used_memory_mb,
                memory_threshold_mb = %self.memory_threshold_mb,
                "resource monitor: memory threshold exceeded"
            );
            return false;
        }

        true
    }

    /// Returns current CPU usage percentage across all cores.
    pub fn cpu_usage_percent(&self) -> f64 {
        let mut sys = self.sys.lock().unwrap();
        sys.refresh_cpu_all();
        Self::cpu_usage_percent_from_raw(sys.global_cpu_usage())
    }

    /// Returns current memory usage percentage.
    pub fn memory_usage_percent(&self) -> f64 {
        let mut sys = self.sys.lock().unwrap();
        sys.refresh_memory();
        let total = sys.total_memory();
        if total == 0 {
            return 0.0;
        }
        (sys.used_memory() as f64 / total as f64) * 100.0
    }

    /// Returns used memory in MB.
    pub fn used_memory_mb(&self) -> u64 {
        let mut sys = self.sys.lock().unwrap();
        sys.refresh_memory();
        sys.used_memory() / (1024 * 1024)
    }

    pub fn cpu_threshold(&self) -> f64 {
        self.cpu_threshold
    }

    pub fn memory_threshold_mb(&self) -> u64 {
        self.memory_threshold_mb
    }

    fn cpu_usage_percent_from_raw(raw_cpu_usage: f32) -> f64 {
        raw_cpu_usage as f64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_config() -> RateLimitConfig {
        RateLimitConfig {
            device_requests_per_second: 1000,
            controller_requests_per_minute: 1000,
            global_requests_per_second: 100_000,
            device_connection_per_minute: 10,
            global_connections_per_second: 100,
            device_bandwidth_bytes_per_sec: 10 * 1024 * 1024,
            controller_bandwidth_bytes_per_sec: 100 * 1024 * 1024,
            global_bandwidth_bytes_per_sec: 100 * 1024 * 1024,
            cpu_threshold_percent: 80.0,
            memory_threshold_mb: 12 * 1024,
        }
    }

    #[test]
    fn resource_monitor_has_healthy_defaults() {
        let mut config = test_config();
        config.cpu_threshold_percent = 100.0;
        config.memory_threshold_mb = u64::MAX;

        let monitor = ResourceMonitor::new(&config);
        assert!(monitor.is_healthy());
    }

    #[test]
    fn thresholds_match_config() {
        let monitor = ResourceMonitor::new(&test_config());
        assert_eq!(monitor.cpu_threshold(), 80.0);
        assert_eq!(monitor.memory_threshold_mb(), 12 * 1024);
    }

    #[test]
    fn metrics_report_nonzero() {
        let monitor = ResourceMonitor::new(&test_config());
        let cpu = monitor.cpu_usage_percent();
        let mem = monitor.memory_usage_percent();
        let used_mb = monitor.used_memory_mb();
        assert!(cpu >= 0.0);
        assert!(mem > 0.0);
        assert!(used_mb > 0);
    }

    #[test]
    fn cpu_usage_normalization_preserves_percentage_units() {
        assert_eq!(ResourceMonitor::cpu_usage_percent_from_raw(0.0), 0.0);
        assert_eq!(ResourceMonitor::cpu_usage_percent_from_raw(42.5), 42.5);
        assert_eq!(ResourceMonitor::cpu_usage_percent_from_raw(100.0), 100.0);
    }
}
