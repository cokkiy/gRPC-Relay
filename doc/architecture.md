# Adapted Architecture for gRPC-Relay (Rust Implementation)

## System Architecture Overview

```
┌─────────────────────────────────────────────────────────────────┐
│                        MQTT Broker                               │
│                  (Service Discovery Hub)                         │
│                                                                   │
│  Topics:                                                         │
│  - relay/{relay_id}/device/online                               │
│  - relay/{relay_id}/device/offline                              │
│  - device/{device_id}/telemetry                                 │
│  - relay/{relay_id}/telemetry                                   │
└────────▲─────────────────────────────────────────▲──────────────┘
         │                                          │
         │ Publish events (QoS 1)                   │ Publish status
         │                                          │
┌────────┴──────────┐                    ┌─────────┴────────────┐
│                   │                    │                      │
│   Relay Server    │◄───────────────────┤  stationService      │
│   (Rust + QUIC)   │   gRPC/QUIC        │  (Device Agent)      │
│                   │                    │                      │
│  ┌─────────────┐  │                    └──────────────────────┘
│  │ Connection  │  │                              ▲
│  │ Manager     │  │                              │
│  └─────────────┘  │                              │
│  ┌─────────────┐  │                              │
│  │ Stream      │  │                    ┌─────────┴────────────┐
│  │ Router      │  │                    │                      │
│  └─────────────┘  │                    │   IoT Device         │
│  ┌─────────────┐  │                    │   (Embedded)         │
│  │ Auth &      │  │                    │                      │
│  │ RBAC Engine │  │                    └──────────────────────┘
│  └─────────────┘  │
│  ┌─────────────┐  │
│  │ Session     │  │
│  │ Manager     │  │
│  │ (300s TTL)  │  │
│  └─────────────┘  │
│  ┌─────────────┐  │
│  │ Idempotency │  │
│  │ Cache       │  │
│  │ (LRU 10K)   │  │
│  └─────────────┘  │
│  ┌─────────────┐  │
│  │ Telemetry   │  │
│  │ Collector   │  │
│  └─────────────┘  │
└────────▲──────────┘
         │
         │ gRPC/HTTP2 + TLS 1.3
         │ (E2E encrypted payload)
         │
┌────────┴──────────┐
│                   │
│   Controller      │
│   (Control Plane) │
│                   │
└───────────────────┘
```

## Architecture Layers

### Layer 1: Transport Layer

```
┌─────────────────────────────────────────────────────────────┐
│                    Transport Layer                           │
├─────────────────────────────────────────────────────────────┤
│                                                               │
│  Device Side (QUIC)              Controller Side (HTTP/2)    │
│  ┌──────────────────┐            ┌──────────────────┐       │
│  │ Quinn (QUIC)     │            │ Tonic (gRPC)     │       │
│  │ - TLS 1.3        │            │ - TLS 1.3        │       │
│  │ - 0-RTT support  │            │ - HTTP/2         │       │
│  │ - Connection     │            │ - Multiplexing   │       │
│  │   migration      │            │                  │       │
│  │ - UDP transport  │            │ - TCP transport  │       │
│  └──────────────────┘            └──────────────────┘       │
│                                                               │
└─────────────────────────────────────────────────────────────┘
```

**Key Design Decisions:**
- **Device → Relay**: QUIC for low latency, connection migration, and multiplexing
- **Controller → Relay**: HTTP/2 for maturity and ecosystem support
- **Future**: Upgrade Controller to QUIC in v2.0

### Layer 2: Connection Management Layer

```
┌─────────────────────────────────────────────────────────────┐
│              Connection Management Layer                     │
├─────────────────────────────────────────────────────────────┤
│                                                               │
│  ┌────────────────────────────────────────────────────┐     │
│  │         Connection Manager (DashMap)               │     │
│  │                                                     │     │
│  │  device_sessions: DeviceId → DeviceSession         │     │
│  │  connection_to_device: ConnectionId → DeviceId     │     │
│  │  recovery_sessions: DeviceId → RecoverySession     │     │
│  │                                                     │     │
│  │  Operations:                                        │     │
│  │  - register_device()                                │     │
│  │  - update_heartbeat()                               │     │
│  │  - mark_offline()                                   │     │
│  │  - recover_session()                                │     │
│  └────────────────────────────────────────────────────┘     │
│                                                               │
│  ┌────────────────────────────────────────────────────┐     │
│  │         Session Manager                             │     │
│  │                                                     │     │
│  │  - Heartbeat monitoring (30s interval)             │     │
│  │  - Timeout detection (90s no heartbeat)            │     │
│  │  - Session recovery window (300s)                  │     │
│  │  - Graceful cleanup                                │     │
│  └────────────────────────────────────────────────────┘     │
│                                                               │
└─────────────────────────────────────────────────────────────┘
```

**Data Structures:**
```
DeviceSession {
    device_id: DeviceId,
    connection_id: ConnectionId,
    state: ConnectionState,
    connected_at: Timestamp,
    last_heartbeat: Timestamp,
    metadata: {
        device_type,
        firmware_version,
        region,
        public_key (for E2E)
    },
    endpoint: {
        relay_address,
        connection_id,
        timestamp
    }
}

RecoverySession {
    session: DeviceSession,
    disconnected_at: Timestamp,
    pending_messages: Vec<Message>
}
```

### Layer 3: Stream Routing Layer

