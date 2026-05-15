# Deploy

Deployment assets for running gRPC-Relay via Docker Compose, on bare-metal Linux hosts, or on Kubernetes clusters, with optional Prometheus/Grafana monitoring and MQTT broker integration. See [BUILD.md](BUILD.md) for manual build instructions (binary and Docker image).

## Directory structure

```
deploy/
├── docker/            # Docker Compose deployment (Dockerfile and compose at repo root)
├── bare-metal/        # systemd-based native Linux deployment
├── kubernetes/        # Kustomize-based Kubernetes manifests
├── prometheus/        # Prometheus scrape configuration
├── grafana/           # Grafana dashboards and datasources
├── mosquitto/         # MQTT broker config placeholder
├── BUILD.md           # Manual build from source
└── README.md
```

---

## Bare-metal

Native Linux deployment via `systemd`. Suitable for single-host or small-scale deployments without container orchestration.

### Files

| File | Purpose |
|---|---|
| `relay.service` | systemd unit definition |
| `relay.env.example` | environment variable template |
| `install.sh` | install binary, config, env file, and enable/start the service |
| `upgrade.sh` | replace binary, optionally refresh config, restart service |
| `uninstall.sh` | stop, disable, and remove the service and binary |

### Host layout

| Path | Purpose |
|---|---|
| `/usr/local/bin/relay` | relay binary |
| `/etc/grpc-relay/relay.yaml` | main configuration file |
| `/etc/grpc-relay/relay.env` | host-specific environment overrides |
| `/etc/grpc-relay/tls/` | TLS certificate and key |
| `/var/log/grpc-relay/` | audit and application logs |
| `/var/lib/grpc-relay/` | runtime data directory |

### Quick start

```bash
# Build the binary first, then:
sudo ./deploy/bare-metal/install.sh

# Edit secrets
sudo vi /etc/grpc-relay/relay.env

# Verify
systemctl status relay
curl http://127.0.0.1:8080/health
```

### Upgrade

```bash
sudo ./deploy/bare-metal/upgrade.sh

# To also refresh the config from config/relay.yaml:
sudo UPDATE_CONFIG=true ./deploy/bare-metal/upgrade.sh
```

### Uninstall

```bash
sudo ./deploy/bare-metal/uninstall.sh

# To also remove /etc/grpc-relay, logs, and runtime data:
sudo REMOVE_DATA=true ./deploy/bare-metal/uninstall.sh
```

### Configuration

All scripts accept environment variable overrides:

| Variable | Default |
|---|---|
| `BINARY_SOURCE` | `../../target/release/relay` |
| `CONFIG_SOURCE` | `../../config/relay.yaml` |
| `INSTALL_BINARY_PATH` | `/usr/local/bin/relay` |
| `CONFIG_DIR` | `/etc/grpc-relay` |
| `SERVICE_USER` / `SERVICE_GROUP` | `relay` |

---

## Kubernetes

Kustomize-based manifests for deploying gRPC-Relay on a Kubernetes cluster. All resources live in the `relay-system` namespace.

### Apply

```bash
kubectl apply -k deploy/kubernetes
```

### Resources

| Resource | File | Purpose |
|---|---|---|
| Namespace | `namespace.yaml` | `relay-system` namespace |
| ServiceAccount | `serviceaccount.yaml` | `relay` service account |
| ConfigMap | `configmap.yaml` | Full relay YAML config (mounted at `/etc/relay/relay.yaml`) |
| Secret | `secret.yaml` | JWT HS256 secret, TLS cert/key |
| Deployment | `deployment.yaml` | Single-replica relay pod with health probes, resource limits, anti-affinity |
| Service | `service.yaml` | ClusterIP exposing gRPC (`:50051`) and health (`:8080`) |
| HPA | `hpa.yaml` | CPU (70%) and memory (80%) autoscaling, 1-10 replicas |
| PDB | `pdb.yaml` | PodDisruptionBudget ensuring `minAvailable: 1` |
| NetworkPolicy | `networkpolicy.yaml` | Restricts ingress to gRPC and health ports within the namespace |
| Kustomization | `kustomization.yaml` | Resource list, common labels, image tag |

### Pod details

- **Image**: `ghcr.io/cokkiy/grpc-relay:latest` (override via kustomize `images`)
- **Ports**: gRPC `50051`, health/metrics `8080`
- **Probes**: liveness (`/health`), readiness (`/health`), startup (`/health/startup`)
- **Resources**: requests `2 CPU / 4 Gi`, limits `8 CPU / 16 Gi`
- **Security**: non-root user (1000), read-only root filesystem, no privilege escalation, all capabilities dropped
- **Anti-affinity**: prefers scheduling pods on different nodes

### Secrets

Before deploying to production, replace placeholder values in `secret.yaml`:

```bash
# Generate a JWT secret
openssl rand -hex 32

# Provide real TLS certificates
# Edit tls.crt and tls.key in secret.yaml
```

---

## Docker

