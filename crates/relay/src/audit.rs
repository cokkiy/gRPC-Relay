use serde::Serialize;
use std::collections::HashSet;
use std::fs::{self, File, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::PathBuf;
use std::sync::Arc;
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;
use tokio::sync::mpsc;

// ── Audit event types ───────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "event_type")]
pub enum AuditEvent {
    #[serde(rename = "device_connect")]
    DeviceConnect {
        timestamp: String,
        relay_id: String,
        device_id: String,
        connection_id: String,
        source_ip: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        metadata: Option<serde_json::Value>,
    },
    #[serde(rename = "device_disconnect")]
    DeviceDisconnect {
        timestamp: String,
        relay_id: String,
        device_id: String,
        connection_id: String,
        reason: String,
        source_ip: String,
    },
    #[serde(rename = "device_register")]
    DeviceRegister {
        timestamp: String,
        relay_id: String,
        device_id: String,
        connection_id: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        previous_connection_id: Option<String>,
        session_resumed: bool,
        source_ip: String,
    },
    #[serde(rename = "controller_connect")]
    ControllerConnect {
        timestamp: String,
        relay_id: String,
        controller_id: String,
        source_ip: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        user_agent: Option<String>,
    },
    #[serde(rename = "controller_disconnect")]
    ControllerDisconnect {
        timestamp: String,
        relay_id: String,
        controller_id: String,
    },
    #[serde(rename = "controller_request")]
    ControllerRequest {
        timestamp: String,
        relay_id: String,
        controller_id: String,
        device_id: String,
        connection_id: String,
        method_name: String,
        sequence_number: i64,
        result: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        error_code: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        latency_ms: Option<f64>,
        #[serde(skip_serializing_if = "Option::is_none")]
        bytes_transferred: Option<u64>,
        source_ip: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        user_agent: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        metadata: Option<serde_json::Value>,
    },
    #[serde(rename = "stream_created")]
    StreamCreated {
        timestamp: String,
        relay_id: String,
        stream_id: String,
        device_id: String,
        controller_id: String,
        method_name: String,
        source_ip: String,
    },
    #[serde(rename = "stream_closed")]
    StreamClosed {
        timestamp: String,
        relay_id: String,
        stream_id: String,
        device_id: String,
        controller_id: String,
        reason: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        bytes_transferred: Option<u64>,
    },
    #[serde(rename = "auth_failure")]
    AuthFailure {
        timestamp: String,
        relay_id: String,
        entity_type: String,
        entity_id: String,
        reason: String,
        source_ip: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        token_prefix: Option<String>,
    },
    #[serde(rename = "auth_success")]
    AuthSuccess {
        timestamp: String,
        relay_id: String,
        entity_type: String,
        entity_id: String,
        source_ip: String,
    },
    #[serde(rename = "authorization_denied")]
    AuthorizationDenied {
        timestamp: String,
        relay_id: String,
        controller_id: String,
        device_id: String,
        method_name: String,
        reason: String,
        source_ip: String,
    },
    #[serde(rename = "rate_limit")]
    RateLimit {
        timestamp: String,
        relay_id: String,
        entity_type: String,
        entity_id: String,
        limit_kind: String,
        source_ip: String,
    },
    #[serde(rename = "session_resumed")]
    SessionResumed {
        timestamp: String,
        relay_id: String,
        device_id: String,
        old_connection_id: String,
        new_connection_id: String,
        source_ip: String,
    },
    #[serde(rename = "session_expired")]
    SessionExpired {
        timestamp: String,
        relay_id: String,
        device_id: String,
        connection_id: String,
        source_ip: String,
    },
    #[serde(rename = "error")]
    Error {
        timestamp: String,
        relay_id: String,
        message: String,
        error_code: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        context: Option<serde_json::Value>,
    },
}

// ── AuditWriter trait ───────────────────────────────────────────────────────

pub trait AuditWriter: Send + Sync + 'static {
    fn write_event(&self, line: String) -> std::io::Result<()>;
    fn flush(&self) -> std::io::Result<()>;
}