```
┌─────────────────────────────────────────────────────────────┐
│                  Stream Routing Layer                        │
├─────────────────────────────────────────────────────────────┤
│                                                               │
│  ┌────────────────────────────────────────────────────┐     │
│  │         Stream Router                               │     │
│  │                                                     │     │
│  │  stream_mappings: StreamId → StreamMapping         │     │
│  │                                                     │     │
│  │  StreamMapping {                                    │     │
│  │    device_stream_id,                                │     │
│  │    controller_stream_id,                            │     │
│  │    device_id,                                       │     │
│  │    controller_id,                                   │     │
│  │    method_name,                                     │     │
│  │    created_at                                       │     │
│  │  }                                                  │     │
│  │                                                     │     │
│  │  Operations:                                        │     │
│  │  - create_mapping()                                 │     │
│  │  - forward_to_device()                              │     │
│  │  - forward_to_controller()                          │     │
│  │  - cleanup_mapping()                                │     │
│  └────────────────────────────────────────────────────┘     │
│                                                               │
│  ┌────────────────────────────────────────────────────┐     │
│  │         Backpressure Handler                        │     │
│  │                                                     │     │
│  │  - Monitor buffer levels                            │     │
│  │  - Apply flow control                               │     │
│  │  - Propagate backpressure signals                   │     │
│  └────────────────────────────────────────────────────┘     │
│                                                               │
└─────────────────────────────────────────────────────────────┘
```

**Stream Flow:**
```
Controller Request → Relay → Device
    ↓                 ↓         ↓
  Encrypt         Metadata   Decrypt
  Payload         Extract    Payload
                  + Audit
                  
Device Response → Relay → Controller
    ↓                ↓          ↓
  Encrypt        Forward    Decrypt
  Payload        (opaque)   Payload
```

### Layer 4: Security Layer

```
┌─────────────────────────────────────────────────────────────┐
│                    Security Layer                            │
├─────────────────────────────────────────────────────────────┤
│                                                               │
│  ┌────────────────────────────────────────────────────┐     │
│  │         Authentication Module                       │     │
│  │                                                     │     │
│  │  Device Auth:                                       │     │
│  │  - mTLS (device certificates)                       │     │
│  │  - Token-based (JWT, 30-day expiry)                │     │
│  │                                                     │     │
│  │  Controller Auth:                                   │     │
│  │  - JWT tokens (1-hour expiry)                       │     │
│  │  - Token refresh mechanism                          │     │
│  │  - Token blacklist (Redis)                          │     │
│  └────────────────────────────────────────────────────┘     │
│                                                               │
│  ┌────────────────────────────────────────────────────┐     │
│  │         Authorization Module (RBAC)                 │     │
│  │                                                     │     │
│  │  Roles:                                             │     │
│  │  - admin: full access                               │     │
│  │  - operator: control + data transfer                │     │
│  │  - viewer: read-only                                │     │
│  │                                                     │     │
│  │  Policy Engine:                                     │     │
│  │  - Check controller → device access                 │     │
│  │  - Validate method permissions                      │     │
│  │  - Project/Tenant isolation                         │     │
│  └────────────────────────────────────────────────────┘     │
│                                                               │
│  ┌────────────────────────────────────────────────────┐     │
│  │         End-to-End Encryption                       │     │
│  │                                                     │     │
│  │  Device ←──────────────────────→ Controller        │     │
│  │         Encrypted Payload                           │     │
│  │         (Relay sees metadata only)                  │     │
│  │                                                     │     │
│  │  Metadata visible to Relay:                         │     │
│  │  - device_id                                        │     │
│  │  - controller_id                                    │     │
│  │  - method_name                                      │     │
│  │  - sequence_number                                  │     │
│  │  - timestamp                                        │     │
│  └────────────────────────────────────────────────────┘     │
│                                                               │
│  ┌────────────────────────────────────────────────────┐     │
│  │         Idempotency Manager                         │     │
│  │                                                     │     │
│  │  LRU Cache (10,000 entries):                        │     │
│  │  sequence_number → cached_response                  │     │
│  │                                                     │     │
│  │  - Detect duplicate requests                        │     │
│  │  - Return cached responses                          │     │
│  │  - Prevent replay attacks                           │     │
│  └────────────────────────────────────────────────────┘     │
│                                                               │
└─────────────────────────────────────────────────────────────┘
```

### Layer 5: Service Discovery Layer

```
┌─────────────────────────────────────────────────────────────┐
│               Service Discovery Layer                        │
├─────────────────────────────────────────────────────────────┤
│                                                               │
│  ┌────────────────────────────────────────────────────┐     │
│  │         MQTT Publisher (Primary)                    │     │
│  │                                                     │     │
│  │  Events Published:                                  │     │
│  │  1. Device Online                                   │     │
│  │     Topic: relay/{relay_id}/device/online           │     │
│  │     QoS: 1, Retained: true                          │     │
│  │     Payload: {                                      │     │
│  │       device_id,                                    │     │
│  │       connection_id,                                │     │
│  │       relay_address,                                │     │
│  │       timestamp,                                    │     │
│  │       metadata                                      │     │
│  │     }                                               │     │
│  │                                                     │     │
│  │  2. Device Offline                                  │     │
│  │     Topic: relay/{relay_id}/device/offline          │     │
│  │     Payload: {                                      │     │
│  │       device_id,                                    │     │
│  │       timestamp,                                    │     │
│  │       reason: "timeout"|"graceful"|"error"          │     │
│  │     }                                               │     │
│  │                                                     │     │
│  │  3. Device Telemetry                                │     │
│  │     Topic: device/{device_id}/telemetry             │     │
│  │     QoS: 0                                          │     │
│  └────────────────────────────────────────────────────┘     │
│                                                               │
│  ┌────────────────────────────────────────────────────┐     │
│  │         gRPC Query Service (Auxiliary)              │     │
│  │                                                     │     │
│  │  RPC: ListOnlineDevices()                           │     │
│  │  - Used for full sync on Controller startup         │     │
│  │  - Fallback when MQTT unavailable                   │     │
│  │  - Supports filtering (region, device_type)         │     │
│  └────────────────────────────────────────────────────┘     │
│                                                               │
│  ┌────────────────────────────────────────────────────┐     │
│  │         stationService Publisher (Backup)           │     │
│  │                                                     │     │
│  │  Topic: station/{station_id}/status                 │     │
│  │  - Device self-reports status                       │     │
│  │  - Controller compares with Relay reports           │     │
│  │  - Detects inconsistencies                          │     │
│  └────────────────────────────────────────────────────┘     │
│                                                               │
└─────────────────────────────────────────────────────────────┘
```

