# 协议与接口定义（Protocol & Interface Spec）

> 版本：v0.1（基于 `doc/requirements.md` / `doc/action_plan.md` / `doc/architecture.md` 与当前仓库 `relay.proto` 的落地现状）
>
> 目标：把协议契约固化下来，避免后续实现偏离；并为代码实现提供明确的 HTTP/gRPC/MQTT 形态约束。

---

## 1. 统一约定（Terminology & Conventions）

### 1.1 标识符
- `device_id`：设备全局唯一标识（字符串）
- `controller_id`：控制端标识（字符串）
- `connection_id`：Relay 为设备活跃连接分配的会话标识符（字符串）
- `sequence_number`：Controller→Device 请求的全局唯一序列号（int64），用于幂等与重放防护

### 1.2 元数据（Relay 可见元数据 vs E2E payload）
- **Relay 可见**：`device_id`、`controller_id`、`method_name`、`sequence_number`、请求发生时间 `timestamp`（本规范规定由 Relay 生成或从控制消息取值）
- **Relay 不可见**：业务 payload（端到端加密的 `encrypted_payload` 字节流），Relay 只转发字节。

### 1.3 JSON 字段类型（MQTT payload）
- 所有 MQTT payload 使用 JSON 编码
- 时间：使用 ISO-8601 字符串（例如：`2025-01-15T10:30:00Z`）
- 字段命名：snake_case 与 requirements/README 保持一致

---

## 2. gRPC 协议契约（Service / Messages / Error Semantics）

### 2.1 Service
```protobuf
service RelayService {
  rpc DeviceConnect(stream DeviceMessage) returns (stream RelayMessage);
  rpc ListOnlineDevices(ListOnlineDevicesRequest) returns (ListOnlineDevicesResponse);
  rpc ConnectToDevice(stream ControllerMessage) returns (stream DeviceResponse);
  rpc RevokeToken(RevokeTokenRequest) returns (RevokeTokenResponse);
}
```

> 注：本仓库当前 `relay.proto` 版本已包含上述服务与消息（见 `crates/relay-proto/proto/relay/v1/relay.proto`）。本节补充“语义/使用规则/错误表达方式”，避免仅依赖字段名造成歧义。

### 2.2 Message / Field 语义

#### 2.2.1 DeviceConnect（Device ⇄ Relay）
- `DeviceMessage`（由 Device 发送，包含顶层身份字段 + `oneof payload`）：
  - 顶层公共字段（来自当前 `relay.proto`）：
    - `device_id`
      - `register` 首条消息必填。
      - `heartbeat` / `data` 消息按当前 proto 继续携带；其值必须与本连接已注册 `device_id` 一致。
      - 该字段是 `DeviceMessage` 的规范化设备标识，Relay 应以该字段作为流级别身份判断依据。
    - `token`
      - `register` 首条消息必填，用于设备鉴权。
      - `heartbeat` / `data` 消息按当前 proto 可继续携带；若实现侧仅在注册阶段校验，也应要求后续消息与已认证会话一致，不得切换设备身份。
  - `RegisterRequest register`
    - 必填：`device_id`
      - 由于当前 proto 同时在 `DeviceMessage.device_id` 与 `RegisterRequest.device_id` 中出现该字段，本规范要求两者在 `register` 消息中必须一致。
      - 若两者不一致，Relay 必须拒绝注册并返回参数错误/鉴权失败，避免不同客户端采用不同优先级策略。
      - 优先级约定：以顶层 `DeviceMessage.device_id` 为准；`RegisterRequest.device_id` 视为冗余镜像字段，用于向后兼容与业务侧显式表达。
    - 可选：`metadata`（map）
    - 可选（用于会话恢复）：`previous_connection_id`
  - `HeartbeatRequest heartbeat`
    - 必填：`connection_id`
    - `timestamp`：Device 端时间戳（Unix epoch 毫秒/秒以系统约定为准；当前实现与后续统一建议：epoch_millis）
    - 语义说明：除 `connection_id` 外，消息顶层仍应携带当前连接对应的 `device_id`（以及 proto 中定义的 `token` 字段，如实现未明确省略）。
  - `DataResponse data`
    - `connection_id`：用于路由到正确会话
    - `sequence_number`：请求幂等匹配键
    - `encrypted_payload`：端到端加密的响应字节
    - `error`：Device 处理结果（Relay 原样转发给 Controller）
    - 语义说明：消息顶层 `device_id` / `token` 如出现，必须与已注册会话保持一致；不得借由后续数据帧变更身份。