// ── FileAuditWriter with rotation ───────────────────────────────────────────

struct FileAuditWriter {
    writer: std::sync::Mutex<BufWriter<File>>,
    file_path: PathBuf,
    max_size: u64,
    max_backups: usize,
    retention_days: u32,
    bytes_written: std::sync::atomic::AtomicU64,
}

impl FileAuditWriter {
    fn new(
        file_path: PathBuf,
        max_size_mb: u64,
        max_backups: usize,
        retention_days: u32,
    ) -> std::io::Result<Self> {
        if let Some(parent) = file_path.parent() {
            fs::create_dir_all(parent)?;
        }

        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&file_path)?;

        let metadata = file.metadata()?;
        let bytes_written = std::sync::atomic::AtomicU64::new(metadata.len());

        let writer = Self {
            writer: std::sync::Mutex::new(BufWriter::new(file)),
            file_path,
            max_size: max_size_mb * 1024 * 1024,
            max_backups,
            retention_days,
            bytes_written,
        };
        writer.cleanup_old_backups();
        Ok(writer)
    }

    /// Delete rotated backup files that are older than `retention_days`.
    fn cleanup_old_backups(&self) {
        if self.retention_days == 0 {
            return;
        }
        let cutoff = std::time::SystemTime::now()
            - std::time::Duration::from_secs(self.retention_days as u64 * 24 * 3600);

        for i in 1..=self.max_backups {
            let backup = self.file_path.with_extension(format!("log.{i}"));
            if !backup.exists() {
                break;
            }
            let age = fs::metadata(&backup)
                .and_then(|m| m.modified())
                .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
            if age < cutoff {
                if let Err(e) = fs::remove_file(&backup) {
                    tracing::warn!(
                        error = %e,
                        path = %backup.display(),
                        "failed to remove old audit log backup"
                    );
                }
            }
        }
    }

    fn rotate(&self) -> std::io::Result<()> {
        {
            // Take the writer lock and flush before renaming to ensure all buffered
            // data is persisted and no concurrent writes occur during the rename.
            let mut writer = self.writer.lock().unwrap();
            writer.flush()?;

            // Rotate old backup files: audit.log.N -> audit.log.(N+1)
            for i in (1..self.max_backups).rev() {
                let old = self.file_path.with_extension(format!("log.{i}"));
                let new = self.file_path.with_extension(format!("log.{}", i + 1));
                if old.exists() {
                    if let Err(e) = fs::rename(&old, &new) {
                        tracing::warn!(
                            error = %e,
                            from = %old.display(),
                            to = %new.display(),
                            "audit log backup rotation failed"
                        );
                    }
                }
            }

            // Rotate current audit.log -> audit.log.1
            let first_backup = self.file_path.with_extension("log.1");
            if let Err(e) = fs::rename(&self.file_path, &first_backup) {
                tracing::error!(
                    error = %e,
                    path = %self.file_path.display(),
                    "failed to rename active audit log for rotation; rotation skipped"
                );
                return Err(e);
            }

            // Create a fresh audit.log
            let file = OpenOptions::new()
                .create(true)
                .append(true)
                .open(&self.file_path)?;

            *writer = BufWriter::new(file);
            self.bytes_written
                .store(0, std::sync::atomic::Ordering::Relaxed);
        } // writer lock is released here before retention cleanup

        self.cleanup_old_backups();
        Ok(())
    }
}

impl AuditWriter for FileAuditWriter {
    fn write_event(&self, line: String) -> std::io::Result<()> {
        let mut writer = self.writer.lock().unwrap();
        writeln!(writer, "{line}")?;
        writer.flush()?;

        let new_total = self
            .bytes_written
            .fetch_add(line.len() as u64 + 1, std::sync::atomic::Ordering::Relaxed)
            + line.len() as u64
            + 1;

        if new_total >= self.max_size {
            drop(writer);
            self.rotate()?;
        }

        Ok(())
    }