**Discovery Flow:**
```
Device Connects
    ↓
Relay validates & registers
    ↓
Relay publishes to MQTT (relay/{id}/device/online)
    ↓
stationService publishes to MQTT (station/{id}/status)
    ↓
Controller subscribes to both topics
    ↓
Controller validates consistency
    ↓
Controller can query Relay via gRPC if needed
```

### Layer 6: Observability Layer

```
┌─────────────────────────────────────────────────────────────┐
│                 Observability Layer                          │
├─────────────────────────────────────────────────────────────┤
│                                                               │
│  ┌────────────────────────────────────────────────────┐     │
│  │         Metrics Collector (Prometheus)              │     │
│  │                                                     │     │
│  │  System Metrics:                                    │     │
│  │  - relay_cpu_usage                                  │     │
│  │  - relay_memory_usage_bytes                         │     │
│  │  - relay_memory_total_bytes                         │     │
│  │                                                     │     │
│  │  Connection Metrics:                                │     │
│  │  - relay_devices_total                              │     │
│  │  - relay_connections_total                          │     │
│  │  - relay_streams_active                             │     │
│  │  - relay_sessions_recovered_total                   │     │
│  │                                                     │     │
│  │  Performance Metrics:                               │     │
│  │  - relay_request_duration_seconds (histogram)       │     │
│  │  - relay_throughput_bytes_per_second                │     │
│  │  - relay_errors_total (by type)                     │     │
│  │                                                     │     │
│  │  Business Metrics:                                  │     │
│  │  - relay_messages_forwarded_total                   │     │
│  │  - relay_idempotent_requests_total                  │     │
│  │  - relay_auth_failures_total                        │     │
│  └────────────────────────────────────────────────────┘     │
│                                                               │
│  ┌────────────────────────────────────────────────────┐     │
│  │         Structured Logging (tracing)                │     │
│  │                                                     │     │
│  │  Log Levels:                                        │     │
│  │  - ERROR: auth failures, connection errors          │     │
│  │  - WARN: timeouts, retries                          │     │
│  │  - INFO: connections, disconnections                │     │
│  │  - DEBUG: message forwarding                        │     │
│  │  - TRACE: detailed protocol events                  │     │
│  │                                                     │     │
│  │  Structured Fields:                                 │     │
│  │  - device_id, controller_id                         │     │
│  │  - connection_id, stream_id                         │     │
│  │  - method_name, sequence_number                     │     │
│  │  - latency_ms, error_code                           │     │
│  └────────────────────────────────────────────────────┘     │
│                                                               │
│  ┌────────────────────────────────────────────────────┐     │
│  │         Distributed Tracing (OpenTelemetry)         │     │
│  │                                                     │     │
│  │  Trace Context Propagation:                         │     │
│  │  Controller → Relay → Device                        │     │
│  │                                                     │     │
│  │  Spans:                                             │     │
│  │  - controller_request                               │     │
│  │  - relay_authorization                              │     │
│  │  - relay_forward                                    │     │
│  │  - device_processing                                │     │
│  │  - relay_response                                   │     │
│  └────────────────────────────────────────────────────┘     │
│                                                               │
│  ┌────────────────────────────────────────────────────┐     │
│  │         Telemetry Reporter                          │     │
│  │                                                     │     │
│  │  Publishes to MQTT every 30s:                       │     │
│  │  Topic: relay/{relay_id}/telemetry                  │     │
│  │  Payload: {                                         │     │
│  │    system: { cpu, memory },                         │     │
│  │    connections: { devices, streams },               │     │
│  │    performance: { latency, throughput }             │     │
│  │  }                                                  │     │
│  └────────────────────────────────────────────────────┘     │
│                                                               │
└─────────────────────────────────────────────────────────────┘
```

## Core Data Flow Sequences

### Sequence 1: Device Registration and Connection

```
Device                stationService         Relay                MQTT Broker
  |                         |                  |                       |
  |─────── Start ──────────>|                  |                       |
  |                         |                  |                       |
  |                         |── QUIC Connect ─>|                       |
  |                         |   (TLS 1.3)      |                       |
  |                         |                  |                       |
  |                         |<─ TLS Handshake ─|                       |
  |                         |                  |                       |
  |                         |── RegisterReq ──>|                       |
  |                         |   (device_id,    |                       |
  |                         |    auth_token,   |                       |
  |                         |    public_key)   |                       |
  |                         |                  |                       |
  |                         |                  |── Validate Token      |
  |                         |                  |── Check Certificate   |
  |                         |                  |── Generate conn_id    |
  |                         |                  |── Store Session       |
  |                         |                  |                       |
  |                         |<─ RegisterResp ──|                       |
  |                         |   (success,      |                       |
  |                         |    conn_id)      |                       |
  |                         |                  |                       |
  |                         |                  |── Publish ───────────>|
  |                         |                  |   relay/{id}/device/  |
  |                         |                  |   online              |
  |                         |                  |                       |
  |                         |── Publish ──────────────────────────────>|
  |                         |   station/{id}/status                    |
  |                         |                  |                       |
  |                         |── Heartbeat ────>|                       |
  |                         |   (every 30s)    |                       |
  |                         |                  |                       |
```

### Sequence 2: Controller Discovers and Connects to Device

