FROM rust:1.75 AS builder
WORKDIR /app
COPY . .
RUN cargo build --release --locked

FROM debian:bookworm-slim
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates curl \
    && rm -rf /var/lib/apt/lists/*

COPY --from=builder /app/target/release/relay /usr/local/bin/relay
COPY config/relay.yaml /etc/relay/relay.yaml

EXPOSE 50051/tcp 50052/udp 8080/tcp
CMD ["relay", "--config", "/etc/relay/relay.yaml"]