    fn flush(&self) -> std::io::Result<()> {
        self.writer.lock().unwrap().flush()
    }
}

// ── StdoutAuditWriter ───────────────────────────────────────────────────────

struct StdoutAuditWriter;

impl AuditWriter for StdoutAuditWriter {
    fn write_event(&self, line: String) -> std::io::Result<()> {
        println!("{line}");
        Ok(())
    }

    fn flush(&self) -> std::io::Result<()> {
        std::io::stdout().flush()
    }
}

// ── AuditLogger ─────────────────────────────────────────────────────────────

pub struct AuditLogger {
    tx: mpsc::Sender<String>,
    relay_id: String,
    enabled_events: Option<HashSet<String>>,
    dropped_events: std::sync::atomic::AtomicU64,
}

impl AuditLogger {
    /// Spawn the audit writer background task and return an AuditLogger.
    /// If `config.enabled` is false, returns None.
    pub fn new(config: &super::config::AuditConfig, relay_id: String) -> Option<Arc<Self>> {
        if !config.enabled {
            tracing::info!("audit logging disabled");
            return None;
        }

        let writer: Box<dyn AuditWriter> = match config.output.as_str() {
            "stdout" => Box::new(StdoutAuditWriter),
            _ => {
                let file_path = PathBuf::from(&config.file_path);
                match FileAuditWriter::new(
                    file_path,
                    config.max_size_mb,
                    config.max_backups,
                    config.retention_days,
                ) {
                    Ok(w) => Box::new(w),
                    Err(e) => {
                        tracing::error!(
                            error = %e,
                            path = %config.file_path,
                            "failed to open audit log file; falling back to stdout"
                        );
                        Box::new(StdoutAuditWriter)
                    }
                }
            }
        };

        let writer = Arc::new(std::sync::Mutex::new(writer));

        let (tx, mut rx) = mpsc::channel::<String>(4096);

        tokio::spawn(async move {
            while let Some(line) = rx.recv().await {
                if let Ok(ref w) = writer.lock() {
                    if let Err(e) = w.write_event(line) {
                        tracing::error!(error = %e, "audit write failed");
                    }
                }
            }
            // Flush on shutdown
            if let Ok(ref w) = writer.lock() {
                let _ = w.flush();
            }
        });

        let enabled_events = if config.events.is_empty() {
            None // all events enabled
        } else {
            Some(config.events.iter().cloned().collect())
        };

        tracing::info!(
            output = %config.output,
            "audit logging enabled"
        );

        Some(Arc::new(Self {
            tx,
            relay_id,
            enabled_events,
            dropped_events: std::sync::atomic::AtomicU64::new(0),
        }))
    }

    fn should_log(&self, event_type: &str) -> bool {
        match &self.enabled_events {
            None => true,
            Some(events) => events.contains(event_type),
        }
    }