```
Controller            Relay                MQTT Broker           Device
  |                     |                       |                   |
  |── Subscribe ───────────────────────────────>|                   |
  |   relay/+/device/online                     |                   |
  |                     |                       |                   |
  |<──── Notification ──────────────────────────|                   |
  |   (device_001 online)                       |                   |
  |                     |                       |                   |
  |── ListDevices() ───>|                       |                   |
  |   (verify)          |                       |                   |
  |                     |                       |                   |
  |<─ DeviceList ───────|                       |                   |
  |   (device_001 confirmed)                    |                   |
  |                     |                       |                   |
  |── ConnectToDevice ─>|                       |                   |
  |   (device_id,       |                       |                   |
  |    method_name,     |                       |                   |
  |    encrypted_payload,                       |                   |
  |    auth_token)      |                       |                   |
  |                     |                       |                   |
  |                     |── Validate Token      |                   |
  |                     |── Check RBAC          |                   |
  |                     |── Check Idempotency   |                   |
  |                     |── Create Stream Map   |                   |
  |                     |                       |                   |
  |                     |── Forward (opaque) ──────────────────────>|
  |                     |   encrypted_payload   |                   |
  |                     |                       |                   |
  |                     |<─ Response ──────────────────────────────|
  |                     |   encrypted_payload   |                   |
  |                     |                       |                   |
  |                     |── Cache Response      |                   |
  |                     |   (for idempotency)   |                   |
  |                     |                       |                   |
  |<─ Response ─────────|                       |                   |
  |   encrypted_payload |                       |                   |
  |                     |                       |                   |
```

### Sequence 3: Device Reconnection and Session Recovery

```
Device                Relay                MQTT Broker
  |                     |                       |
  |── Network Loss ────>|                       |
  |                     |                       |
  |                     |── Detect Timeout      |
  |                     |   (90s no heartbeat)  |
  |                     |                       |
  |                     |── Move to Recovery    |
  |                     |   Buffer (300s TTL)   |
  |                     |                       |
  |                     |── Publish ───────────>|
  |                     |   relay/{id}/device/  |
  |                     |   offline             |
  |                     |                       |
  |── Network Restored ─|                       |
  |                     |                       |
  |── QUIC Reconnect ──>|                       |
  |   (new IP address)  |                       |
  |                     |                       |
  |── RegisterReq ─────>|                       |
  |   (device_id,       |                       |
  |    old_conn_id)     |                       |
  |                     |                       |
  |                     |── Check Recovery      |
  |                     |   Buffer              |
  |                     |── Found Session       |
  |                     |   (< 300s)            |
  |                     |── Generate new        |
  |                     |   conn_id             |
  |                     |── Restore Session     |
  |                     |                       |
  |<─ RegisterResp ─────|                       |
  |   (session_resumed, |                       |
  |    new_conn_id)     |                       |
  |                     |                       |
  |<─ Pending Messages ─|                       |
  |   (buffered during  |                       |
  |    disconnect)      |                       |
  |                     |                       |
  |                     |── Publish ───────────>|
  |                     |   relay/{id}/device/  |
  |                     |   online              |
  |                     |                       |
```

## Deployment Architecture

### Single-Node Deployment (MVP v1.0)

```
┌─────────────────────────────────────────────────────────────┐
│                     Single Host                              │
├─────────────────────────────────────────────────────────────┤
│                                                               │
│  ┌──────────────────────────────────────────────────────┐   │
│  │  Relay Server (Rust Binary)                          │   │
│  │  - Port 4433/UDP (QUIC for devices)                  │   │
│  │  - Port 8080/TCP (HTTP/2 for controllers)            │   │
│  │  - Port 9090/TCP (Prometheus metrics)                │   │
│  └──────────────────────────────────────────────────────┘   │
│                                                               │
│  ┌──────────────────────────────────────────────────────┐   │
│  │  MQTT Broker (EMQX)                                   │   │
│  │  - Port 1883 (MQTT)                                   │   │
│  │  - Port 18083 (Dashboard)                             │   │
│  └──────────────────────────────────────────────────────┘   │
│                                                               │
│  ┌──────────────────────────────────────────────────────┐   │
│  │  Monitoring Stack                                     │   │
│  │  - Prometheus (metrics storage)                       │   │
│  │  - Grafana (visualization)                            │   │
│  └──────────────────────────────────────────────────────┘   │
│                                                               │
│  Resources:                                                   │
│  - CPU: 8 cores                                              │
│  - Memory: 16 GB                                             │
│  - Network: 1 Gbps                                           │
│  - Capacity: 10K concurrent devices                          │
│                                                               │
└─────────────────────────────────────────────────────────────┘
```

### Multi-Node Deployment (v2.0)

```
┌─────────────────────────────────────────────────────────────┐
│                    Load Balancer Layer                       │
│                                                               │
│  ┌──────────────────────────────────────────────────────┐   │
│  │  L4 Load Balancer (UDP-aware)                         │   │
│  │  - QUIC Connection ID routing                         │   │
│  │  - Consistent hashing                                 │   │
│  │  - Health checks                                      │   │
│  └──────────────────────────────────────────────────────┘   │
└─────────────────────────────────────────────────────────────┘
                              │
        ┌─────────────────────┼─────────────────────┐
        │                     │                     │
        ▼                     ▼                     ▼
┌──────────────┐      ┌──────────────┐      ┌──────────────┐
│ Relay Node 1 │      │ Relay Node 2 │      │ Relay Node N │
│              │      │              │      │              │
│ 10K devices  │      │ 10K devices  │      │ 10K devices  │
└──────┬───────┘      └──────┬───────┘      └──────┬───────┘
       │                     │                     │
       └─────────────────────┼─────────────────────┘
                             │
                             ▼
              ┌──────────────────────────┐
              │   Shared Services        │
              │                          │
              │  - MQTT Broker Cluster   │
              │  - Redis (token cache)   │
              │  - Prometheus            │
              │  - Grafana               │
              └──────────────────────────┘
```