- `RelayMessage`（由 Relay 发送，使用 `oneof payload`）：
  - `RegisterResponse register_response`
    - `connection_id`：Relay 分配的新连接标识（即便恢复成功也可产生新 connection_id，是否复用按实现策略）
    - `session_resumed`：是否恢复成功
    - `timestamp`：Relay 侧时间戳（Unix epoch；建议 epoch_millis）
  - `HeartbeatResponse heartbeat_response`
    - `timestamp`：Relay 侧时间戳（Unix epoch；建议 epoch_millis）
  - `DataRequest data_request`
    - `connection_id`
    - `sequence_number`
    - `encrypted_payload`：端到端加密的请求字节

#### 2.2.2 ConnectToDevice（Controller ⇄ Relay ⇄ Device）
- `ControllerMessage`（Controller→Relay，双向流内发送）：
  - 必填：`controller_id`
  - 必填：`token`（认证）
  - 必填：`target_device_id`（授权/路由）
  - 必填：`method_name`（元数据：用于审计/授权/审计索引）
  - 必填：`sequence_number`（幂等与去重）
  - 必填：`encrypted_payload`

- `DeviceResponse`（Relay→Controller）：
  - `device_id`
  - `sequence_number`
  - `encrypted_payload`
  - `error`（统一错误码枚举）

#### 2.2.3 ListOnlineDevices（Controller 查询）
- `ListOnlineDevicesRequest`
  - `controller_id`
  - `token`
  - `region_filter`：可选过滤条件（字符串，允许为空/未设置）
- `ListOnlineDevicesResponse`
  - `devices[]`：每项包含
    - `device_id`
    - `connection_id`
    - `relay_address`
    - `connected_at`（Unix epoch；建议 epoch_millis）
    - `metadata`（map）

> 错误表达：
> - 本 proto 中 `ListOnlineDevicesResponse` 没有 `ErrorCode` 字段。
> - 因此该 RPC 的鉴权/授权/限流/内部错误应使用 **gRPC Status** 表达，而不是写入响应体的 `error_code`。

#### 2.2.4 RevokeToken（Controller 管理操作）
- `RevokeTokenRequest`
  - `controller_id`：发起撤销的 Controller 标识。
  - `admin_token`：管理员 Controller 的 JWT。
  - `target_type`：`CONTROLLER` 或 `DEVICE`。
  - `target_token_hash_or_prefix`：待撤销 token 的完整值或前缀；Relay 按完整匹配或前缀匹配拒绝后续认证。
  - `reason`：审计用撤销原因。
- `RevokeTokenResponse`
  - `revoked`：撤销是否已被 Relay 接受。

> 权限语义：仅 `role=admin` 的 Controller JWT 可以调用该 RPC；非 admin 返回 `PERMISSION_DENIED`。当前 MVP/P1 实现使用单 Relay 进程内存撤销集合，重启后不持久化，多 Relay 一致性留给后续版本。

### 2.3 ErrorCode 与 gRPC Status code 映射（建议契约）
本规范以 `ErrorCode` 枚举为“应用层结果码”，以 gRPC Status 为“传输/鉴权/授权层结果”。

