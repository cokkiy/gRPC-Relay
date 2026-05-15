# Manual Build

Build the `relay` binary from source, or build the Docker image locally.

## Prerequisites

| Dependency | Version | Required | Notes |
|---|---|---|---|
| Rust | stable | Yes | Install via [rustup](https://rustup.rs) |
| OpenSSL | libssl-dev | Yes | Linking dependency for TLS |
| pkg-config | any | Yes | Build dependency detection |

Protobuf compiler is **not** required — `relay-proto` vendors `protoc` via `protoc-bin-vendored`.

### Install system dependencies

```bash
# Debian / Ubuntu
sudo apt-get install -y build-essential libssl-dev pkg-config

# Fedora / RHEL
sudo dnf install -y gcc openssl-devel pkg-config

# Arch
sudo pacman -S --needed base-devel openssl pkg-config

# macOS
brew install openssl pkg-config
```

### Install Rust

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
source ~/.cargo/env
rustup component add rustfmt clippy
```

Or use the repo's `rust-toolchain.toml` — rustup auto-selects the correct channel and components when you run `cargo` in the project root.

## Build

```bash
# Clone the repository
git clone https://github.com/cokkiy/gRPC-Relay.git
cd gRPC-Relay

# Release build (optimized, stripped by default)
cargo build --release --locked

# Debug build (faster to compile, no optimizations)
cargo build --locked
```

The binary is at `target/release/relay` (release) or `target/debug/relay` (debug).

### Build options

| Command | Output | Use case |
|---|---|---|
| `cargo build --release --locked` | `target/release/relay` | Production deployment |
| `cargo build --locked` | `target/debug/relay` | Development / quick iteration |
| `cargo build --release --locked -p relay` | `target/release/relay` | Build only the relay crate (skip SDKs) |
| `cargo build --release --locked --frozen` | `target/release/relay` | CI: fail if Cargo.lock is stale |

`--locked` ensures dependencies match `Cargo.lock` exactly, producing reproducible builds.

### Build time

First build pulls and compiles all dependencies (~200 crates). Subsequent builds are incremental. On a typical developer machine:

- **First build**: 5-10 minutes
- **Incremental (no dep changes)**: 30-60 seconds

## Verify

```bash
# Run the full test suite
cargo test --workspace --locked

# Check formatting
cargo fmt --all --check

# Run clippy lints
cargo clippy --workspace --all-targets -- -D warnings

# Quick compile check (no binary output)
cargo check --workspace
```

## Run

```bash
# Default config path: config/relay.yaml
./target/release/relay

# Custom config path
./target/release/relay --config /etc/grpc-relay/relay.yaml

# Show version
./target/release/relay --version
```

### CLI reference

```
relay --config <path>
      --version
      --help
```

| Flag | Default | Description |
|---|---|---|
| `--config` | `config/relay.yaml` | Path to the relay YAML configuration file |
| `--version` | — | Print version and exit |

### Ports

| Port | Protocol | Purpose |
|---|---|---|
| `50051` | TCP (gRPC) | Controller and device gRPC connections |
| `50052` | UDP (QUIC) | Optional QUIC transport |
| `8080` | TCP (HTTP) | Readiness checks (`/health/ready`), liveness checks (`/health/live`), and Prometheus metrics (`/metrics`) |

## Docker image

Build the container image locally using the repo's `Dockerfile`. Requires Docker or a compatible OCI builder.

### Prerequisites

- Docker 24+ or compatible (Podman, containerd)
- `docker buildx` (included with Docker Desktop, or `sudo apt-get install docker-buildx` on Linux)

### Build

```bash
# Clone the repository
git clone https://github.com/cokkiy/gRPC-Relay.git
cd gRPC-Relay

# Build the image (tags it as grpc-relay:latest)
docker build -t grpc-relay:latest .

# With a custom tag
docker build -t grpc-relay:v1.0.0-alpha .

# Build for a different platform
docker build --platform linux/arm64 -t grpc-relay:latest .
```

### Build stages

The `Dockerfile` is a multi-stage build:

| Stage | Base image | What it does |
|---|---|---|
| **builder** | `debian:bookworm-slim` | Installs Rust + system deps (libssl-dev, pkg-config, protobuf-compiler), copies manifests, pre-fetches crates with dummy sources, then `cargo build --release --locked` |
| **runtime** | `debian:bookworm-slim` | Copies the `relay` binary from builder, installs only `ca-certificates` + `curl` at runtime, creates non-root `relay` user, sets `HEALTHCHECK` |

### Build caching

The Dockerfile is structured to cache Rust dependencies separately from source code changes:

```bash
# First build: full compile (5-15 minutes)
docker build -t grpc-relay:latest .

# Subsequent builds with only source changes:
# Cargo.toml unchanged → dependency layer is cached → only recompiles your code (1-2 minutes)
docker build -t grpc-relay:latest .
```

For CI environments, GHA caching is configured in `.github/workflows/ci.yml` using `type=gha`.

### Run the built image

```bash
# With docker-compose (uses the locally built image)
# First edit docker-compose.yml: comment "image:" and uncomment "build: ."
docker compose up -d

# Or run standalone
docker run -d \
  --name relay \
  -p 50051:50051 -p 50052:50052/udp -p 8080:8080 \
  -v ./config/relay.yaml:/etc/relay/relay.yaml:ro \
  --env-file .env \
  grpc-relay:latest

# Verify
curl http://localhost:8080/health
docker logs relay
```

### Push to a registry

```bash
# Tag for your registry
docker tag grpc-relay:latest ghcr.io/YOUR_USER/grpc-relay:latest

# Login and push
docker login ghcr.io -u YOUR_USER
docker push ghcr.io/YOUR_USER/grpc-relay:latest
```

### Build troubleshooting

**Buildkit not available** — set the legacy builder:

```bash
DOCKER_BUILDKIT=0 docker build -t grpc-relay:latest .
```

**Cargo fetch fails** — the Dockerfile uses `cargo fetch --locked`. If `Cargo.lock` is stale:

```bash
# Regenerate the lockfile first
cargo generate-lockfile
docker build -t grpc-relay:latest .
```

**Out of disk during build** — the builder stage accumulates Rust artifacts. Clean up after build:

```bash
docker builder prune --filter "until=24h"
```

## CI pipeline

The CI workflow (`.github/workflows/ci.yml`) runs on pushes to `master` and `main`, on pull requests, and on version tags matching `v*`:

1. **Check**: `cargo fmt --check`, `cargo clippy`, `cargo check --workspace`
2. **Test**: `cargo test --workspace --lib` and `--tests` (with Mosquitto service container)
3. **Coverage**: `cargo llvm-cov --workspace --fail-under-lines 80`
4. **Docker Build**: builds the image via Docker Buildx with GHA caching on CI runs; image publishing to `ghcr.io/cokkiy/grpc-relay` only happens for the workflow's tag/release publishing path, not for ordinary branch or PR builds

## Troubleshooting

### OpenSSL headers not found

```bash
# Debian/Ubuntu
sudo apt-get install -y libssl-dev pkg-config

# Set OPENSSL_DIR if installed in a custom location
export OPENSSL_DIR=/usr/local/opt/openssl
export PKG_CONFIG_PATH=$OPENSSL_DIR/lib/pkgconfig
```

### protoc errors

`relay-proto` uses `protoc-bin-vendored` which bundles protoc. If you see protoc errors:

```bash
# Verify the vendored protoc binary is accessible
cargo clean -p relay-proto
cargo build -p relay-proto
```

### Linker errors / out of memory

The release build can consume significant RAM during linking. If the linker is killed:

```bash
# Reduce parallel codegen units
export CARGO_BUILD_JOBS=2
cargo build --release --locked

# Or use the LLVM linker (faster, less memory)
# Install: sudo apt-get install lld
export RUSTFLAGS="-C link-arg=-fuse-ld=lld"
cargo build --release --locked
```

### Linker errors (missing `-lssl` / `-lcrypto`)

Ensure OpenSSL development libraries are installed (see system dependencies above). If using a non-standard OpenSSL location, set:

```bash
export OPENSSL_LIB_DIR=/path/to/openssl/lib
export OPENSSL_INCLUDE_DIR=/path/to/openssl/include
```