## Technology Stack

### Core Components

| Component | Technology | Rationale |
|-----------|-----------|-----------|
| **QUIC Implementation** | Quinn | Pure Rust, production-ready, excellent performance |
| **gRPC Framework** | Tonic | De facto standard for
```
| **gRPC Framework** | Tonic | De facto standard for Rust gRPC, good ecosystem |
| **Async Runtime** | Tokio | Industry standard, mature, excellent performance |
| **MQTT Client** | rumqttc | Pure Rust, async-first, reliable |
| **TLS** | rustls | Memory-safe, modern TLS 1.3 implementation |
| **Serialization** | Prost (Protobuf) | Fast, type-safe, gRPC native |
| **Concurrent Collections** | DashMap | Lock-free HashMap, high concurrency |
| **Metrics** | Prometheus crate | Standard observability format |
| **Tracing** | tracing + OpenTelemetry | Structured logging + distributed tracing |
| **Configuration** | config-rs | Flexible, supports multiple formats |
| **Error Handling** | anyhow + thiserror | Ergonomic error handling |
```

### External Dependencies

| Service | Technology | Purpose |
|---------|-----------|---------|
| **MQTT Broker** | EMQX 5.x | High-performance, clustering support, MQTT 5.0 |
| **Metrics Storage** | Prometheus | Time-series database for metrics |
| **Visualization** | Grafana | Dashboards and alerting |
| **Tracing Backend** | Jaeger | Distributed tracing analysis |
| **Container Runtime** | Docker + Kubernetes | Deployment and orchestration |

## Performance Architecture

### Resource Allocation Model

```
┌─────────────────────────────────────────────────────────────┐
│              Relay Server Resource Model                     │
│              (16 GB RAM, 8 CPU cores)                        │
├─────────────────────────────────────────────────────────────┤
│                                                               │
│  Memory Allocation:                                          │
│  ┌────────────────────────────────────────────────────┐     │
│  │ Device Sessions (10K × 2 KB)        = 20 MB       │     │
│  │ Stream Mappings (1K × 1 KB)         = 1 MB        │     │
│  │ Idempotency Cache (10K × 10 KB)     = 100 MB      │     │
│  │ Recovery Sessions (1K × 5 KB)       = 5 MB        │     │
│  │ QUIC Buffers (10K × 64 KB)          = 640 MB      │     │
│  │ Application Buffers                  = 500 MB      │     │
│  │ Rust Runtime + Libraries             = 200 MB      │     │
│  │ OS + Overhead                        = 534 MB      │     │
│  │                                                     │     │
│  │ Total                                = ~2 GB       │     │
│  │ Safety Margin                        = 14 GB       │     │
│  └────────────────────────────────────────────────────┘     │
│                                                               │
│  CPU Allocation:                                             │
│  ┌────────────────────────────────────────────────────┐     │
│  │ QUIC I/O (2 cores)                                 │     │
│  │ gRPC Handling (2 cores)                            │     │
│  │ Stream Routing (2 cores)                           │     │
│  │ Background Tasks (1 core)                          │     │
│  │ OS + Overhead (1 core)                             │     │
│  └────────────────────────────────────────────────────┘     │
│                                                               │
│  Network Bandwidth:                                          │
│  ┌────────────────────────────────────────────────────┐     │
│  │ 1K active streams × 10 MB/s = 10 Gbps theoretical  │     │
│  │ Actual capacity with 1 Gbps NIC = 100 streams      │     │
│  │ at full speed, or 1K streams at 1 MB/s each        │     │
│  └────────────────────────────────────────────────────┘     │
│                                                               │
└─────────────────────────────────────────────────────────────┘
```

### Latency Budget

```
┌─────────────────────────────────────────────────────────────┐
│                    End-to-End Latency                        │
│              Controller → Relay → Device                     │
├─────────────────────────────────────────────────────────────┤
│                                                               │
│  Controller → Relay (HTTP/2):                                │
│  ├─ Network latency                    5-20 ms               │
│  ├─ TLS handshake (first request)      20-50 ms              │
│  └─ gRPC overhead                       1-2 ms               │
│                                                               │
│  Relay Processing:                                           │
│  ├─ Token validation                    0.5 ms               │
│  ├─ RBAC check                          0.5 ms               │
│  ├─ Idempotency check                   0.2 ms               │
│  ├─ Stream mapping lookup               0.1 ms               │
│  ├─ Message forwarding                  0.5 ms               │
│  └─ Audit logging (async)               0 ms (non-blocking)  │
│  Total Relay overhead:                  1.8 ms (P50)         │
│                                         5-20 ms (P99)        │
│                                                               │
│  Relay → Device (QUIC):                                      │
│  ├─ Network latency                     5-50 ms              │
│  ├─ QUIC overhead                       0.5-1 ms             │
│  └─ 0-RTT (reconnection)                0 ms                 │
│                                                               │
│  Device Processing:                                          │
│  └─ Application logic                   10-1000 ms           │
│                                                               │
│  Total Round Trip:                                           │
│  ├─ Best case (LAN, cached)             25 ms                │
│  ├─ Typical (Internet)                  50-100 ms            │
│  └─ Worst case (high latency network)   200+ ms              │
│                                                               │
└─────────────────────────────────────────────────────────────┘
```

### Throughput Model

