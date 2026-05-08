# stationService Device SDK

This document describes the SDK that an external `stationService` application can use to connect devices to gRPC-Relay.

`stationService` is intentionally not implemented in this repository. The crate in `crates/device-sdk` is the integration boundary for that separate application.

## Completion Status

The third action-plan item, "设备侧接入链路", is partially complete in this repository:

| Capability | SDK status | Relay status |
| --- | --- | --- |
| DeviceConnect client stream | Implemented | Proto exists; server skeleton only |
| Register message | Implemented | Not wired in Relay server |
| Heartbeat loop | Implemented | Not wired in Relay server |
| Disconnect handling | Reconnects after stream close | Session cleanup not implemented |
| Session recovery request | Sends `previous_connection_id` inside recovery window | Recovery registry not implemented |
| TCP fallback transport | Implemented via tonic HTTP/2 endpoint | Relay gRPC server not started yet |
| QUIC transport | Config field reserved | Not implemented |
| Controller-to-device data response | Handler callback + `DataResponse` implemented | Stream router not implemented |

Conclusion: the SDK side for stationService is available and compiles, but the full device-side access path is not end-to-end successful until Relay implements `DeviceConnect`, session management, and stream routing.

## Crate

Add the SDK as a path dependency from the stationService application:

```toml
[dependencies]
device-sdk = { path = "../gRPC-Relay/crates/device-sdk" }
anyhow = "1"
async-trait = "0.1"
tokio = { version = "1", features = ["macros", "rt-multi-thread"] }
tracing-subscriber = { version = "0.3", features = ["env-filter", "fmt"] }
```

If stationService is in a different repository, publish this crate or vendor it as a git/path dependency according to your build pipeline.

## Configuration

The SDK supports a YAML/TOML/JSON config file plus `STATION_SERVICE__*` overrides.

Example:

```yaml
relay:
  tcp_addr: "127.0.0.1:50051"
  quic_addr: null

device_id: "device-001"
token: "device-token"

metadata:
  region: "us-west"
  device_type: "iot-sensor"
  firmware_version: "1.0.0"

session_recovery_window_seconds: 300
heartbeat_interval_seconds: 30
backoff_initial_seconds: 1
backoff_max_seconds: 60

transport:
  max_payload_bytes: 10485760
  enable_tcp_fallback: true
```

Environment override example:

```bash
export STATION_SERVICE_CONFIG=crates/device-sdk/examples/station_service.yaml
export STATION_SERVICE__RELAY__TCP_ADDR=relay.example.com:50051
export STATION_SERVICE__DEVICE_ID=device-001
export STATION_SERVICE__TOKEN=device-token
```

For simple deployments without a config file:

```bash
export RELAY_TCP_ADDR=127.0.0.1:50051
export DEVICE_ID=device-001
export DEVICE_TOKEN=device-token
export DEVICE_METADATA_REGION=us-west
export DEVICE_METADATA_DEVICE_TYPE=iot-sensor
```

## Minimal stationService Code

```rust
use device_sdk::{
    handler::{DataRequestContext, DeviceDataHandler, EncryptedPayload},
    DeviceConnectClient, DeviceSdkConfig,
};

#[derive(Clone, Default)]
struct StationHandler;

#[async_trait::async_trait]
impl DeviceDataHandler for StationHandler {
    async fn on_data_request(
        &self,
        ctx: DataRequestContext,
        encrypted_payload: EncryptedPayload,
    ) -> anyhow::Result<EncryptedPayload> {
        tracing::info!(
            device_id = %ctx.device_id,
            connection_id = %ctx.connection_id,
            sequence_number = ctx.sequence_number,
            payload_bytes = encrypted_payload.len(),
            "received opaque controller payload"
        );

        // Decrypt/execute/encrypt in stationService business code.
        Ok(encrypted_payload)
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt().with_env_filter("info").init();

    let config = DeviceSdkConfig::load("station_service.yaml")?;
    let client = DeviceConnectClient::new(config, StationHandler)?;
    client.run().await
}
```

## Runtime Behavior

`DeviceConnectClient::run()` is a long-running loop:

1. Connect to Relay using `relay.tcp_addr`.
2. Send `RegisterRequest`.
3. Start heartbeat after `RegisterResponse`.
4. Dispatch each `DataRequest` to `DeviceDataHandler::on_data_request`.
5. Send `DataResponse` with the same `sequence_number`.
6. On stream close or gRPC transport error, reconnect with exponential backoff.
7. During the recovery window, include `previous_connection_id` on the next register request.

The SDK treats `encrypted_payload` as opaque bytes. End-to-end encryption belongs in stationService and Controller code, not inside Relay.

## Run the Example

```bash
export STATION_SERVICE_CONFIG=crates/device-sdk/examples/station_service.yaml
cargo run -p device-sdk --example station_service_minimal
```

The example requires a Relay implementation that serves `RelayService::DeviceConnect`. The current Relay binary only starts the health endpoint, so this example currently validates SDK compilation and application wiring rather than a live end-to-end session.
