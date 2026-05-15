# Controller SDK

This document describes the SDK that an external Controller application can use to query devices and open Controller-to-Device sessions through gRPC-Relay.

`Controller` is intentionally not implemented in this repository. The crate in `crates/controller-sdk` is the integration boundary for that separate application.

## Completion Status

The fourth action-plan item, "Controller 接入链路", is complete in this repository within the defined scope of "external program + SDK":

| Capability                          | SDK status            | Relay status                      |
| ----------------------------------- | --------------------- | --------------------------------- |
| Controller auth parameter injection | Implemented           | Implemented                       |
| `ListOnlineDevices` request         | Implemented           | Implemented                       |
| `region_filter` support             | Implemented           | Implemented                       |
| `ConnectToDevice` bidi stream       | Implemented           | Implemented                       |
| `sequence_number` response matching | Implemented           | Implemented                       |
| Controller-side error mapping       | Implemented           | Implemented                       |
| RBAC / project filtering            | Transparent to caller | Implemented                       |
| Request idempotency / replay cache  | Transparent to caller | Implemented                       |
| Example integration program         | Implemented           | Works with running Relay + Device |

Conclusion: the repository now contains both sides required for the Controller integration path:

1. a reusable SDK for external Controller programs
2. Relay-side `ListOnlineDevices` and `ConnectToDevice` implementations
3. tests that validate the main behavior of the Controller path

This does not mean the repository contains a full standalone Controller product. The delivered scope is the SDK, example code, and Relay support required by external Controller applications.

## Crate

Add the SDK as a dependency from crates.io:

```toml
[dependencies]
controller-sdk = "1.0.0-alpha"
anyhow = "1"
bytes = "1"
tokio = { version = "1", features = ["macros", "rt-multi-thread"] }
tracing-subscriber = { version = "0.3", features = ["env-filter", "fmt"] }
```

Or as a path dependency from a local checkout:

```toml
[dependencies]
controller-sdk = { path = "../gRPC-Relay/crates/controller-sdk" }
```

If the Controller is in a different repository, you can also reference this SDK directly from GitHub:

```toml
[dependencies]
controller-sdk = { git = "https://github.com/cokkiy/gRPC-Relay.git", package = "controller-sdk" }
anyhow = "1"
bytes = "1"
tokio = { version = "1", features = ["macros", "rt-multi-thread"] }
tracing-subscriber = { version = "0.3", features = ["env-filter", "fmt"] }
```

If you need to pin a branch, tag, or revision:

```toml
[dependencies]
controller-sdk = { git = "https://github.com/cokkiy/gRPC-Relay.git", package = "controller-sdk", branch = "master" }
```

```toml
[dependencies]
controller-sdk = { git = "https://github.com/cokkiy/gRPC-Relay.git", package = "controller-sdk", tag = "v0.1.0" }
```

```toml
[dependencies]
controller-sdk = { git = "https://github.com/cokkiy/gRPC-Relay.git", package = "controller-sdk", rev = "<commit-sha>" }
```

Use the GitHub form only when the repository is reachable by the Controller project's build environment. For private repositories, make sure Cargo can authenticate in CI and local development.

## Configuration

The SDK supports direct config construction and environment-based loading through `ControllerSdkConfig::from_env()`.

Environment variables:

```bash
export RELAY_ADDRESS=https://relay.example.com:50051
export CONTROLLER_ID=ctrl-1
export CONTROLLER_TOKEN=<jwt>
export MAX_PAYLOAD_BYTES=10485760
```

Config fields:

```text
relay_address
controller_id
token
max_payload_bytes
```

Notes:

1. `relay_address` may be passed as `relay.example.com:50051` or `https://relay.example.com:50051`
2. if the scheme is omitted, the SDK normalizes it to `https://...`
3. the current SDK uses a static token provider; token refresh can be added later by extending the provider abstraction

## Minimal Controller Code

```rust
use bytes::Bytes;
use controller_sdk::{ControllerClient, ControllerSdkConfig};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt().with_env_filter("info").init();

    let config = ControllerSdkConfig::from_env()?;
    let client = ControllerClient::new(config)?;

    let devices = client.list_online_devices(None).await?;
    let device = devices
        .first()
        .expect("no online device found");

    let session = client.connect_to_device(device.device_id.clone()).await?;

    let response = session
        .send_request(
            "svc.Device/Invoke".to_string(),
            1,
            Bytes::from_static(b"opaque_encrypted_payload"),
            std::time::Duration::from_secs(30),
        )
        .await?;

    println!("response bytes={}", response.len());
    Ok(())
}
```

## SDK API

### `ControllerClient`

Entry point for one-shot API calls and stream-session creation.

- `ControllerClient::new(config)`
- `list_online_devices(region_filter)`
- `connect_to_device(target_device_id)`
- `revoke_token(target_type, target_token_hash_or_prefix, reason)`

### `ControllerConnectSession`

Represents one long-lived `ConnectToDevice` stream bound to:

- one `controller_id`
- one `target_device_id`
- one `method_name` contract per stream binding on the Relay side

Use `send_request(method_name, sequence_number, encrypted_payload, timeout)` to send an opaque payload and await the matching `DeviceResponse`.

## Runtime Behavior

`list_online_devices()`:

1. creates a tonic client connection to Relay
2. injects `controller_id` and `token`
3. sends `ListOnlineDevicesRequest`
4. returns the filtered online device list

`connect_to_device()` plus `send_request()`:

1. opens a `ConnectToDevice` bidi stream to Relay
2. keeps an outbound channel alive for the session lifetime
3. sends `ControllerMessage` with `controller_id`, `token`, `target_device_id`, `method_name`, `sequence_number`, and `encrypted_payload`
4. waits for the matching `DeviceResponse.sequence_number`
5. maps Relay error codes into `ControllerSdkError`

The SDK treats `encrypted_payload` as opaque bytes. End-to-end encryption belongs in Controller and Device business code, not inside Relay or the SDK.

## Error Handling

The SDK maps Relay-side responses into typed errors:

- `Unauthorized`
- `DeviceOffline`
- `DeviceNotFound`
- `RateLimited`
- `PayloadTooLarge`
- `InternalError`
- `StreamClosed`
- `SequenceResponseNotFound`

Recommended caller behavior:

- `DeviceOffline`: wait for MQTT or poll again with `ListOnlineDevices`
- `Unauthorized`: refresh token or re-authenticate
- `RateLimited`: retry with backoff
- `DeviceNotFound`: verify the `device_id`
- `SequenceResponseNotFound`: treat as timeout and decide whether the request is safe to retry

## Run the Example

```bash
export RELAY_ADDRESS=https://127.0.0.1:50051
export CONTROLLER_ID=ctrl-1
export CONTROLLER_TOKEN=<jwt>

cargo run -p controller-sdk --example simple_controller
```

The example requires:

1. a running Relay server
2. at least one connected Device session
3. a valid Controller JWT accepted by Relay

## Verification

The Controller SDK and Relay controller path are covered by tests in this repository:

```bash
cargo test -p controller-sdk
cargo test -p relay grpc_service::tests:: -- --test-threads=1
```