```
┌─────────────────────────────────────────────────────────────┐
│                    Throughput Capacity                       │
├─────────────────────────────────────────────────────────────┤
│                                                               │
│  Single Stream:                                              │
│  ├─ Maximum: 10 MB/s (80 Mbps)                              │
│  ├─ Typical: 5 MB/s (40 Mbps)                               │
│  └─ Minimum: 1 MB/s (8 Mbps)                                │
│                                                               │
│  Aggregate (1 Gbps NIC):                                     │
│  ├─ 100 streams @ 10 MB/s each                              │
│  ├─ 200 streams @ 5 MB/s each                               │
│  └─ 1000 streams @ 1 MB/s each                              │
│                                                               │
│  Message Rate:                                               │
│  ├─ Small messages (1 KB): 100K msg/s                       │
│  ├─ Medium messages (10 KB): 50K msg/s                      │
│  └─ Large messages (100 KB): 10K msg/s                      │
│                                                               │
└─────────────────────────────────────────────────────────────┘
```

## Scalability Architecture

### Horizontal Scaling Strategy

```
┌─────────────────────────────────────────────────────────────┐
│                  Scaling Dimensions                          │
├─────────────────────────────────────────────────────────────┤
│                                                               │
│  1. Connection Scaling (Device Growth)                       │
│     ┌──────────────────────────────────────────────┐        │
│     │ Single Node:    10K devices                  │        │
│     │ 2 Nodes:        20K devices                  │        │
│     │ N Nodes:        N × 10K devices              │        │
│     │                                               │        │
│     │ Scaling Trigger:                             │        │
│     │ - Device count > 8K (80% capacity)           │        │
│     │ - CPU usage > 70%                            │        │
│     │ - Memory usage > 12 GB                       │        │
│     └──────────────────────────────────────────────┘        │
│                                                               │
│  2. Throughput Scaling (Traffic Growth)                      │
│     ┌──────────────────────────────────────────────┐        │
│     │ Add nodes when:                              │        │
│     │ - Network utilization > 70%                  │        │
│     │ - P99 latency > 50 ms                        │        │
│     │ - Active streams > 800                       │        │
│     └──────────────────────────────────────────────┘        │
│                                                               │
│  3. Geographic Scaling (Multi-Region)                        │
│     ┌──────────────────────────────────────────────┐        │
│     │ Region 1: Relay Cluster (Asia)               │        │
│     │ Region 2: Relay Cluster (Europe)             │        │
│     │ Region 3: Relay Cluster (Americas)           │        │
│     │                                               │        │
│     │ Shared: MQTT Broker Cluster (global)         │        │
│     └──────────────────────────────────────────────┘        │
│                                                               │
└─────────────────────────────────────────────────────────────┘
```

### Load Balancing Strategy

```
┌─────────────────────────────────────────────────────────────┐
│              Load Balancing Architecture                     │
├─────────────────────────────────────────────────────────────┤
│                                                               │
│  Device Connections (QUIC):                                  │
│  ┌────────────────────────────────────────────────────┐     │
│  │ Method: Consistent Hashing on device_id            │     │
│  │                                                     │     │
│  │ Benefits:                                           │     │
│  │ - Same device always routes to same node           │     │
│  │ - Minimizes session disruption on scaling          │     │
│  │ - Supports connection migration                    │     │
│  │                                                     │     │
│  │ Implementation:                                     │     │
│  │ - DNS-based (simple, MVP)                          │     │
│  │ - Anycast (advanced, v2.0)                         │     │
│  │ - L4 LB with QUIC CID routing (v2.0)              │     │
│  └────────────────────────────────────────────────────┘     │
│                                                               │
│  Controller Connections (HTTP/2):                            │
│  ┌────────────────────────────────────────────────────┐     │
│  │ Method: Round-robin or least-connections           │     │
│  │                                                     │     │
│  │ Benefits:                                           │     │
│  │ - Simple, stateless                                │     │
│  │ - Even distribution                                │     │
│  │ - Standard L7 load balancer                        │     │
│  └────────────────────────────────────────────────────┘     │
│                                                               │
└─────────────────────────────────────────────────────────────┘
```

## Failure Handling Architecture

### Failure Scenarios and Recovery

```
┌─────────────────────────────────────────────────────────────┐
│                  Failure Scenarios                           │
├─────────────────────────────────────────────────────────────┤
│                                                               │
│  1. Device Network Failure                                   │
│     ┌──────────────────────────────────────────────┐        │
│     │ Detection: No heartbeat for 90s              │        │
│     │ Action:                                       │        │
│     │ - Mark device offline                        │        │
│     │ - Move session to recovery buffer            │        │
│     │ - Publish offline event to MQTT              │        │
│     │ - Keep session for 300s                      │        │
│     │                                               │        │
│     │ Recovery:                                     │        │
│     │ - Device reconnects within 300s              │        │
│     │ - Session restored automatically             │        │
│     │ - Pending messages delivered                 │        │
│     └──────────────────────────────────────────────┘        │
│                                                               │
│  2. Relay Node Failure                                       │
│     ┌──────────────────────────────────────────────┐        │
│     │ Detection: Health check failure              │        │
│     │ Action:                                       │        │
│     │ - Load balancer removes node                 │        │
│     │ - Devices reconnect to healthy nodes         │        │
│     │ - QUIC 0-RTT for fast reconnection          │        │
│     │                                               │        │
│     │ Impact:                                       │        │
│     │ - ~10K devices need to reconnect             │        │
│     │ - Reconnection time: 1-5 seconds             │        │
│     │ - No data loss (E2E encryption)              │        │
│     └──────────────────────────────────────────────┘        │
│                                                               │
│  3. MQTT Broker Failure                                      │
│     ┌──────────────────────────────────────────────┐        │
│     │ Detection: Connection timeout                │        │
│     │ Action:                                       │        │
│     │ - Relay continues forwarding (degraded)      │        │
│     │ - Service discovery via gRPC only            │        │
│     │ - Buffer events for republishing             │        │
│     │                                               │        │
│     │ Recovery:                                     │        │
│     │ - Reconnect to MQTT                          │        │
│     │ - Republish buffered events                  │        │
│     │ - Resume normal operation                    │        │
│     └──────────────────────────────────────────────┘        │
│                                                               │
│  4. Controller Connection Failure                            │
│     ┌──────────────────────────────────────────────┐        │
│     │ Detection: Stream closed                     │        │
│     │ Action:                                       │        │
│     │ - Clean up stream mapping                    │        │
│     │ - Release resources                          │        │
│     │ - Log event                                  │        │
│     │                                               │        │
│     │ Recovery:                                     │        │
│     │ - Controller reconnects                      │        │
│     │ - Reestablish stream                         │        │
│     │ - Idempotency prevents duplicate processing  │        │
│     └──────────────────────────────────────────────┘        │
│                                                               │
│  5. Partial Network Partition                                │
│     ┌──────────────────────────────────────────────┐        │
│     │ Scenario: Device can't reach Relay,          │        │
│     │           but can reach MQTT                 │        │
│     │                                               │        │
│     │ Detection:                                    │        │
│     │ - stationService publishes status to MQTT    │        │
│     │ - Relay doesn't see device connection        │        │
│     │ - Controller detects inconsistency           │        │
│     │                                               │        │
│     │ Action:                                       │        │
│     │ - Alert operator                             │        │
│     │ - Device attempts reconnection               │        │
│     │ - Fallback to alternative Relay if available │        │
│     └──────────────────────────────────────────────┘        │
│                                                               │
└─────────────────────────────────────────────────────────────┘
```