Docker Compose stack bundling relay with MQTT, Prometheus, Grafana, and Jaeger. Pre-built images are published to `ghcr.io/cokkiy/grpc-relay` — no local Rust toolchain required. The `Dockerfile`, `docker-compose.yml`, and `.dockerignore` live at the repo root; `deploy/docker/` contains the env template and docs.

### Files

| File | Location | Purpose |
|---|---|---|
| `Dockerfile` | repo root | Multi-stage Rust build (bookworm-slim builder + slim runtime) |
| `docker-compose.yml` | repo root | Orchestrates relay, mqtt, prometheus, grafana, jaeger |
| `.dockerignore` | repo root | Excludes build artifacts, docs, IDE files |
| `.env.example` | `deploy/docker/` | Environment variable template |

### Quick start

```bash
# 1. Create env file
cp deploy/docker/.env.example .env

# 2. Edit .env — set JWT secret and Grafana password
vi .env

# 3. Pull pre-built images and start
docker compose pull
docker compose up -d

# 4. Verify
curl http://localhost:8080/health
```

### Image tags

Images are published to [ghcr.io/cokkiy/grpc-relay](https://github.com/cokkiy/gRPC-Relay/pkgs/container/grpc-relay). Available tags:

| Tag | Source |
|---|---|
| `latest` | Latest published release |
| `v<version>` | Specific release (e.g., `v1.0.0-alpha`) |
| `<sha>` | Specific commit |

To use a specific version:

```bash
export RELAY_VERSION=v1.0.0-alpha
docker compose up -d
```

### Services

| Service | Port(s) | Notes |
|---|---|---|
| `relay` | `50051` (gRPC), `50052` (QUIC/UDP), `8080` (health/metrics) | Pulled from `ghcr.io/cokkiy/grpc-relay` |
| `mqtt` | `1883`, `9001` (WS) | Eclipse Mosquitto 2.x |
| `prometheus` | `9090` | 30d retention, scrapes relay at `relay:8080` |
| `grafana` | `3000` | Pre-provisioned dashboards and Prometheus datasource |
| `jaeger` | `16686` (UI), `4317` (OTLP), `4318` (OTLP HTTP) | All-in-one tracing |

### Standalone container run

```bash
# Pull the pre-built image from GHCR
docker pull ghcr.io/cokkiy/grpc-relay:latest

docker run -d \
  --name relay \
  -p 50051:50051 -p 50052:50052/udp -p 8080:8080 \
  -v ./config/relay.yaml:/etc/relay/relay.yaml:ro \
  --env-file .env \
  ghcr.io/cokkiy/grpc-relay:latest
```

### Image details

- **Builder**: installs Rust via rustup, copies manifests, pre-fetches deps with dummy sources, then `cargo build --release --locked`
- **Runtime**: `debian:bookworm-slim`, non-root `relay` user, `HEALTHCHECK` via curl
- **OCI labels**: title, description, license (MIT), source URL

### Configuration

The config at `config/relay.yaml` is mounted read-only. Override values via environment variables using `RELAY__<section>__<key>` (config-rs convention):

```bash
RELAY__MQTT__BROKER_ADDRESS=my-mqtt:1883
RELAY__TLS__ENABLED=true
```

### Volumes

| Volume | Purpose |
|---|---|
| `relay-logs` | Relay audit and application logs |
| `prometheus-data` | Prometheus TSDB (30d retention) |
| `grafana-data` | Grafana state (users, preferences) |

---

## Prometheus

Scrape configuration targeting the relay metrics endpoint and Prometheus self-monitoring.

### Scrape targets

| Job | Target | Path |
|---|---|---|
| `relay` | `relay:8080` | `/metrics` |
| `prometheus` | `localhost:9090` | `/metrics` |

Deploy with:

```bash
# Copy to your Prometheus config directory or ConfigMap
cp deploy/prometheus/prometheus.yml /etc/prometheus/prometheus.yml
```

---

## Grafana

Provisioning configuration for Grafana dashboards and Prometheus datasource.

### Files

| Path | Purpose |
|---|---|
| `datasources/prometheus.yml` | Configures Prometheus as the default datasource (expects it at `http://prometheus:9090`) |
| `dashboards/dashboards.yml` | Dashboard provider pointing to `/etc/grafana/provisioning/dashboards` |
| `dashboards/relay-overview.json` | Pre-built relay overview dashboard (339 lines, JSON model) |

Deploy by mounting into your Grafana instance's provisioning path:

```bash
# Datasource
cp deploy/grafana/datasources/prometheus.yml /etc/grafana/provisioning/datasources/
# Dashboards
cp deploy/grafana/dashboards/*.yml /etc/grafana/provisioning/dashboards/
cp deploy/grafana/dashboards/relay-overview.json /etc/grafana/provisioning/dashboards/
```

---

## Mosquitto

Placeholder directory for MQTT broker (Mosquitto) configuration. Populate with `mosquitto.conf` and TLS certificates when MQTT is enabled in the relay config.
