# Docker Deployment

Docker and Docker Compose deployment for gRPC-Relay with optional MQTT, Prometheus, Grafana, and Jaeger sidecars.

Pre-built images are published to [GitHub Container Registry](https://github.com/cokkiy/gRPC-Relay/pkgs/container/grpc-relay) (`ghcr.io/cokkiy/grpc-relay`). The compose file uses the GHCR image by default — no local Rust toolchain needed.

[![GHCR Image](https://img.shields.io/github/v/release/cokkiy/gRPC-Relay?label=ghcr.io%2Fcokkiy%2Fgrpc-relay&color=blue)](https://github.com/cokkiy/gRPC-Relay/pkgs/container/grpc-relay)

## Available tags

Images are published to [ghcr.io/cokkiy/grpc-relay](https://github.com/cokkiy/gRPC-Relay/pkgs/container/grpc-relay) on every release. Available tags:

| Tag | Source | When |
|---|---|---|
| `latest` | Latest published release | Every release |
| `v<version>` | Specific release (e.g., `v1.0.0-alpha`) | Every release |
| `<sha>` | Specific commit | Every release (via CI) |

List all published tags:

```bash
# Via GitHub CLI
gh api repos/cokkiy/gRPC-Relay/packages/container/grpc-relay/versions --jq '.[].metadata.container.tags[]' | sort -V

# Or check the GHCR page
# https://github.com/cokkiy/gRPC-Relay/pkgs/container/grpc-relay
```

## Files

| File | Location | Purpose |
|---|---|---|
| `Dockerfile` | repo root | Multi-stage Rust build (builder + slim runtime) |
| `docker-compose.yml` | repo root | Orchestrates relay + mqtt + prometheus + grafana + jaeger |
| `.dockerignore` | repo root | Excludes build artifacts, docs, IDE files from context |
| `.env.example` | `deploy/docker/` | Environment variable template |

## Quick start

```bash
# 1. Create the env file from the template
cp deploy/docker/.env.example .env

# 2. Edit .env and set required values:
#    - RELAY__AUTH__JWT__HS256_SECRET (generate with: openssl rand -hex 32)
#    - GRAFANA_ADMIN_PASSWORD

# 3. Pull images and start all services
docker compose pull
docker compose up -d

# 4. Verify
curl http://localhost:8080/health
```

## Using a specific version

```bash
# Pin to a release tag
export RELAY_VERSION=v1.0.0-alpha
docker compose up -d
```

Override the image tag in your `.env` or export `RELAY_VERSION` before running compose.

### Building locally instead

To build from source instead of pulling the GHCR image, edit `docker-compose.yml`:
comment the `image:` line and uncomment `build: .` under the `relay` service.

## Services

| Service | Port | Description |
|---|---|---|
| `relay` | `50051` (gRPC TCP), `50052` (QUIC UDP), `8080` (health/metrics) | The relay server |
| `mqtt` | `1883` (MQTT), `9001` (WebSocket) | Eclipse Mosquitto 2.x broker |
| `prometheus` | `9090` | Metrics collection and storage (30d retention) |
| `grafana` | `3000` | Dashboards (default login: `admin` / value of `GRAFANA_ADMIN_PASSWORD`) |
| `jaeger` | `16686` (UI), `4317` (OTLP gRPC), `4318` (OTLP HTTP) | Distributed tracing |

## Run standalone container

```bash
# Pull the pre-built image from GHCR
docker pull ghcr.io/cokkiy/grpc-relay:latest

# Run standalone (requires config mounted)
docker run -d \
  --name relay \
  -p 50051:50051 -p 50052:50052/udp -p 8080:8080 \
  -v ./config/relay.yaml:/etc/relay/relay.yaml:ro \
  --env-file .env \
  ghcr.io/cokkiy/grpc-relay:latest
```

### Building locally

```bash
docker build -t grpc-relay:latest .
docker run -d \
  --name relay \
  -p 50051:50051 -p 50052:50052/udp -p 8080:8080 \
  -v ./config/relay.yaml:/etc/relay/relay.yaml:ro \
  --env-file .env \
  grpc-relay:latest
```

## Image details

- **Base image**: `debian:bookworm-slim`
- **Builder stage**: installs Rust toolchain, pre-fetches dependencies via dummy sources, then builds `--release --locked`
- **Runtime stage**: copies the `relay` binary and config, creates non-root `relay` user, drops privileges
- **OCI labels**: `org.opencontainers.image.*` set for title, description, license, source
- **Healthcheck**: `curl -f http://localhost:8080/health` every 30s

## Configuration overrides

The relay config at `config/relay.yaml` is mounted read-only. Override values at runtime via environment variables using the `RELAY__<section>__<key>` convention (config-rs style):

```bash
# Override MQTT broker
RELAY__MQTT__BROKER_ADDRESS=my-mqtt:1883

# Enable TLS
RELAY__TLS__ENABLED=true
RELAY__TLS__CERT_PATH=/etc/relay/tls/server.crt
RELAY__TLS__KEY_PATH=/etc/relay/tls/server.key
```

Mount TLS certificates as additional volumes when TLS is enabled:

```yaml
volumes:
  - ./certs/server.crt:/etc/relay/tls/server.crt:ro
  - ./certs/server.key:/etc/relay/tls/server.key:ro
```

## Volumes

| Volume | Purpose |
|---|---|
| `relay-logs` | Relay audit and application logs |
| `prometheus-data` | Prometheus TSDB (30d retention) |
| `grafana-data` | Grafana dashboards, users, preferences |

## Networking

All services share the `relay-network` bridge network. Internal service discovery:

- Relay reaches MQTT at `mqtt:1883`
- Prometheus scrapes relay at `relay:8080`
- Grafana queries Prometheus at `prometheus:9090`
- Jaeger receives traces at `jaeger:4317`
