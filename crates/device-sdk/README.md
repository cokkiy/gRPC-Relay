# device-sdk（Device 侧 stationService 接入 SDK）

device-sdk 为设备侧 stationService 提供与 Relay 的 **DeviceConnect** 长连接能力：
- 注册 Register
- 周期心跳 Heartbeat
- 断线重连（指数退避）
- session recovery（携带 `previous_connection_id`，窗口内才会携带）
- Controller 下发 opaque payload 的回调处理
- stationService 配置文件与环境变量加载

> stationService 是独立应用，不属于本仓库。本 crate 是该应用接入 Relay 的 SDK 边界。
>
> 当前 Relay server 仍是骨架，尚未启动 `RelayService::DeviceConnect` 服务；因此 SDK 可编译、可集成，但完整设备侧接入链路还不能在本仓库内端到端跑通。

---

## 快速开始（最小示例）

示例文件：
- `crates/device-sdk/examples/station_service_minimal.rs`

运行方式（环境变量）：
```bash
export RELAY_TCP_ADDR="127.0.0.1:50051"
export DEVICE_ID="device-001"
export DEVICE_TOKEN="device-token"
export DEVICE_METADATA_REGION="us-west"
export DEVICE_METADATA_DEVICE_TYPE="iot-sensor"

cargo run -p device-sdk --example station_service_minimal
```

运行方式（配置文件）：
```bash
export STATION_SERVICE_CONFIG="crates/device-sdk/examples/station_service.yaml"
cargo run -p device-sdk --example station_service_minimal
```

---

## 使用方式

### 1) 实现回调：DeviceDataHandler

```rust
use device_sdk::handler::{DeviceDataHandler, DataRequestContext};

struct MyHandler;

#[async_trait::async_trait]
impl DeviceDataHandler for MyHandler {
    async fn on_data_request(
        &self,
        _ctx: DataRequestContext,
        encrypted_payload: device_sdk::handler::EncryptedPayload,
    ) -> anyhow::Result<device_sdk::handler::EncryptedPayload> {
        // MVP：不解密/不理解语义，只返回 opaque bytes
        Ok(encrypted_payload)
    }
}
```

### 2) 加载配置 DeviceSdkConfig

配置结构见：
- `crates/device-sdk/src/config.rs`
- `crates/device-sdk/examples/station_service.yaml`

示例里默认使用：
- `session_recovery_window_seconds: 300`
- `heartbeat_interval_seconds: 30`
- 重连 backoff：`1s -> 60s`

---

## 重要语义约定（与 relay.proto 对齐）

- `RegisterRequest.previous_connection_id`：仅在 recovery 窗口内才会携带
- `HeartbeatRequest.connection_id`：使用 relay 返回的连接标识
- `DataRequest.encrypted_payload` / `DataResponse.encrypted_payload`：opaque 透明转发

---

## 详细文档

完整 stationService 集成说明见：
- `doc/device_sdk.md`

---

## 依赖约束

- device-sdk 与 `relay-proto` 使用相同的 tonic/prost 版本（避免 gRPC 类型不匹配问题）。
