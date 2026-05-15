# gRPC-Relay | [中文版](README-cn_ZH.md)

[![CI](https://github.com/cokkiy/gRPC-Relay/actions/workflows/ci.yml/badge.svg)](https://github.com/cokkiy/gRPC-Relay/actions/workflows/ci.yml)
[![Release](https://github.com/cokkiy/gRPC-Relay/actions/workflows/release.yml/badge.svg)](https://github.com/cokkiy/gRPC-Relay/actions/workflows/release.yml)
[![Create Release](https://github.com/cokkiy/gRPC-Relay/actions/workflows/create-release.yml/badge.svg)](https://github.com/cokkiy/gRPC-Relay/actions/workflows/create-release.yml)
[![relay-proto](https://img.shields.io/crates/v/relay-proto?label=relay-proto)](https://crates.io/crates/relay-proto)
[![device-sdk](https://img.shields.io/crates/v/device-sdk?label=device-sdk)](https://crates.io/crates/device-sdk)
[![controller-sdk](https://img.shields.io/crates/v/controller-sdk?label=controller-sdk)](https://crates.io/crates/controller-sdk)

gRPC-Relay is a cross-domain communication relay system designed to establish a secure, controllable, and high-performance gRPC channel between internal devices and external controllers.

It is intended for the following scenarios:

- Internal devices managed by a public-network or office-network Controller through a Relay
- Bidirectional streaming data transfer, including control commands and file/data uploads
- MQTT-based device online/offline notifications, status reporting, and telemetry
- gRPC-based online device discovery and streaming relay capabilities

---

## Table of Contents

- [Background and Goals](#background-and-goals)
- [Core Roles](#core-roles)
- [System Architecture](#system-architecture)
- [Core Workflows](#core-workflows)
- [API Design](#api-design)
- [Security and Authorization Model](#security-and-authorization-model)
- [Non-Functional Requirements](#non-functional-requirements)
- [CI/CD](#cicd)
- [Deployment and Operations](#deployment-and-operations)
- [Testing Strategy](#testing-strategy)
- [MVP Scope and Roadmap](#mvp-scope-and-roadmap)
- [References](#references)

---

## Background and Goals

The core goal of gRPC-Relay is to provide cross-domain gRPC relay capability so that devices inside private networks, without public IP addresses, can still be accessed and managed securely by external controllers.

### Design Principles

- **Controllable relay**: Relay sees metadata only and never decrypts business payloads
- **End-to-end encryption**: Business data is encrypted/decrypted only by Device and Controller
- **Availability-first baseline**: Deliver a single-node MVP first, then expand to multi-node
- **Observable by default**: Built-in health checks, metrics, logs, audit, and tracing
- **Graceful degradation**: Fall back to TLS/TCP when QUIC is unavailable

---

## Core Roles

| Role | Description | Responsibilities |
|------|------|------|
| Device | Physical device such as an IoT device or workstation | Runs stationService and executes business logic |
| stationService | Agent process running on the device | Maintains long-lived connection with Relay, registers, heartbeats, reconnects, reports status |
| Controller | Human-operated control system | Discovers devices, initiates sessions, sends control commands, receives responses |
| Relay | Relay server | Manages long-lived connections, forwards traffic, publishes notifications, provides query APIs |
| MQTT Broker | Message broker | Transmits telemetry data and device online/offline notifications |

---

## System Architecture

### Protocol Layers

| Link | Protocol | Purpose |
|------|----------|---------|
| Device ↔ Relay | gRPC over QUIC | Long-lived device connection, low-latency transport |
| Controller ↔ Relay | gRPC over HTTP/2 + TLS 1.3 | Controller access and querying |
| Relay ↔ MQTT Broker | MQTT over TLS 1.3 | Device notifications and telemetry |
| Fallback | TLS/TCP | Used when QUIC is unavailable |

### Architectural Characteristics

- Relay handles metadata, authentication, authorization, rate limiting, and stream forwarding only
- Business payloads between Device and Controller are end-to-end encrypted
- MQTT Broker is deployed independently and decoupled from Relay
- The first release uses a single Relay node, with multi-node and load balancing in later versions

---

## Core Workflows

### 1. Device Registration and Online Status

1. Device starts stationService
2. stationService connects to Relay
3. Relay verifies device identity
4. Relay assigns a `connection_id`
5. Relay publishes a device online event to MQTT
6. stationService may optionally publish its own status as backup validation

### 2. Heartbeat and Liveness

- stationService sends a heartbeat every 30 seconds
- Relay updates the device `last_seen`
- If no heartbeat is received for 120 seconds, the device is marked as suspected offline
- If no heartbeat is received for 300 seconds, Relay closes the connection and publishes an offline event

### 3. Device Discovery by Controller

Three complementary discovery methods are supported:

- Relay publishes online/offline events through MQTT
- stationService reports status through MQTT
- Controller queries the online device list through gRPC

### 4. Controller Session Initiation

1. Controller obtains target device information
2. Controller connects to Relay and specifies `target_device_id`
3. Relay verifies Controller identity and permissions
4. Relay creates a stream mapping between Controller and Device
5. Relay starts forwarding bidirectional stream data

### 5. Device Reconnect and Session Recovery

- Device reconnects automatically after disconnection
- Reconnect requests include `previous_connection_id`
- Relay attempts to restore the session within the recovery window
- If recovery fails, a new session is created and a new `connection_id` is assigned

### 6. Idempotency

- Requests carry a globally unique `sequence_number`
- Relay caches recently processed sequence numbers
- Duplicate requests return cached responses to avoid repeated execution

---

## API Design

### gRPC Services

Core services include:

- `DeviceConnect(stream DeviceMessage) returns (stream RelayMessage)`
- `ListOnlineDevices(ListOnlineDevicesRequest) returns (ListOnlineDevicesResponse)`
- `ConnectToDevice(stream ControllerMessage) returns (stream DeviceResponse)`
- `RevokeToken(RevokeTokenRequest) returns (RevokeTokenResponse)`

### Key Messages

- `DeviceMessage`: device registration, heartbeat, data reporting
- `RelayMessage`: registration response, heartbeat response, data request
- `ControllerMessage`: request from controller to device
- `DeviceResponse`: response from device
- `ListOnlineDevicesRequest/Response`: online device query
- `RevokeTokenRequest/Response`: admin token revocation

### MQTT Topics

| Topic | Purpose |
|------|---------|
| `relay/device/online` | Device online notification |
| `relay/device/offline` | Device offline notification |
| `device/{device_id}/status` | Device self-reported status |
| `telemetry/{device_id}` | Device telemetry data |
| `telemetry/relay/{relay_id}` | Relay telemetry data |

### Error Codes

- `OK`
- `DEVICE_OFFLINE`
- `UNAUTHORIZED`
- `DEVICE_NOT_FOUND`
- `RATE_LIMITED`
- `INTERNAL_ERROR`

---

## Security and Authorization Model

### Authentication

- **Device**: mTLS device certificates are recommended, with pre-provisioned tokens as an alternative
- **Controller**: HS256 JWT token authentication with `controller_id`, `role`, allowed projects, expiry, issuer, and audience claims

### Authorization

The system uses **RBAC + device ownership**:

- `admin`: access all devices
- `operator`: access authorized devices and perform control/data transfer
- `viewer`: read-only access

### Security Requirements

- All connections must use TLS 1.3
- Business payloads must be end-to-end encrypted
- Relay must not log encrypted payload contents
- Rate limiting must apply at device, Controller, and global levels
- Metadata such as `device_id`, `controller_id`, and `method_name` must be validated
- Admin Controllers can revoke Controller or Device tokens through the gRPC `RevokeToken` API; the current MVP/P1 implementation keeps revocations in Relay memory

---

## Non-Functional Requirements

### Performance Targets

| Metric | Target |
|--------|--------|
| Single-instance long-lived connections | 10,000 |
| Concurrent active streams | 1,000 |
| Relay additional hop latency | P50 < 5ms, P99 < 20ms |
| Maximum single-stream bandwidth | 10 MB/s |
| Memory budget | < 2 GB for 10K connections |
| CPU usage | < 80% at 10K connections and 1K active streams |

### Availability Targets

- Service availability: 99.9%
- Device reconnect time: < 10 seconds
- Session recovery success rate: > 95%
- MTTR: < 5 minutes

### Observability

The system provides:

- `/health` health check (with component-level status)
- Full Prometheus `/metrics` endpoint (connection, stream, latency, error, resource metrics)
- Structured JSON logging (via `tracing-subscriber`)
- Audit logging (auth events, connections, rate limits, errors)
- OpenTelemetry distributed tracing (OTLP exporter, configurable sampling)
- MQTT relay telemetry publishing
- Built-in alerting engine (CPU, memory, MQTT, connection thresholds)

## CI/CD

Three GitHub Actions workflows automate quality checks, releases, and publishing.

| Workflow | Trigger | What it does |
|----------|---------|--------------|
| **[CI](https://github.com/cokkiy/gRPC-Relay/actions/workflows/ci.yml)** | push (master/main), PR (master/main), tag, manual | `cargo fmt --check` → `cargo clippy` → `cargo check` → unit tests + integration tests → coverage (80% threshold) → Docker build |
| **[Create Release](https://github.com/cokkiy/gRPC-Relay/actions/workflows/create-release.yml)** | manual (`workflow_dispatch`) | Validates version vs `Cargo.toml`, runs full test suite, builds release binary, verifies `relay --version`, creates git tag, generates categorized release notes, creates GitHub release, triggers **Release** |
| **[Release](https://github.com/cokkiy/gRPC-Relay/actions/workflows/release.yml)** | `release: published` | Publishes `relay-proto` to crates.io, waits for index propagation, publishes `device-sdk` and `controller-sdk`, builds and pushes Docker image to GHCR |

### Release Flow

```
  prepare-release.sh          PR merge              create-release.yml       release.yml (auto)
  (local: bumps version,  →   (CI validates    →    (tag + GitHub       →    (crates.io + GHCR
   opens a PR)                on the branch)        release)                 Docker image)
```

See [`doc/RELEASE.md`](doc/RELEASE.md) for the full release process, including SemVer guidance, rollback procedures, and troubleshooting.

---

## Deployment and Operations

### Deployment Options

| Method | Directory | What's included |
|--------|-----------|-----------------|
| **Bare Metal** | [`deploy/bare-metal/`](deploy/bare-metal/) | systemd service, install/uninstall/upgrade scripts, env template |
| **Docker** | `Dockerfile`, `docker-compose.yml` | Multi-stage Rust build, slim runtime image, Compose with MQTT + Prometheus + Grafana |
| **Kubernetes** | [`deploy/kubernetes/`](deploy/kubernetes/) | Deployment, Service, ConfigMap, Secret, HPA, NetworkPolicy, PDB, ServiceAccount, Namespace, Kustomization |

### Monitoring Stack

| Component | Path | Purpose |
|-----------|------|---------|
| **Grafana** | [`deploy/grafana/`](deploy/grafana/) | Pre-built `relay-overview` dashboard + Prometheus datasource |
| **Prometheus** | [`deploy/prometheus/`](deploy/prometheus/) | Scrape config targeting relay metrics endpoint |
| **Mosquitto** | [`deploy/mosquitto/`](deploy/mosquitto/) | MQTT broker configuration |

### Recommended Ports

| Port | Protocol | Purpose |
|------|----------|---------|
| `50051` | TCP | gRPC (HTTP/2) |
| `50052` | UDP | gRPC over QUIC (v2.0) |
| `8080` | TCP | `/health` and `/metrics` |
| `8883` | TCP | MQTT over TLS |

### Configuration

The relay server is configured via a single YAML file ([example](config/relay.yaml)). Key sections:

| Section | Contents |
|---------|----------|
| `relay` | id, address, QUIC address, max connections, heartbeat interval |
| `relay.stream` | idle timeout, max active streams, per-controller limits |
| `relay.rate_limiting` | per-device/controller/global request + connection + bandwidth limits, CPU/memory thresholds |
| `relay.idempotency` | cache capacity + TTL |
| `relay.auth` | enable flag, token maps (device + controller), method whitelist, JWT config |
| `relay.mqtt` | enable flag, broker address, credentials, telemetry interval, reconnect config |
| `relay.tls` | enable flag, cert/key/CA paths |
| `observability` | logging level/format, health bind, audit config, OpenTelemetry tracing, alerting rules |

---

## Testing Strategy

### Unit Tests

Coverage includes:

- Authentication and authorization
- Sequence number deduplication
- Session management
- Rate limiting
- Error handling

### Integration Tests

Coverage includes:

1. Device connection and registration
2. Controller session initiation
3. Bidirectional data transfer
4. Device reconnect and session recovery
5. MQTT notifications and queries
6. Authentication failure handling
7. Authorization rejection handling
8. Rate limit triggering

### Performance Tests

- 10K concurrent connections
- 1K concurrent active streams
- Latency target validation
- Long-running stability validation

### Security Tests

- Unauthenticated access
- Forged tokens
- Cross-device privilege escalation
- DDoS simulation
- Large payload attacks
- Replay attacks

---

## MVP Scope and Roadmap

### v1.0 MVP

The first release focuses on:

- Device ↔ Relay QUIC connection
- Controller ↔ Relay HTTP/2 connection
- Bidirectional stream relay
- Registration, heartbeat, reconnect, offline handling
- MQTT online/offline notifications
- Controller online device query
- RBAC authorization
- Idempotency
- End-to-end encryption
- Basic rate limiting and input validation
- Metrics, logs, and audit
- Relay telemetry
- Health checks
- Docker / Kubernetes deployment

### Future Versions

- v1.1: Session persistence and stronger recovery
- v1.2: Multi-Relay nodes, high availability, load balancing
- v2.0: Controller QUIC, connection migration, 0-RTT, ABAC

---

## References

- [gRPC Official Documentation](https://grpc.io/docs/)
- [QUIC RFC 9000](https://www.rfc-editor.org/rfc/rfc9000.html)
- [MQTT v5.0 Specification](https://docs.oasis-open.org/mqtt/mqtt/v5.0/mqtt-v5.0.html)
- [OpenTelemetry Documentation](https://opentelemetry.io/docs/)
- [Prometheus Best Practices](https://prometheus.io/docs/practices/)

---

## Document Notes

This README was created based on the following project documents:

- `doc/requirements.md`
- `doc/architecture.md`
- `doc/action_plan.md`
- `doc/RELEASE.md`
- `doc/v1.0_release_summary.md`

It is intended as a user-facing entry document that emphasizes project overview, architecture, and implementation path.
