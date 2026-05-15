FROM debian:bookworm-slim AS builder
WORKDIR /app

ENV CARGO_HOME=/usr/local/cargo
ENV RUSTUP_HOME=/usr/local/rustup
ENV PATH=/usr/local/cargo/bin:${PATH}

RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        build-essential \
        ca-certificates \
        curl \
        libssl-dev \
        pkg-config \
        protobuf-compiler \
    && rm -rf /var/lib/apt/lists/*

COPY rust-toolchain.toml ./
RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal \
    && rustup component add rustfmt clippy

# Copy manifests first for dependency caching
COPY Cargo.toml ./
COPY crates/relay/Cargo.toml crates/relay/
COPY crates/relay-proto/Cargo.toml crates/relay-proto/
COPY crates/device-sdk/Cargo.toml crates/device-sdk/
COPY crates/controller-sdk/Cargo.toml crates/controller-sdk/
COPY config/ config/

# Create dummy src/lib.rs so cargo can pre-fetch dependencies
RUN mkdir -p crates/relay/src crates/relay-proto/src crates/device-sdk/src crates/controller-sdk/src && \
    echo 'fn main() {}' > crates/relay/src/main.rs && \
    echo '' > crates/relay-proto/src/lib.rs && \
    echo '' > crates/device-sdk/src/lib.rs && \
    echo '' > crates/controller-sdk/src/lib.rs && \
    cargo generate-lockfile && \
    cargo fetch --locked && \
    rm -rf crates/relay/src crates/relay-proto/src crates/device-sdk/src crates/controller-sdk/src

# Copy real source and build
COPY crates/ crates/
RUN cargo build --release --locked

# Stage 2: Runtime
FROM debian:bookworm-slim

RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates curl \
    && rm -rf /var/lib/apt/lists/*

# Create non-root user
RUN groupadd -r relay && useradd -r -g relay -d /var/lib/relay -s /sbin/nologin relay

COPY --from=builder /app/target/release/relay /usr/local/bin/relay
COPY config/relay.yaml /etc/relay/relay.yaml

RUN mkdir -p /var/log/relay && chown -R relay:relay /var/log/relay /etc/relay

USER relay

EXPOSE 50051/tcp 50052/udp 8080/tcp

HEALTHCHECK --interval=30s --timeout=10s --retries=3 --start-period=10s \
    CMD ["curl", "-f", "http://localhost:8080/health/ready"]

# OCI labels
LABEL org.opencontainers.image.title="gRPC-Relay" \
      org.opencontainers.image.description="Cross-network gRPC relay server" \
      org.opencontainers.image.licenses="MIT" \
      org.opencontainers.image.source="https://github.com/gRPC-Relay/gRPC-Relay"

CMD ["relay", "--config", "/etc/relay/relay.yaml"]