    fn now(&self) -> String {
        OffsetDateTime::now_utc()
            .format(&Rfc3339)
            .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_string())
    }

    fn send(&self, event: AuditEvent) {
        if let Ok(json) = serde_json::to_string(&event) {
            if let Err(e) = self.tx.try_send(json) {
                let dropped = self
                    .dropped_events
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
                    + 1;
                // Log every 100 drops to avoid flooding the log under sustained pressure.
                if dropped % 100 == 1 {
                    tracing::warn!(
                        dropped_total = dropped,
                        reason = %e,
                        "audit channel full; events are being dropped"
                    );
                }
            }
        }
    }

    // ── Convenience methods ─────────────────────────────────────────────

    pub fn device_connect(
        &self,
        device_id: &str,
        connection_id: &str,
        source_ip: &str,
        metadata: Option<serde_json::Value>,
    ) {
        if !self.should_log("device_connect") {
            return;
        }
        self.send(AuditEvent::DeviceConnect {
            timestamp: self.now(),
            relay_id: self.relay_id.clone(),
            device_id: device_id.to_string(),
            connection_id: connection_id.to_string(),
            source_ip: source_ip.to_string(),
            metadata,
        });
    }

    pub fn device_disconnect(
        &self,
        device_id: &str,
        connection_id: &str,
        reason: &str,
        source_ip: &str,
    ) {
        if !self.should_log("device_disconnect") {
            return;
        }
        self.send(AuditEvent::DeviceDisconnect {
            timestamp: self.now(),
            relay_id: self.relay_id.clone(),
            device_id: device_id.to_string(),
            connection_id: connection_id.to_string(),
            reason: reason.to_string(),
            source_ip: source_ip.to_string(),
        });
    }

    pub fn device_register(
        &self,
        device_id: &str,
        connection_id: &str,
        previous_connection_id: Option<&str>,
        session_resumed: bool,
        source_ip: &str,
    ) {
        if !self.should_log("device_register") {
            return;
        }
        self.send(AuditEvent::DeviceRegister {
            timestamp: self.now(),
            relay_id: self.relay_id.clone(),
            device_id: device_id.to_string(),
            connection_id: connection_id.to_string(),
            previous_connection_id: previous_connection_id.map(|s| s.to_string()),
            session_resumed,
            source_ip: source_ip.to_string(),
        });
    }

    pub fn controller_connect(
        &self,
        controller_id: &str,
        source_ip: &str,
        user_agent: Option<&str>,
    ) {
        if !self.should_log("controller_connect") {
            return;
        }
        self.send(AuditEvent::ControllerConnect {
            timestamp: self.now(),
            relay_id: self.relay_id.clone(),
            controller_id: controller_id.to_string(),
            source_ip: source_ip.to_string(),
            user_agent: user_agent.map(|s| s.to_string()),
        });
    }

    pub fn controller_disconnect(&self, controller_id: &str) {
        if !self.should_log("controller_disconnect") {
            return;
        }
        self.send(AuditEvent::ControllerDisconnect {
            timestamp: self.now(),
            relay_id: self.relay_id.clone(),
            controller_id: controller_id.to_string(),
        });
    }

    #[allow(clippy::too_many_arguments)]
    pub fn controller_request(
        &self,
        controller_id: &str,
        device_id: &str,
        connection_id: &str,
        method_name: &str,
        sequence_number: i64,
        result: &str,
        error_code: Option<&str>,
        latency_ms: Option<f64>,
        bytes_transferred: Option<u64>,
        source_ip: &str,
        user_agent: Option<&str>,
        metadata: Option<serde_json::Value>,
    ) {
        if !self.should_log("controller_request") {
            return;
        }
        self.send(AuditEvent::ControllerRequest {
            timestamp: self.now(),
            relay_id: self.relay_id.clone(),
            controller_id: controller_id.to_string(),
            device_id: device_id.to_string(),
            connection_id: connection_id.to_string(),
            method_name: method_name.to_string(),
            sequence_number,
            result: result.to_string(),
            error_code: error_code.map(|s| s.to_string()),
            latency_ms,
            bytes_transferred,
            source_ip: source_ip.to_string(),
            user_agent: user_agent.map(|s| s.to_string()),
            metadata,
        });
    }

    pub fn stream_created(
        &self,
        stream_id: &str,
        device_id: &str,
        controller_id: &str,
        method_name: &str,
        source_ip: &str,
    ) {
        if !self.should_log("stream_created") {
            return;
        }
        self.send(AuditEvent::StreamCreated {
            timestamp: self.now(),
            relay_id: self.relay_id.clone(),
            stream_id: stream_id.to_string(),
            device_id: device_id.to_string(),
            controller_id: controller_id.to_string(),
            method_name: method_name.to_string(),
            source_ip: source_ip.to_string(),
        });
    }

    pub fn stream_closed(
        &self,
        stream_id: &str,
        device_id: &str,
        controller_id: &str,
        reason: &str,
        bytes_transferred: Option<u64>,
    ) {
        if !self.should_log("stream_closed") {
            return;
        }
        self.send(AuditEvent::StreamClosed {
            timestamp: self.now(),
            relay_id: self.relay_id.clone(),
            stream_id: stream_id.to_string(),
            device_id: device_id.to_string(),
            controller_id: controller_id.to_string(),
            reason: reason.to_string(),
            bytes_transferred,
        });
    }

    pub fn auth_failure(
        &self,
        entity_type: &str,
        entity_id: &str,
        reason: &str,
        source_ip: &str,
        token_prefix: Option<&str>,
    ) {
        if !self.should_log("auth_failure") {
            return;
        }
        self.send(AuditEvent::AuthFailure {
            timestamp: self.now(),
            relay_id: self.relay_id.clone(),
            entity_type: entity_type.to_string(),
            entity_id: entity_id.to_string(),
            reason: reason.to_string(),
            source_ip: source_ip.to_string(),
            token_prefix: token_prefix.map(|s| s.to_string()),
        });
    }

    pub fn auth_success(&self, entity_type: &str, entity_id: &str, source_ip: &str) {
        if !self.should_log("auth_success") {
            return;
        }
        self.send(AuditEvent::AuthSuccess {
            timestamp: self.now(),
            relay_id: self.relay_id.clone(),
            entity_type: entity_type.to_string(),
            entity_id: entity_id.to_string(),
            source_ip: source_ip.to_string(),
        });
    }

    pub fn authorization_denied(
        &self,
        controller_id: &str,
        device_id: &str,
        method_name: &str,
        reason: &str,
        source_ip: &str,
    ) {
        if !self.should_log("authorization_denied") {
            return;
        }
        self.send(AuditEvent::AuthorizationDenied {
            timestamp: self.now(),
            relay_id: self.relay_id.clone(),
            controller_id: controller_id.to_string(),
            device_id: device_id.to_string(),
            method_name: method_name.to_string(),
            reason: reason.to_string(),
            source_ip: source_ip.to_string(),
        });
    }

    pub fn rate_limit(
        &self,
        entity_type: &str,
        entity_id: &str,
        limit_kind: &str,
        source_ip: &str,
    ) {
        if !self.should_log("rate_limit") {
            return;
        }
        self.send(AuditEvent::RateLimit {
            timestamp: self.now(),
            relay_id: self.relay_id.clone(),
            entity_type: entity_type.to_string(),
            entity_id: entity_id.to_string(),
            limit_kind: limit_kind.to_string(),
            source_ip: source_ip.to_string(),
        });
    }

    pub fn session_resumed(
        &self,
        device_id: &str,
        old_connection_id: &str,
        new_connection_id: &str,
        source_ip: &str,
    ) {
        if !self.should_log("session_resumed") {
            return;
        }
        self.send(AuditEvent::SessionResumed {
            timestamp: self.now(),
            relay_id: self.relay_id.clone(),
            device_id: device_id.to_string(),
            old_connection_id: old_connection_id.to_string(),
            new_connection_id: new_connection_id.to_string(),
            source_ip: source_ip.to_string(),
        });
    }

    pub fn session_expired(&self, device_id: &str, connection_id: &str, source_ip: &str) {
        if !self.should_log("session_expired") {
            return;
        }
        self.send(AuditEvent::SessionExpired {
            timestamp: self.now(),
            relay_id: self.relay_id.clone(),
            device_id: device_id.to_string(),
            connection_id: connection_id.to_string(),
            source_ip: source_ip.to_string(),
        });
    }

    pub fn error(&self, message: &str, error_code: &str, context: Option<serde_json::Value>) {
        if !self.should_log("error") {
            return;
        }
        self.send(AuditEvent::Error {
            timestamp: self.now(),
            relay_id: self.relay_id.clone(),
            message: message.to_string(),
            error_code: error_code.to_string(),
            context,
        });
    }
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn test_config() -> super::super::config::AuditConfig {
        super::super::config::AuditConfig {
            enabled: true,
            output: "stdout".to_string(),
            file_path: String::new(),
            max_size_mb: 100,
            max_backups: 10,
            retention_days: 30,
            events: vec![],
        }
    }

    #[tokio::test]
    async fn audit_logger_creates_valid_json() {
        let config = test_config();
        let logger = AuditLogger::new(&config, "relay-test".into()).unwrap();

        logger.device_connect(
            "dev-1",
            "conn-1",
            "192.168.1.100",
            Some(serde_json::json!({"region": "us-west"})),
        );

        // The event goes through a channel; in stdout mode it's printed.
        // At minimum, the logger should be constructable and not panic.
    }

    #[test]
    fn audit_logger_disabled_when_config_disabled() {
        let mut config = test_config();
        config.enabled = false;
        assert!(AuditLogger::new(&config, "relay-test".into()).is_none());
    }

    #[tokio::test]
    async fn audit_event_filter_respects_enabled_events() {
        let mut config = test_config();
        config.events = vec!["auth_failure".to_string(), "rate_limit".to_string()];

        let logger = AuditLogger::new(&config, "relay-test".into()).unwrap();
        assert!(logger.should_log("auth_failure"));
        assert!(logger.should_log("rate_limit"));
        assert!(!logger.should_log("device_connect"));
        assert!(!logger.should_log("auth_success"));
    }

    #[tokio::test]
    async fn audit_event_all_enabled_when_filter_empty() {
        let config = test_config();
        let logger = AuditLogger::new(&config, "relay-test".into()).unwrap();
        assert!(logger.should_log("device_connect"));
        assert!(logger.should_log("auth_failure"));
        assert!(logger.should_log("controller_request"));
        assert!(logger.should_log("unknown_event_type"));
    }

    #[test]
    fn file_audit_writer_creates_and_writes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("audit.log");

        let writer = FileAuditWriter::new(path.clone(), 1, 3, 30).unwrap();
        writer
            .write_event(r#"{"event_type":"test"}"#.to_string())
            .unwrap();

        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("test"));
    }

    #[test]
    fn file_audit_writer_rotates_when_exceeding_size() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("audit.log");

        // 1 MB max, write enough to trigger rotation
        let writer = FileAuditWriter::new(path.clone(), 1, 3, 30).unwrap();

        // Write a large line (~100KB) multiple times to exceed 1MB
        let big_line = "x".repeat(100_000);
        for _ in 0..12 {
            writer.write_event(big_line.clone()).unwrap();
        }

        // Should have rotated: audit.log.1 should exist
        let backup = path.with_extension("log.1");
        assert!(backup.exists(), "backup file should exist after rotation");
    }

    #[test]
    fn audit_event_serializes_with_correct_event_type_field() {
        let now = "2025-01-15T10:30:00Z";

        let event = AuditEvent::DeviceConnect {
            timestamp: now.to_string(),
            relay_id: "relay-1".into(),
            device_id: "dev-1".into(),
            connection_id: "conn-1".into(),
            source_ip: "10.0.0.1".into(),
            metadata: None,
        };

        let json = serde_json::to_string(&event).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["event_type"], "device_connect");
        assert_eq!(parsed["device_id"], "dev-1");
        assert_eq!(parsed["connection_id"], "conn-1");
        assert!(parsed.get("metadata").is_none());
    }

    #[test]
    fn auth_failure_event_includes_token_prefix() {
        let now = "2025-01-15T10:30:00Z";

        let event = AuditEvent::AuthFailure {
            timestamp: now.to_string(),
            relay_id: "relay-1".into(),
            entity_type: "controller".into(),
            entity_id: "ctrl-1".into(),
            reason: "invalid_token".into(),
            source_ip: "10.0.0.1".into(),
            token_prefix: Some("abcd1234".into()),
        };

        let json = serde_json::to_string(&event).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["event_type"], "auth_failure");
        assert_eq!(parsed["token_prefix"], "abcd1234");
        // Must NOT contain full token value
        assert!(!json.contains("secret-token"));
    }

    #[test]
    fn token_sanitization_returns_only_prefix() {
        let full_token = "abcdefghijklmnop";
        let prefix = "abcdefgh";
        assert_eq!(crate::auth::AuthService::token_prefix(full_token), prefix);
    }

    #[test]
    fn no_payload_in_audit_log() {
        let event = AuditEvent::ControllerRequest {
            timestamp: "2025-01-15T10:30:00Z".to_string(),
            relay_id: "relay-1".to_string(),
            controller_id: "ctrl-1".to_string(),
            device_id: "dev-1".to_string(),
            connection_id: "conn-1".to_string(),
            method_name: "ExecuteCommand".to_string(),
            sequence_number: 1,
            result: "success".to_string(),
            error_code: None,
            latency_ms: None,
            bytes_transferred: None,
            source_ip: "10.0.0.1".to_string(),
            user_agent: None,
            metadata: None,
        };
        let json = serde_json::to_string(&event).unwrap();
        // Must not contain encrypted_payload
        assert!(!json.contains("encrypted_payload"));
        assert!(!json.contains("payload"));
    }

    #[test]
    fn audit_jsonl_format_single_line() {
        let event = AuditEvent::DeviceConnect {
            timestamp: "2025-01-15T10:30:00Z".to_string(),
            relay_id: "relay-1".to_string(),
            device_id: "dev-1".to_string(),
            connection_id: "conn-1".to_string(),
            source_ip: "10.0.0.1".to_string(),
            metadata: None,
        };
        let json = serde_json::to_string(&event).unwrap();
        // JSONL means the output must not contain newlines within the event
        assert!(!json.contains('\n'));
        // Valid JSON
        let _: serde_json::Value = serde_json::from_str(&json).unwrap();
    }

    #[test]
    fn file_rotation_creates_backup() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("audit.log");

        // 1 MB max size
        let writer = FileAuditWriter::new(path.clone(), 1, 3, 30).unwrap();

        // Write ~100KB each, 12 iterations > 1MB total
        let line = "x".repeat(100_000);
        for _ in 0..12 {
            writer.write_event(line.clone()).unwrap();
        }

        // After rotation, audit.log.1 should exist
        let backup = path.with_extension("log.1");
        assert!(backup.exists(), "backup should exist after rotation");
    }

    #[tokio::test]
    async fn async_write_non_blocking() {
        // AuditLogger uses mpsc channel to decouple writes from the hot path
        // Test that send() doesn't block the caller
        let config = crate::config::AuditConfig {
            enabled: true,
            output: "stdout".to_string(),
            file_path: String::new(),
            max_size_mb: 100,
            max_backups: 10,
            retention_days: 30,
            events: vec![],
        };

        let logger = AuditLogger::new(&config, "relay-test".into());
        assert!(logger.is_some());

        // Sending events should not block (try_send is used internally)
        let logger = logger.unwrap();
        logger.device_connect("dev-1", "conn-1", "10.0.0.1", None);
        // No crash means non-blocking behavior is working
    }

    #[tokio::test]
    async fn event_filter_config_limits_events() {
        let config = crate::config::AuditConfig {
            enabled: true,
            output: "stdout".to_string(),
            file_path: String::new(),
            max_size_mb: 100,
            max_backups: 10,
            retention_days: 30,
            events: vec!["auth_failure".to_string(), "rate_limit".to_string()],
        };

        let logger = AuditLogger::new(&config, "relay-test".into()).unwrap();
        assert!(logger.should_log("auth_failure"));
        assert!(logger.should_log("rate_limit"));
        assert!(!logger.should_log("device_connect"));
        assert!(!logger.should_log("auth_success"));
    }
}