#### 2.3.1 应用层（Controller 流里）使用 ErrorCode
- `OK`：成功
- `DEVICE_OFFLINE`：目标设备离线或断连（Controller 可等待 MQTT 通知或轮询）
- `UNAUTHORIZED`：认证/授权失败（Controller 可重新登录刷新 token）
- `DEVICE_NOT_FOUND`：设备不存在（检查 `device_id`）
- `RATE_LIMITED`：限流
- `INTERNAL_ERROR`：Relay/Device 侧内部错误

在 `ConnectToDevice` 流中，以上错误应写入 `DeviceResponse.error` 并由 Relay 按流语义返回。

#### 2.3.2 传输/控制层（RPC 级）使用 gRPC Status
- `UNAUTHENTICATED`：token 无效/过期/缺失
- `PERMISSION_DENIED`：RBAC 不通过
- `NOT_FOUND`：目标设备不存在（可选）
- `RESOURCE_EXHAUSTED`：限流
- `INTERNAL`：内部错误

> 当前 Relay 已对 `ListOnlineDevices` 使用 gRPC Status 表达认证失败、权限失败和限流；`ConnectToDevice` 流内继续使用 `DeviceResponse.error`。

### 2.4 Controller JWT Claims（当前实现）
Controller 认证使用 HS256 JWT。Relay 校验签名、过期时间，以及可选的 issuer/audience。

JWT claims 至少包含：
```json
{
  "sub": "ctrl-1",
  "controller_id": "ctrl-1",
  "role": "admin | operator | viewer",
  "allowed_project_ids": ["proj-1"],
  "exp": 1893456000,
  "iss": "grpc-relay",
  "aud": "grpc-relay-controller"
}
```

约束：
- `sub` 与 `controller_id` 必须等于请求字段 `controller_id`。
- `admin` 可访问所有设备并可调用 `RevokeToken`。
- `operator` / `viewer` 只能访问 `allowed_project_ids` 覆盖的设备。

---

## 3. MQTT 服务发现契约（Topics / QoS / Payload Schema）

> 统一：本规范采用 requirements/README 的“canonical topics”（与 `doc/action_plan.md` 一致）。
>
> 若在历史上出现过 `relay/{relay_id}/device/online` 等前缀形式，可在后续实现中提供兼容策略，但本规范以“canonical topics”作为最终契约。

### 3.1 设备上线通知（Relay → Controller）
- **Topic**：`relay/device/online`
- **QoS**：1（至少一次）
- **Retain**：建议 `true`（便于 Controller 启动时拉取最近状态；如不希望 retain 可置为 false，但契约需保持一致）
- **Payload(JSON)**：
```json
{
  "device_id": "device-001",
  "connection_id": "conn-12345",
  "relay_address": "relay1.example.com:50051",
  "timestamp": "2025-01-15T10:30:00Z",
  "metadata": {
    "region": "us-west",
    "device_type": "iot-sensor"
  }
}
```

### 3.2 设备离线通知（Relay → Controller）
- **Topic**：`relay/device/offline`
- **QoS**：1
- **Retain**：建议 `false`
- **Payload(JSON)**：
```json
{
  "device_id": "device-001",
  "connection_id": "conn-12345",
  "timestamp": "2025-01-15T11:00:00Z",
  "reason": "timeout" | "graceful_shutdown" | "error"
}
```

### 3.3 设备自报状态（stationService 可选备份验证）
- **Topic**：`device/{device_id}/status`
- **QoS**：1
- **Payload(JSON)**：
```json
{
  "device_id": "device-001",
  "status": "online" | "offline",
  "relay_address": "relay1.example.com:50051",
  "connection_id": "conn-12345",
  "timestamp": "2025-01-15T10:30:00Z"
}
```

### 3.4 设备遥测（Device → Telemetry）
- **Topic**：`telemetry/{device_id}`
- **QoS**：0（最多一次）
- **Payload(JSON)**（示例结构，字段可扩展，但应保持 `device_id` 与 `timestamp`）：
```json
{
  "device_id": "device-001",
  "timestamp": "2025-01-15T10:30:00Z",
  "metrics": {
    "cpu_usage": 45.2,
    "memory_usage": 60.5,
    "network_rx_bytes": 1024000,
    "network_tx_bytes": 512000
  }
}
```