### Circuit Breaker Pattern

```
┌─────────────────────────────────────────────────────────────┐
│              Circuit Breaker for Backend Services            │
├─────────────────────────────────────────────────────────────┤
│                                                               │
│  States:                                                     │
│  ┌────────────────────────────────────────────────────┐     │
│  │                                                     │     │
│  │  CLOSED ──────> OPEN ──────> HALF_OPEN             │     │
│  │    │              │              │                  │     │
│  │    │              │              │                  │     │
│  │    └──────────────┴──────────────┘                 │     │
│  │                                                     │     │
│  └────────────────────────────────────────────────────┘     │
│                                                               │
│  Thresholds:                                                 │
│  - Error rate > 50% over 10s → OPEN                         │
│  - Timeout rate > 30% → OPEN                                │
│  - Open duration: 30s                                        │
│  - Half-open test requests: 3                               │
│                                                               │
│  Actions when OPEN:                                          │
│  - Return cached response (if available)                     │
│  - Return error to controller                                │
│  - Log circuit breaker event                                 │
│  - Alert monitoring system                                   │
│                                                               │
└─────────────────────────────────────────────────────────────┘
```

## Security Architecture

### Defense in Depth

```
┌─────────────────────────────────────────────────────────────┐
│                    Security Layers                           │
├─────────────────────────────────────────────────────────────┤
│                                                               │
│  Layer 1: Network Security                                   │
│  ┌────────────────────────────────────────────────────┐     │
│  │ - Firewall rules (allow only 4433/UDP, 8080/TCP)  │     │
│  │ - DDoS protection (rate limiting at edge)          │     │
│  │ - IP allowlisting for controllers                  │     │
│  │ - VPC/private network for internal services        │     │
│  └────────────────────────────────────────────────────┘     │
│                                                               │
│  Layer 2: Transport Security                                 │
│  ┌────────────────────────────────────────────────────┐     │
│  │ - TLS 1.3 for all connections                      │     │
│  │ - Strong cipher suites only                        │     │
│  │ - Certificate pinning (devices)                    │     │
│  │ - mTLS for device authentication                   │     │
│  └────────────────────────────────────────────────────┘     │
│                                                               │
│  Layer 3: Authentication                                     │
│  ┌────────────────────────────────────────────────────┐     │
│  │ Devices:                                           │     │
│  │ - X.509 certificates (preferred)                   │     │
│  │ - JWT tokens (fallback)                            │     │
│  │ - Token rotation every 30 days                     │     │
│  │                                                     │     │
│  │ Controllers:                                        │     │
│  │ - JWT tokens (1-hour expiry)                       │     │
│  │ - Refresh tokens (7-day expiry)                    │     │
│  │ - Token revocation list (Redis)                    │     │
│  └────────────────────────────────────────────────────┘     │
│                                                               │
│  Layer 4: Authorization (RBAC)                               │
│  ┌────────────────────────────────────────────────────┐     │
│  │ Policy Enforcement:                                │     │
│  │ - Controller → Device access control              │     │
│  │ - Method-level permissions                         │     │
│  │ - Tenant/project isolation                         │     │
│  │ - Time-based access (optional)                     │     │
│  └────────────────────────────────────────────────────┘     │
│                                                               │
│  Layer 5: Application Security                               │
│  ┌────────────────────────────────────────────────────┐     │
│  │ - Input validation (all messages)                  │     │
│  │ - Rate limiting (per device, per controller)       │     │
│  │ - Payload size limits (10 MB default)              │     │
│  │ - Sequence number validation (anti-replay)         │     │
│  └────────────────────────────────────────────────────┘     │
│                                                               │
│  Layer 6: Data Security                                      │
│  ┌────────────────────────────────────────────────────┐     │
│  │ - End-to-end encryption (Device ↔ Controller)     │     │
│  │ - Relay sees metadata only                         │     │
│  │ - No persistent storage of payloads                │     │
│  │ - Secure key exchange (X25519)                     │     │
│  └────────────────────────────────────────────────────┘     │
│                                                               │
│  Layer 7: Audit & Monitoring                                 │
│  ┌────────────────────────────────────────────────────┐     │
│  │ - All authentication attempts logged               │     │
│  │ - Authorization failures logged                    │     │
│  │ - Connection events logged                         │     │
│  │ - Anomaly detection (unusual patterns)             │     │
│  └────────────────────────────────────────────────────┘     │
│                                                               │
└─────────────────────────────────────────────────────────────┘
```