### 3.5 Relay 遥测（Relay → Telemetry）
- **Topic**：`telemetry/relay/{relay_id}`
- **QoS**：0
- **Payload(JSON)**：结构较大，本规范要求至少包含以下根字段：
  - `relay_id`
  - `relay_address`
  - `timestamp`
  - 以及对象字段：`system_metrics`、`connection_metrics`、`stream_metrics`、`performance_metrics`、`error_metrics`、`queue_metrics`、`mqtt_metrics`、`health_status`

> 如后续文档与实现需要精确字段列表，可在 v0.2 补齐到“逐字段契约”。

---

## 4. HTTP 健康检查接口契约（/health）

### 4.1 Endpoint
- **Method**：GET
- **Path**：由配置 `observability.health.path` 决定（默认：`/health`）
- **Port**：由配置 `observability.health.address` 决定（默认：`0.0.0.0:8080`）
- **Response**：200 OK（本 MVP 骨架中不做组件实际探测时，仍返回 200，并在 body 中标注状态）

### 4.2 Response JSON Shape
与 `doc/requirements.md` 保持一致：

```json
{
  "status": "healthy" | "degraded" | "unhealthy",
  "timestamp": "2025-01-15T10:30:00Z",
  "uptime_seconds": 86400,
  "version": "1.0.0",
  "components": {
    "grpc_server": {
      "status": "healthy" | "degraded" | "unhealthy",
      "message": "Listening on :50051"
    },
    "quic_listener": {
      "status": "healthy" | "degraded" | "unhealthy",
      "message": "Listening on :50052"
    },
    "mqtt_client": {
      "status": "healthy" | "degraded" | "unhealthy",
      "message": "Connected to broker1.example.com:8883"
    },
    "auth_service": {
      "status": "healthy" | "degraded" | "unhealthy",
      "message": "Token validation working"
    },
    "metrics_collector": {
      "status": "healthy" | "degraded" | "unhealthy",
      "message": "Exporting to Prometheus"
    }
  },
  "metrics": {
    "active_device_connections": 8500,
    "active_controller_connections": 150,
    "active_streams": 1024,
    "cpu_usage_percent": 45.2,
    "memory_usage_percent": 60.5
  }
}
```

### 4.3 状态判定（语义）
- `healthy`：所有组件正常
- `degraded`：部分非关键组件异常（例如 MQTT 断连但 gRPC 正常）
- `unhealthy`：关键组件异常（例如 gRPC 服务器无法启动）

> MVP 现阶段仅实现了 health server 本体时，为了契约可用性：
> - `status` 必须仍然使用本规范定义的枚举值：`healthy` / `degraded` / `unhealthy`。
> - “not implemented / unavailable” 之类的说明应写入顶层或各组件的 `message` 字段，而不是写入 `status`。
> - 具体应返回哪个 `status`，由各组件当前可用性与影响范围决定，但字段结构必须满足本规范。

---

## 5. 指标接口契约（/metrics/security）

### 5.1 状态
当前 MVP/P1 已落地安全指标 JSON endpoint：`GET /metrics/security`。

### 5.2 约定
- 该 endpoint 与 `/health` 使用同一个 HTTP 服务端口，默认 `0.0.0.0:8080`。
- 该 endpoint 返回 JSON，不是 Prometheus 文本格式。
- 完整 Prometheus `/metrics` 仍是后续版本范围。

### 5.3 Response JSON Shape
```json
{
  "auth_success_total": 10,
  "auth_failure_total": 2,
  "authorization_denied_total": 1,
  "rate_limit_total": 3,
  "revoked_tokens_total": 1,
  "auth_failure_ratio": 0.16666666666666666,
  "authorization_denied_ratio": 0.08333333333333333,
  "rate_limit_ratio": 0.25
}
```

---