### Rate Limiting Strategy

```
┌─────────────────────────────────────────────────────────────┐
│                    Rate Limiting                             │
├─────────────────────────────────────────────────────────────┤
│                                                               │
│  Per-Device Limits:                                          │
│  ┌────────────────────────────────────────────────────┐     │
│  │ - Connection attempts: 10/minute                   │     │
│  │ - Messages: 1000/second                            │     │
│  │ - Bandwidth: 10 MB/s per stream                    │     │
│  │ - Concurrent streams: 10                           │     │
│  └────────────────────────────────────────────────────┘     │
│                                                               │
│  Per-Controller Limits:                                      │
│  ┌────────────────────────────────────────────────────┐     │
│  │ - API calls: 1000/minute                           │     │
│  │ - Concurrent connections: 100 devices              │     │
│  │ - Bandwidth: 100 MB/s aggregate                    │     │
│  └────────────────────────────────────────────────────┘     │
│                                                               │
│  Global Limits:                                              │
│  ┌────────────────────────────────────────────────────┐     │
│  │ - New connections: 100/second                      │     │
│  │ - Total bandwidth: 800 Mbps (80% of 1 Gbps)       │     │
│  │ - CPU usage: 80% threshold                         │     │
│  │ - Memory usage: 12 GB threshold                    │     │
│  └────────────────────────────────────────────────────┘     │
│                                                               │
│  Implementation:                                             │
│  - Token bucket algorithm                                    │
│  - Sliding window counters                                   │
│  - Distributed rate limiting (Redis for multi-node)          │
│                                                               │
└─────────────────────────────────────────────────────────────┘
```

## Operational Architecture

### Health Checks

```
┌─────────────────────────────────────────────────────────────┐
│                    Health Check System                       │
├─────────────────────────────────────────────────────────────┤
│                                                               │
│  Liveness Probe:                                             │
│  ┌────────────────────────────────────────────────────┐     │
│  │ Endpoint: GET /health/live                         │     │
│  │ Checks: Process is running                         │     │
│  │ Interval: 10s                                      │     │
│  │ Timeout: 1s                                        │     │
│  │ Failure threshold: 3                               │     │
│  └────────────────────────────────────────────────────┘     │
│                                                               │
│  Readiness Probe:                                            │
│  ┌────────────────────────────────────────────────────┐     │
│  │ Endpoint: GET /health/ready                        │     │
│  │ Checks:                                            │     │
│  │ - MQTT connection active                           │     │
│  │ - CPU usage < 90%                                  │     │
│  │ - Memory usage < 14 GB                             │     │
│  │ - Can accept new connections                       │     │
│  │ Interval: 5s                                       │     │
│  │ Timeout: 2s                                        │     │
│  │ Failure threshold: 2                               │     │
│  └────────────────────────────────────────────────────┘     │
│                                                               │
│  Startup Probe:                                              │
│  ┌────────────────────────────────────────────────────┐     │
│  │ Endpoint: GET /health/startup                      │     │
│  │ Checks:                                            │     │
│  │ - Configuration loaded                             │     │
│  │ - TLS certificates valid                           │     │
│  │ - QUIC endpoint listening                          │     │
│  │ - gRPC server listening                            │     │
│  │ - MQTT connected                                   │     │
│  │ Interval: 10s                                      │     │
│  │ Timeout: 5s                                        │     │
│  │ Failure threshold: 30 (5 minutes)                  │     │
│  └────────────────────────────────────────────────────┘     │
│                                                               │
└─────────────────────────────────────────────────────────────┘
```

### Graceful Shutdown

```
┌─────────────────────────────────────────────────────────────┐
│                  Graceful Shutdown Sequence                  │
├─────────────────────────────────────────────────────────────┤
│                                                               │
│  1. Receive SIGTERM                                          │
│     ↓                                                        │
│  2. Mark as not ready (health check fails)                   │
│     ↓                                                        │
│  3. Stop accepting new connections                           │
│     ↓                                                        │
│  4. Wait for in-flight requests (max 30s)                    │
│     ↓                                                        │
│  5. Close device connections gracefully                      │
│     - Send GOAWAY frames                                     │
│     - Allow devices to reconnect to other nodes              │
│     ↓                                                        │
│  6. Flush telemetry and logs                                 │
│     ↓                                                        │
│  7. Close MQTT connection                                    │
│     ↓                                                        │
│  8. Exit process                                             │
│                                                               │
│  Total shutdown time: 30-60 seconds                          │
│                                                               │
└─────────────────────────────────────────────────────────────┘
```

## Summary: Key Architectural Decisions

| Decision | Choice | Rationale |
|----------|--------|-----------|
| **Language** | Rust | Memory safety, performance, async ecosystem |
| **Device Transport** | QUIC (Quinn) | Low latency, connection migration, multiplexing |
| **Controller Transport** | HTTP/2 (Tonic) | Maturity, ecosystem, upgrade path to QUIC |
| **Service Discovery** | MQTT (primary) + gRPC (auxiliary) | Reliability through redundancy |
| **State Management** | In-memory (DashMap) | Low latency, stateless design |
| **Session Recovery** | 300s window | Balance between UX and resource usage |
| **Idempotency** | LRU cache (10K entries) | Prevent duplicates, bounded memory |
| **Security Model** | E2E encryption + metadata visibility | Compliance + control |
| **Authorization** | RBAC | Flexible, auditable |
| **Observability** | Prometheus + OpenTelemetry | Industry standard |
| **Deployment** | Docker + Kubernetes | Portability, scalability |
| **MVP Scope** | Single node | Validate before scaling |

This architecture adapts the generic QUIC proxy to meet the specific requirements of the gRPC-Relay system, with emphasis on IoT device management, service discovery via MQTT, end-to-end encryption, and production-grade reliability.