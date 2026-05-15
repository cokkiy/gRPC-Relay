下面是基于 `doc/requirements.md` 和 `doc/architecture.md` 拆解出的实施行动计划，按 MVP 优先级和依赖关系组织。

## 一、总体策略

目标不是“一次做完全部能力”，而是先交付 **可运行、可验证、可扩展的单节点 MVP**，再逐步补齐高级能力。

### MVP 核心原则
1. **先打通主链路**：Device ↔ Relay ↔ Controller
2. **先保证安全边界**：认证、授权、TLS、端到端加密
3. **先保证可运维**：健康检查、指标、日志、审计
4. **先保证可验证**：单元测试、集成测试、压力测试基线
5. **明确后置项**：多节点、会话持久化、Controller QUIC、0-RTT

---

## 二、工作分解结构

### 1) 基础工程与项目骨架
**状态**：完成

**目标**：搭建 Rust 项目结构、模块边界、配置体系、构建/测试/部署基础。

**交付物**
- Rust workspace / crate 结构
- `proto` 定义与生成流程
- 配置加载与环境变量支持
- 日志、错误处理、依赖注入的基础骨架
- Dockerfile / docker-compose / K8s 基础模板

**依赖**
- 无，第一优先级

---

### 2) 协议与接口定义
**状态**：基本完成（`/metrics` 接口规范暂缓）

**目标**：把需求中的协议契约固化下来，避免后续实现偏离。

**交付物**
- gRPC proto 文件（已完成）
- MQTT topic 与 payload schema 文档（已完成）
- 错误码、状态码、元数据规范（已完成）
- Health / metrics HTTP 接口规范（Health 已完成，`/metrics` endpoint 当前 MVP 暂不落地）

**备注**
- 当前契约文档见 `doc/protocol_spec.md`，proto 定义见 `crates/relay-proto/proto/relay/v1/relay.proto`。
- `/metrics` 仅保留后续实现基线；如未来落地 endpoint，需要同步更新协议文档、代码和验证用例。

**依赖**
- 基础工程与项目骨架

---

### 3) 设备侧接入链路
**状态**：基本完成（QUIC transport 推迟至 v2）

**目标**：实现 stationService 与 Relay 的长连接、注册、心跳、断线重连、会话恢复。

**交付物**
- DeviceConnect 流程：SDK client 已实现；Relay server 已实现 ✅
- Register / Heartbeat / Disconnect 逻辑：SDK 已实现；Relay 侧已实现 ✅
- QUIC 连接与 TCP fallback：推迟至 v2（当前使用 tonic HTTP/2 TCP）⏳
- 心跳超时、离线判定、重连退避：SDK 已实现重连退避；Relay 心跳超时/离线判定已实现 ✅
- session / connection_id 管理：SDK 支持 recovery 窗口内携带 `previous_connection_id`；Relay session registry 已实现，但断线后恢复窗口仍未实现 ⏳
- stationService SDK：见 `crates/device-sdk` 与 `doc/device_sdk.md`

**实现细节**
- Relay 侧 `DeviceConnect` 双向流：`grpc_service.rs` — `run_device_connect_stream()` 处理 Register/Heartbeat/Data
- 心跳超时检测：通过 `tokio::time::timeout` 包装 `inbound.next()`，默认 120s 无消息即判定超时，断开连接并发布 MQTT 离线通知（reason=timeout）
- `RelayState` 新增 `device_last_seen: DashMap<String, Instant>` 及 `touch_device()` 方法追踪设备最后活跃时间
- `RelayConfig` 新增 `heartbeat_timeout_seconds` 配置项（默认 120s）
- Session 管理：`RelayState` 维护 `sessions_by_device_id` / `connection_to_device_id` 双向映射，`SessionRegistry` 提供查询门面

**验收备注**
- stationService 是外部应用，不包含在本仓库内；本仓库交付其接入 SDK、示例代码和文档。
- 当前可验证项：`cargo test -p relay`（121 tests），含 `device_heartbeat_timeout_disconnects_and_publishes_offline`
- QUIC transport 推迟至 v2：`crates/relay/src/transport.rs` 为占位 stub，仅启动 tonic HTTP/2 server

**依赖**
- 协议与接口定义
- 基础工程与项目骨架

---

### 4) Controller 接入链路（外部程序 + Controller SDK）
**状态**：完成（Controller SDK、示例代码、Relay 侧 `ListOnlineDevices` / `ConnectToDevice` 均已落地）

**目标**：打通 Controller（外部程序）到 Relay 的认证、在线设备查询、会话建立，并交付一个可供外部 Controller 程序直接集成的客户端 SDK。

> 说明：Controller 是外部程序，不在本仓库内；当前仓库交付其接入 SDK、示例代码、配套文档，以及 Relay 侧 `RelayService::ListOnlineDevices` / `RelayService::ConnectToDevice` 的实现与测试。验收范围是“外部 Controller 可基于本仓库提供的 SDK 和 Relay 能力完成接入”，而不是在本仓库内再实现一个完整独立的 Controller 产品。

**交付物**
- Controller JWT 鉴权参数注入（SDK 负责携带 `controller_id`、`token`）
- `ListOnlineDevices`：查询在线设备列表（可选 `region_filter`）
- `ConnectToDevice`：维护 `ConnectToDevice` 双向流会话，并按 `sequence_number` 匹配 `DeviceResponse`
- Controller 侧错误处理建议：
  - `DEVICE_OFFLINE`：等待上线/轮询补偿重试
  - `UNAUTHORIZED`：刷新 token / 重新登录
  - `RATE_LIMITED`：指数退避重试
  - `DEVICE_NOT_FOUND`：检查 device_id
- SDK crate 交付：`crates/controller-sdk`
  - 提供 `ControllerClient`、`ControllerConnectSession`
  - 提供 `examples/simple_controller.rs` 示例（外部程序集成参考）
  - 提供文档：`doc/controller_sdk.md`

**依赖**
- 协议与接口定义（`crates/relay-proto/proto/relay/v1/relay.proto` / `doc/protocol_spec.md`）
- 认证与授权基础能力（已由 Relay 服务端完成校验；SDK 负责参数注入与契约对齐）


---

### 5) 流中继与幂等性
**目标**：实现双向流转发、元数据提取、幂等缓存、基础限流。

**交付物**
- Stream Router
- Controller ↔ Device 双向流映射
- sequence_number 去重缓存（LRU 10K）
- 请求重放处理
- payload 大小限制与输入校验
- 速率限制策略

**依赖**
- 设备侧接入
- Controller 接入
- 安全基础能力

---

### 6) 认证、授权与安全（实施行动计划）
**状态**：完成（MVP 6.1 + P1 安全增强已落地；多 Relay 撤销一致性仍属 6.3 后续项）

**目标**：建立系统安全边界（可控中继模式），确保 Relay 能完成认证/授权/审计/限流与 anti-replay，但**不解密**业务 payload。

---

#### 6.1 MVP 必达安全能力（Week 6 验收主线）

1. **传输层加密（TLS 1.3）**
- Device ↔ Relay：QUIC 内置 TLS 1.3
- Controller ↔ Relay：HTTP/2 + TLS 1.3

2. **端到端 payload 透明转发**
- Relay 仅转发 `encrypted_payload` 字节：不解密、不重加密、不做内容级审计
- 授权/审计/幂等只依赖元数据（`device_id/controller_id/method_name/sequence_number/timestamp` 等）

3. **认证（Authentication）**
- Device 侧：mTLS / Token（二选一或兼容并行，MVP 至少实现一种）
- Controller 侧：JWT
- 认证失败返回语义：
  - `ListOnlineDevices`：使用 gRPC Status（`UNAUTHENTICATED`）
  - `ConnectToDevice`：流内返回 `DeviceResponse.error = UNAUTHORIZED`

4. **授权（Authorization：RBAC + 设备归属）**
- RBAC 角色：`admin / operator / viewer`
- 设备归属：`device_id -> project_id/tenant_id`
- 方法白名单：`method_name` 不在允许列表拒绝
- 授权检查时机（MVP 建议）：在“连接/流映射创建时检查一次”，避免每条消息重复开销
- 授权拒绝返回语义：
  - `ListOnlineDevices`：`PERMISSION_DENIED`
  - `ConnectToDevice`：`DeviceResponse.error = UNAUTHORIZED`
  - 审计事件：`authorization_denied`

5. **输入验证与安全约束**
- 必填性校验：ControllerMessage 的 `controller_id/token/target_device_id/method_name/sequence_number/encrypted_payload`
- 身份字段格式校验：`device_id/controller_id` 长度与字符集等
- payload 大小限制：`encrypted_payload < 10MB`
- 透明转发安全回归点：Relay 代码不得读取/解密 payload 内容

6. **anti-replay / 幂等（sequence_number）**
- 使用 `sequence_number` 做幂等：LRU/TTL（最近 10K，过期 1 小时）
- 重复请求：返回缓存响应，不重复转发到 Device
- 安全意义：基础重放防护 + 传输重试幂等

7. **限流（基础 DDoS 防护，应用层）**
    - 限流位置：认证/授权通过之后、转发之前
    - **请求速率（token bucket）**：
      - per-device：1,000 req/s
      - per-controller：1,000 req/min
      - global：100,000 req/s
    - **连接速率（sliding window）**：
      - per-device：10 conn/min
      - global：100 conn/s
    - **并发流限制**：
      - per-device：10 concurrent streams
      - per-controller：100 concurrent streams
    - **带宽限制（rotating 1s window）**：
      - per-device：10 MB/s
      - per-controller：100 MB/s
      - global：100 MB/s（~800 Mbps）
    - **资源阈值**：
      - CPU: 80%（超出拒绝新连接）
      - Memory: 12 GB（超出拒绝新连接）
    - 返回语义：
      - `ConnectToDevice`：`DeviceResponse.error = RATE_LIMITED`
      - `ListOnlineDevices`：gRPC Status `RESOURCE_EXHAUSTED`
      - `DeviceConnect`：gRPC Status `RESOURCE_EXHAUSTED`
    - 审计事件：`rate_limit`（token 脱敏）

8. **审计日志与 tracing（可追溯、可脱敏）**
- MVP 审计事件落地（至少）：
  - `auth_failure / auth_success`
  - `authorization_denied`
  - `controller_request`
  - `rate_limit`
- 脱敏规则：
  - 不记录 `encrypted_payload` 明文
  - token 只记录摘要/前 8 位
- tracing 主链路（至少一个关键链路）：
  - `controller_request_to_device`：auth_verify / permission_check / relay_forward / relay_response

---

#### 6.2 P1（Week 9）安全增强（首版必须补齐）
- token revocation 生效增强：撤销后可在可接受延迟内拒绝
- 安全观测性增强：
  - 认证失败率、授权拒绝率、限流命中率可检索/可度量
- 安全测试用例覆盖：
  - 未授权访问 / 伪造 token / 跨设备访问 / 重放（sequence_number）/ 超大 payload / 限流触发

**落地说明**
- Controller 认证使用 HS256 JWT；Device 认证使用 token。
- TLS 通过 `relay.tls` PEM 文件路径配置接入 tonic gRPC server。
- token revocation 通过 `RevokeToken` gRPC RPC 下发，仅 admin JWT 可调用；当前为单 Relay 内存生效。
- 安全指标通过 `/metrics/security` JSON endpoint 暴露。

---

#### 6.3 P2（后续版本）可选增强
- 更细粒度 ABAC（如需要）
- 更强 DDoS/连接级防护策略（应用层 + 边缘配合）
- 多 Relay 下的权限/撤销一致性改进

---

### 7) MQTT 服务发现与遥测（Discovery & Telemetry）

**目标**：通过 MQTT 实现设备在线/离线可见性与遥测上报；并在 MQTT 不可用时通过 gRPC 查询实现可恢复的服务发现链路；同时发布 Relay 自身遥测数据用于运维与容量观测。

---

#### 7.1 MQTT 主题契约（以 `doc/protocol_spec.md` 为准，canonical topics）

- **设备上线通知（Relay → Controller）**
  - Topic：`relay/device/online`
  - QoS：1（至少一次）
  - Retain：建议 `true`
  - Payload（JSON）：
    - `device_id` / `connection_id` / `relay_address` / `timestamp` / `metadata{...}`

- **设备离线通知（Relay → Controller）**
  - Topic：`relay/device/offline`
  - QoS：1
  - Retain：建议 `false`
  - Payload（JSON）：
    - `device_id` / `connection_id` / `timestamp` / `reason`（`timeout | graceful_shutdown | error`）

- **设备自报状态（stationService 可选备份验证）**
  - Topic：`device/{device_id}/status`
  - QoS：1
  - Payload（JSON）：
    - `device_id` / `status`（`online|offline`）/ `relay_address` / `connection_id` / `timestamp`

- **设备遥测（Device → Telemetry）**
  - Topic：`telemetry/{device_id}`
  - QoS：0（最多一次，允许丢失）
  - Payload（JSON）：
    - `device_id` / `timestamp` / `metrics{...}`（至少包含 cpu/memory/network 等基础字段）

- **Relay 遥测（Relay → Telemetry）**
  - Topic：`telemetry/relay/{relay_id}`
  - QoS：0（最多一次）
  - Payload（JSON）：至少包含
    - `relay_id` / `relay_address` / `timestamp`
    - `system_metrics`、`connection_metrics`、`stream_metrics`、`performance_metrics`、`error_metrics`、`queue_metrics`、`mqtt_metrics`、`health_status`

---

#### 7.2 MVP 职责边界（实现落点）

- **Relay（本仓库内实现）**
  1. 设备连接成功后发布：`relay/device/online`
  2. 设备断连清理后发布：`relay/device/offline`（带 `reason`）
  3. 固定周期发布遥测：`telemetry/relay/{relay_id}`
  4. MQTT 断连时按“降级策略”运行（不影响主链路转发）

- **Controller（若项目包含消费逻辑；否则作为对外契约）**
  - 订阅 `relay/device/online` 与 `relay/device/offline`
  - 收到通知后更新在线集合（保存 `device_id/connection_id/relay_address/timestamp`）
  - MQTT 丢失/不可用时，通过 gRPC `ListOnlineDevices()` 做全量补偿/修复

---

#### 7.3 “三路兜底”发现一致性策略（可验证）

1. **主路径**：MQTT 通知实时更新在线集合  
2. **备份验证**：若设备侧 `device/{device_id}/status` 可用，则对比并记录冲突告警（不阻断主路径）  
3. **最终补偿**：当 MQTT 不可用或订阅恢复后，Controller 触发 `ListOnlineDevices()` 拉取全量在线集合，并以 gRPC 返回为准合并修复

---

#### 7.4 MQTT 客户端重连与降级策略（必须落地）

- **重连**
  - 断线后指数退避重连，避免重连风暴
  - 重连成功后触发一次“全量状态同步/补偿”（不依赖 retained 为唯一真相）

- **MQTT 不可用降级（Relay → Controller）**
  - 降低/暂停遥测与事件发布频率
  - Controller 切换为轮询：每 30 秒调用 `ListOnlineDevices()`，保证“在线集合可用”

- **MQTT 恢复**
  - Controller 可逐步恢复订阅模式；或继续混合（MQTT + gRPC）以提高一致性

---

#### 7.5 验收标准（面向可验证输出）

- **在线/离线事件**
  - 设备完成注册并建立长连接后：Controller（或测试订阅端）必须在窗口内收到 `relay/device/online`
  - 设备断连清理后：必须收到 `relay/device/offline`，且 `reason` 与实现约定一致
- **遥测发布**
  - `telemetry/relay/{relay_id}` 周期发布正常；断网/重连不导致进程崩溃或资源泄漏
- **补偿机制**
  - 人为模拟 MQTT 断连/恢复后：Controller 通过 `ListOnlineDevices()` 能恢复正确在线集合

---

#### 7.6 集成测试建议（最小集）

1. **MQTT 在线通知测试**
   - 订阅 `relay/device/online`
   - 启动设备并注册长连接
   - 断言 payload 字段完整且与会话一致

2. **MQTT 离线通知测试**
   - 断开设备连接（graceful shutdown 或 timeout）
   - 订阅 `relay/device/offline`
   - 断言 `reason` 与 `connection_id` 行为符合实现约定

3. **MQTT 断连降级测试**
   - 暂停 MQTT Broker / 断网
   - 验证 Controller 轮询 `ListOnlineDevices()` 能维持在线集合正确
   - MQTT 恢复后可继续恢复订阅/混合
  - Topic：`device/{device_id}/status`
  - QoS：1
  - Payload（JSON）：
    - `device_id` / `status`（`online|offline`）/ `relay_address` / `connection_id` / `timestamp`

- **设备遥测（Device → Telemetry）**
  - Topic：`telemetry/{device_id}`
  - QoS：0（最多一次，允许丢失）
  - Payload（JSON）：
    - `device_id` / `timestamp` / `metrics{...}`（至少包含 cpu/memory/network 等基础字段）

- **Relay 遥测（Relay → Telemetry）**
  - Topic：`telemetry/relay/{relay_id}`
  - QoS：0（最多一次）
  - Payload（JSON）：至少包含
    - `relay_id` / `relay_address` / `timestamp`
    - `system_metrics`、`connection_metrics`、`stream_metrics`、`performance_metrics`、`error_metrics`、`queue_metrics`、`mqtt_metrics`、`health_status`

---

#### 7.2 MVP 职责边界（实现落点）

- **Relay（本仓库内实现）**
  1. 设备连接成功后发布：`relay/device/online`
  2. 设备断连清理后发布：`relay/device/offline`（带 `reason`）
  3. 固定周期发布遥测：`telemetry/relay/{relay_id}`
  4. MQTT 断连时按“降级策略”运行（不影响主链路转发）

- **Controller（若项目包含消费逻辑；否则作为对外契约）**
  - 订阅 `relay/device/online` 与 `relay/device/offline`
  - 收到通知后更新在线集合（保存 `device_id/connection_id/relay_address/timestamp`）
  - MQTT 丢失/不可用时，通过 gRPC `ListOnlineDevices()` 做全量补偿/修复

---

#### 7.3 “三路兜底”发现一致性策略（可验证）

1. **主路径**：MQTT 通知实时更新在线集合  
2. **备份验证**：若设备侧 `device/{device_id}/status` 可用，则对比并记录冲突告警（不阻断主路径）  
3. **最终补偿**：当 MQTT 不可用或订阅恢复后，Controller 触发 `ListOnlineDevices()` 拉取全量在线集合，并以 gRPC 返回为准合并修复

---

#### 7.4 MQTT 客户端重连与降级策略（必须落地）

- **重连**
  - 断线后指数退避重连，避免重连风暴
  - 重连成功后触发一次“全量状态同步/补偿”（不依赖 retained 为唯一真相）

- **MQTT 不可用降级（Relay → Controller）**
  - 降低/暂停遥测与事件发布频率
  - Controller 切换为轮询：每 30 秒调用 `ListOnlineDevices()`，保证“在线集合可用”

- **MQTT 恢复**
  - Controller 可逐步恢复订阅模式；或继续混合（MQTT + gRPC）以提高一致性

---

#### 7.5 验收标准（面向可验证输出）

- **在线/离线事件**
  - 设备完成注册并建立长连接后：Controller（或测试订阅端）必须在窗口内收到 `relay/device/online`
  - 设备断连清理后：必须收到 `relay/device/offline`，且 `reason` 与实现约定一致
- **遥测发布**
  - `telemetry/relay/{relay_id}` 周期发布正常；断网/重连不导致进程崩溃或资源泄漏
- **补偿机制**
  - 人为模拟 MQTT 断连/恢复后：Controller 通过 `ListOnlineDevices()` 能恢复正确在线集合

---

#### 7.6 集成测试建议（最小集）

1. **MQTT 在线通知测试**
   - 订阅 `relay/device/online`
   - 启动设备并注册长连接
   - 断言 payload 字段完整且与会话一致

2. **MQTT 离线通知测试**
   - 断开设备连接（graceful shutdown 或 timeout）
   - 订阅 `relay/device/offline`
   - 断言 `reason` 与 `connection_id` 行为符合实现约定

3. **MQTT 断连降级测试**
   - 暂停 MQTT Broker / 断网
   - 验证 Controller 轮询 `ListOnlineDevices()` 能维持在线集合正确
   - MQTT 恢复后可继续恢复订阅/混合
</REPLACE>

---

### 8) 观测性与运维接口
**目标**：补齐生产可用性基础。

**当前状态总览**：
- ✅ `/health` — 已实现（含真实 device/stream/controller 计数、资源阈值、MQTT 状态判定）
- ✅ `/health/live` `/health/ready` `/health/startup` — 已实现（与架构文档运维接口对齐）
- ✅ `/metrics/security` — 已实现（安全计数与比率快照）
- ✅ Prometheus `/metrics` endpoint — 已实现（核心 auth / request / stream / resource / MQTT / health 指标）
- ✅ 结构化日志 — 已实现（`tracing-subscriber`，支持 JSON/text + EnvFilter）
- ✅ OpenTelemetry 分布式追踪 — 已实现（可配置 OTLP 导出、采样率、service.name）
- ✅ 资源监控 — 已实现（CPU / 内存阈值检查，供 health / telemetry / alerting 复用）
- ✅ Relay MQTT 遥测 — 已实现（定期发布 relay telemetry）
- ✅ MQTT runtime 状态追踪 — 已实现（连接状态 / 重连次数 / 丢弃数 / 队列深度）
- ✅ 审计日志 — 已实现（JSONL、异步写入、事件过滤、轮转、脱敏、核心链路埋点）
- ✅ 告警阈值配置与通知入口 — 已实现（规则评估、抑制窗口、结构化告警输出；外部渠道预留）

---

#### 8.1 审计日志（Audit Logs）

**目标**：实现需求 7.4 节定义的审计日志体系——结构化 JSON 输出、事件类型齐全、脱敏规则到位、不记录加密 payload。

##### 8.1.1 审计事件类型（需求节选）

需要落地的 15 种事件类型（按需求 7.4 审计日志事件类型清单）：

| 事件类型                | 触发条件                    | 优先级 |
| ----------------------- | --------------------------- | ------ |
| `device_connect`        | 设备成功建立连接            | P0     |
| `device_disconnect`     | 设备主动断开或超时          | P0     |
| `device_register`       | 设备首次注册或重新注册      | P0     |
| `controller_connect`    | Controller 成功建立连接     | P0     |
| `controller_disconnect` | Controller 主动断开         | P1     |
| `controller_request`    | Controller 发起设备访问请求 | P0     |
| `stream_created`        | 新的双向流建立              | P0     |
| `stream_closed`         | 流正常或异常关闭            | P0     |
| `auth_failure`          | Token 验证失败或证书无效    | P0     |
| `auth_success`          | 认证通过                    | P1     |
| `authorization_denied`  | 权限检查失败                | P0     |
| `rate_limit`            | 请求频率超过限制            | P0     |
| `session_resumed`       | 设备重连后成功恢复会话      | P1     |
| `session_expired`       | 会话超时被清理              | P1     |
| `error`                 | Relay 内部异常              | P0     |

##### 8.1.2 审计日志结构（需求定义）

```json
{
  "timestamp": "2025-01-15T10:30:00.123Z",
  "event_type": "device_connect",
  "relay_id": "relay-001",
  "device_id": "device-001",
  "controller_id": "controller-123",
  "connection_id": "conn-12345",
  "method_name": "ExecuteCommand",
  "sequence_number": 98765,
  "result": "success",
  "error_code": "OK",
  "latency_ms": 15.6,
  "bytes_transferred": 10240,
  "source_ip": "192.168.1.100",
  "user_agent": "controller-client/1.0",
  "metadata": {
    "region": "us-west",
    "device_type": "iot-sensor",
    "project_id": "proj-456"
  }
}
```

**脱敏规则**：
- 不记录 `encrypted_payload` 明文
- Token 只记录前 8 位（如 `abcd1234...`）
- 不记录私钥或敏感个人信息

##### 8.1.3 实现方案

**模块位置**：新建 `crates/relay/src/audit.rs`

**核心类型**：

```rust
// 审计事件枚举 — 每种事件携带其专属字段
enum AuditEvent {
    DeviceConnect { device_id, connection_id, source_ip, metadata },
    DeviceDisconnect { device_id, connection_id, reason, source_ip },
    DeviceRegister { device_id, connection_id, previous_connection_id, session_resumed },
    ControllerConnect { controller_id, source_ip, user_agent },
    ControllerDisconnect { controller_id },
    ControllerRequest { controller_id, device_id, method_name, sequence_number, payload_size, latency_ms, result, error_code },
    StreamCreated { stream_id, device_id, controller_id, method_name },
    StreamClosed { stream_id, reason, bytes_transferred },
    AuthFailure { entity_type, entity_id, reason, source_ip },
    AuthSuccess { entity_type, entity_id },
    AuthorizationDenied { controller_id, device_id, method_name },
    RateLimit { entity_type, entity_id, limit_kind },
    SessionResumed { device_id, old_connection_id, new_connection_id },
    SessionExpired { device_id, connection_id },
    Error { message, error_code, context },
}
```

**输出策略**：
- **Writer 抽象**：trait `AuditWriter`（支持文件写入 / stdout / 后续扩展 Kafka/ES）
- **默认实现**：`FileAuditWriter` — 行分隔 JSON（JSONL），每行一个事件
- **异步写入**：通过 `tokio::sync::mpsc` 通道解耦，避免阻塞热路径
- **文件轮转**：基于大小的轮转（默认 100 MB → `audit.log.1`, `audit.log.2` 等，最多 10 个）
  - 若需要压缩则 gzip 历史文件

**配置新增（`ObservabilityConfig` 下）**：

```yaml
observability:
  audit:
    enabled: true
    output: file              # file | stdout
    file_path: /var/log/relay/audit.log
    max_size_mb: 100          # 单个文件最大 100 MB
    max_backups: 10            # 最多保留 10 个历史文件
    retention_days: 30         # 保留 30 天
    # 可选的事件过滤（只记录指定类型）
    events:
      - device_connect
      - device_disconnect
      - controller_request
      - auth_failure
      - rate_limit
```

**依赖注入**：`AuditLogger` 作为共享状态（`Arc<AuditLogger>`）注入到：
- `DeviceConnect` 流处理（设备注册/心跳超时/断连时发出事件）
- `ConnectToDevice` 流处理（流创建/关闭/转发时发出事件）
- Auth 模块（认证成功/失败时发出事件）
- RBAC 模块（授权拒绝时发出事件）
- Rate limiter（限流触发时发出事件）
- Session manager（会话恢复/过期时发出事件）

**验收标准**：
- [ ] 15 种事件类型至少在集成测试中覆盖 10 种核心事件
- [ ] 输出格式为合法 JSONL，每行可独立解析
- [ ] 脱敏验证：日志中不出现完整 Token（仅前 8 位）、不出现 payload 内容
- [ ] 异步写入不影响请求延迟（P99 < 20ms 保持）
- [ ] 文件轮转：超过 100 MB 自动轮转，历史文件不超过 10 个

---

#### 8.2 Prometheus `/metrics` Endpoint（后续迭代暂缓，此处仅定义接口契约）

**状态**：已实现核心 `/metrics` endpoint。需求文档中的“MVP 暂缓”说明已过期，需要在需求文档中另行同步。

**契约要点**（来自需求 7.4 节完整指标清单）：
- Connection metrics: `relay_active_connections`, `relay_connection_duration_seconds`, `relay_connection_rate`, `relay_connections_by_region`
- Stream metrics: `relay_active_streams`, `relay_stream_duration_seconds`, `relay_stream_errors_total`
- Latency metrics: `relay_request_latency_seconds` (histogram), `relay_queue_wait_time_seconds`
- Throughput metrics: `relay_bytes_transferred_total`, `relay_requests_total`
- Error metrics: `relay_errors_total`, `relay_auth_failures_total`, `relay_rate_limit_hits_total`
- Resource metrics: `relay_cpu_usage_percent`, `relay_memory_used_bytes`, `relay_open_file_descriptors`, `relay_goroutines`
- Queue metrics: `relay_pending_messages`, `relay_queue_overflow_total`
- MQTT metrics: `relay_mqtt_connected`, `relay_mqtt_publish_rate`, `relay_mqtt_reconnect_total`
- Health: `relay_health_status`, `relay_component_health`

**后续落地时需要的步骤**：
1. 引入 `prometheus` crate（或 `opentelemetry-prometheus`）
2. 注册 Registry + 各指标（Counter / Gauge / Histogram）
3. 在 `serve_health` 的 axum Router 上挂载 `/metrics` route（`axum::routing::get(metrics_handler)`）
4. Metrics handler 调用 `prometheus::TextEncoder` 输出 OpenMetrics 文本格式
5. 在 Relay 各模块埋点（connection open/close、stream create/destroy、rate limit hit 等）
6. 更新 `doc/protocol_spec.md` 与 action_plan 完成状态

---

#### 8.3 OpenTelemetry 分布式追踪（后续迭代，此处仅定义接入点）

**状态**：已实现可配置 OTLP tracing layer，并在关键请求链路添加 span/属性。

**Trace 结构回顾**（需求定义）：
```
Trace: controller_request_to_device
├─ Span: controller_connect (Controller → Relay)
│  ├─ Span: auth_verify
│  └─ Span: permission_check
├─ Span: relay_route
├─ Span: relay_forward (Relay → Device)
│  ├─ Span: queue_wait
│  └─ Span: network_send
├─ Span: device_process
└─ Span: relay_response
```

**后续落地步骤**：
1. 引入 `opentelemetry` + `opentelemetry-otlp` + `tracing-opentelemetry` crates
2. 在 `logging::init` 中叠加 OpenTelemetry layer（采样率默认 0.1）
3. 在 gRPC 请求入口、auth 校验、stream forward、MQTT publish 等关键路径添加 `#[tracing::instrument]` 宏
4. Span 属性注入：`relay.id`, `device.id`, `controller.id`, `connection.id`, `method.name`, `sequence.number`
5. 通过 OTLP exporter 导出到 Jaeger / Tempo
6. 配置项新增（`ObservabilityConfig` 下）：`tracing.enabled`, `tracing.sampling_rate`, `tracing.exporter`, `tracing.otlp_endpoint`

---

#### 8.4 告警阈值与通知（后续迭代，此处定义配置结构）

**状态**：已实现本地规则评估、抑制与结构化告警输出；Slack/SMTP/PagerDuty 仍为后续可选扩展。

**告警规则回顾**（需求 7.4 节 14 条告警规则）：

| 指标       | Warning 阈值 | Critical 阈值 |
| ---------- | ------------ | ------------- |
| 错误率     | > 1%         | > 5%          |
| P99 延迟   | > 50ms       | > 100ms       |
| 连接失败率 | > 5%         | > 10%         |
| CPU        | > 80%        | > 95%         |
| 内存       | > 85%        | > 95%         |
| 活跃连接数 | > 9000       | > 9500        |
| 队列深度   | > 5000       | > 8000        |
| MQTT 断连  | > 30s        | > 60s         |
| 认证失败率 | > 10/min     | > 50/min      |

**配置结构**（供后续实现直接使用，加在 `ObservabilityConfig` 下）：

```yaml
observability:
  alerting:
    enabled: true
    channels:
      - type: slack
        webhook_url_file: /etc/relay/secrets/slack_webhook
        severity: warning,critical
      - type: email
        smtp_server: smtp.example.com:587
        from: relay-alerts@example.com
        to: ops-team@example.com
        severity: critical
    rules:
      - name: high_error_rate
        condition: error_rate > 0.05
        severity: critical
        message: "Error rate exceeded 5%"
      - name: high_latency
        condition: p99_latency_ms > 100
        severity: critical
        message: "P99 latency exceeded 100ms"
      - name: high_cpu_usage
        condition: cpu_usage_percent > 80
        severity: warning
        message: "CPU usage exceeded 80%"
      - name: mqtt_disconnected
        condition: mqtt_connected == false
        duration_seconds: 60
        severity: critical
        message: "MQTT broker disconnected for over 60 seconds"
    # 抑制规则
    suppression:
      # 同一告警 5 分钟内只发一次
      min_interval_seconds: 300
      # 维护窗口抑制所有告警
      maintenance_windows: []
```

**后续落地要点**：
- AlertEvaluator 周期性（每 10-30s）评估规则条件
- 告警状态机：OK → Warning → Critical，含冷却时间
- Critical 抑制 Warning（同指标）
- 通知渠道：Slack webhook / SMTP / 预留 PagerDuty

---

#### 8.5 补齐清单与执行顺序

**本次 MVP 周期必须完成的（当前状态）**：

| 序号  | 任务                             | 说明                                   | 预估工作量 |
| ----- | -------------------------------- | -------------------------------------- | ---------- |
| 8.1.1 | `audit.rs` 基础设施              | 已完成                                 | ✅          |
| 8.1.2 | 审计配置结构                     | 已完成                                 | ✅          |
| 8.1.3 | Auth/RBAC 埋点                   | 已完成                                 | ✅          |
| 8.1.4 | Connection 埋点                  | 已完成                                 | ✅          |
| 8.1.5 | Stream / Controller Request 埋点 | 已完成                                 | ✅          |
| 8.1.6 | Rate Limiter 埋点                | 已完成                                 | ✅          |
| 8.1.7 | Session 埋点                     | 已完成                                 | ✅          |
| 8.1.8 | 测试补齐                         | 已完成基础覆盖，后续可继续扩展集成验证 | ✅          |

**后续迭代（P2 可选增强）**：
- 扩展 `/metrics` 指标维度与 histogram 覆盖
- 接入真实外部告警通道（Slack / SMTP / PagerDuty）
- 补充更细粒度 tracing span 与跨进程上下文传播

**验收标准（阶段 4 整体）**：
- [x] `/health` 返回完整组件状态和资源指标
- [x] `/health/live` `/health/ready` `/health/startup` 可用
- [x] `/metrics` 输出核心 Prometheus 指标
- [x] 审计日志按 JSONL 格式输出，覆盖核心事件类型
- [x] Token 脱敏生效（仅前 8 位）
- [x] 文件轮转正常工作
- [x] 审计日志不阻塞请求热路径（mpsc 异步写入）
- [x] relay telemetry 周期发布到 MQTT
- [x] tracing 可按配置启用并导出到 OTLP
- [x] alerting 规则可本地评估并输出告警事件

**依赖**
- 核心服务框架基本成型（已满足）
- Auth/RBAC 模块（已实现，需在现有基础上添加审计埋点）
- Connection/Stream 模块（需要这些模块稳定后添加审计埋点）

---

### 9) 测试体系
**目标**：为 MVP 的正确性、稳定性和性能建立验证闭环，确保各层功能符合需求规格，并为后续迭代建立可持续维护的测试基础设施。

**状态总览**：

| 测试类别   | 覆盖范围                                                            | 需要补齐的内容                                                                              |
| ---------- | ------------------------------------------------------------------- | ------------------------------------------------------------------------------------------- |
| 单元测试   | `idempotency.rs`/`session.rs`/`rate_limiter.rs`/`audit.rs` 部分覆盖 | `auth.rs`/`rbac.rs`/`validator.rs`/`stream.rs`/`mqtt.rs` 完全无测试；已有模块需补充边界用例 |
| 集成测试   | 无 `tests/` 目录                                                    | 7+ 个集成测试文件需新建                                                                     |
| SDK 测试   | `device-sdk`/`controller-sdk` 完全无测试                            | SDK 单元测试需补齐                                                                          |
| 性能测试   | 无基准测试                                                          | Criterion 基准 + 负载压测程序需新建                                                         |
| 安全测试   | 无                                                                  | 8 个安全场景测试需新建                                                                      |
| 故障注入   | 无                                                                  | 6 个故障场景测试需新建                                                                      |
| 覆盖率门控 | 无                                                                  | CI 覆盖率门控配置需新建                                                                     |

---

#### 9.1 测试基础设施（Test Infrastructure）

**位置**：`crates/relay/tests/` + `crates/relay/src/test_helpers.rs` + `crates/relay/benches/`

##### 9.1.1 新增 dev-dependencies

在 `crates/relay/Cargo.toml` 中新增：

```toml
[dev-dependencies]
tokio = { workspace = true, features = ["macros", "rt-multi-thread", "time", "test-util"] }
tokio-test = "0.4"
tempfile = "3"                          # 已有，用于审计文件测试
wiremock = "0.6"                        # HTTP mock server（认证服务 mock）
testcontainers = "0.23"                 # Docker 容器化测试（MQTT Broker）
testcontainers-modules = { version = "0.11", features = ["mosquitto"] }
fake = { version = "2", features = ["derive"] }
proptest = "1"                          # 属性测试（边界值）
criterion = { version = "0.5", features = ["async_tokio"] }
rstest = "0.23"                         # 参数化测试
tracing-test = "0.2"                    # 测试中捕获 tracing 输出
```

##### 9.1.2 测试辅助模块

新建 `crates/relay/src/test_helpers.rs`（`#[cfg(test)]` 门控）：

```rust
// 核心辅助工具（仅测试环境可用）
pub mod test_helpers {
    /// 构造测试用 AppState（不启动真实 gRPC 监听）
    pub async fn build_test_state() -> Arc<AppState>;

    /// 构造一个有效的 Controller JWT（HS256，给定 role / project_ids）
    pub fn make_controller_jwt(controller_id: &str, role: &str, allowed_projects: &[&str]) -> String;

    /// 构造一个有效的 Device Token
    pub fn make_device_token(device_id: &str, project_id: &str) -> String;

    /// 向 SessionManager 注入一个模拟在线设备
    pub async fn register_mock_device(state: &AppState, device_id: &str, project_id: &str, region: &str);

    /// 启动测试用 gRPC server（随机端口，返回 SocketAddr）
    pub async fn start_test_relay_server(state: Arc<AppState>) -> (SocketAddr, JoinHandle<()>);

    /// 启动测试用 MQTT Broker（通过 testcontainers）
    pub async fn start_test_mqtt_broker() -> MqttBrokerHandle;

    /// 创建测试用 Config（默认值 + 覆盖项）
    pub fn test_config(overrides: &[(ConfigKey, &str)]) -> RelayConfig;
}
```

##### 9.1.3 集成测试公共固件

新建 `crates/relay/tests/common/`：

```
crates/relay/tests/
├── common/
│   ├── mod.rs                 # 公共模块导出
│   ├── fixtures.rs            # 测试固件（证书、token、设备数据）
│   └── mqtt_subscriber.rs     # 轻量 MQTT 订阅客户端（用于验证 MQTT 通知）
├── test_device_lifecycle.rs   # 设备生命周期集成测试
├── test_controller_flow.rs    # Controller 流程集成测试
├── test_data_relay.rs         # 数据中继集成测试
├── test_session_recovery.rs   # 会话恢复集成测试
├── test_mqtt_discovery.rs     # MQTT 服务发现集成测试
├── test_auth_integration.rs   # 认证授权端到端测试
├── test_fault_scenarios.rs    # 故障注入测试
└── test_security.rs           # 安全场景测试
```

---

#### 9.2 单元测试（Unit Tests）

**覆盖率目标**：核心逻辑（auth/rbac/idempotency/session/rate_limiter）> 90%，整体 > 80%。

**工具**：`cargo test` + `cargo llvm-cov`。

##### 9.2.1 认证模块（`auth.rs`）—— 新建 `#[cfg(test)] mod tests`

| 测试函数                                       | 验证场景                                                      | 优先级 |
| ---------------------------------------------- | ------------------------------------------------------------- | ------ |
| `test_authenticate_controller_valid_jwt`       | 合法 JWT，正确解析 controller_id/role/project_ids             | P0     |
| `test_authenticate_controller_expired_jwt`     | 过期 JWT 返回 `InvalidToken`                                  | P0     |
| `test_authenticate_controller_wrong_signature` | 错误签名返回 `InvalidToken`                                   | P0     |
| `test_authenticate_controller_id_mismatch`     | claims 中 controller_id 与请求不符返回 `ControllerIdMismatch` | P0     |
| `test_authenticate_controller_revoked_token`   | 已撤销 token 返回 `RevokedToken`                              | P0     |
| `test_authenticate_device_valid_token`         | 合法设备 token 返回 `DevicePrincipal`                         | P0     |
| `test_authenticate_device_unknown_token`       | 未注册设备 token 返回 `UnknownDevice`                         | P0     |
| `test_revoke_token_takes_effect`               | 撤销后再用同一 token 认证应失败                               | P1     |
| `test_auth_disabled_allows_all`                | 认证关闭时任意 token 通过（`is_enabled=false`）               | P1     |
| `test_token_prefix_truncation`                 | `token_prefix()` 只返回前 8 位                                | P0     |

##### 9.2.2 授权模块（`rbac.rs`）—— 新建 `#[cfg(test)] mod tests`

| 测试函数                                           | 验证场景                              | 优先级 |
| -------------------------------------------------- | ------------------------------------- | ------ |
| `test_admin_can_access_any_device`                 | admin 角色可访问任意设备              | P0     |
| `test_operator_can_access_own_project_device`      | operator 可访问同 project 设备        | P0     |
| `test_operator_cannot_access_other_project_device` | operator 不可访问其他 project 设备    | P0     |
| `test_viewer_cannot_execute_control_command`       | viewer 禁止执行 method 白名单外操作   | P0     |
| `test_method_not_in_whitelist_rejected`            | 方法不在白名单返回 `MethodNotAllowed` | P0     |
| `test_rbac_disabled_allows_all`                    | RBAC 关闭时所有请求通过               | P1     |
| `test_authorization_denied_returns_correct_error`  | 拒绝时返回正确错误类型                | P0     |

##### 9.2.3 幂等缓存（`idempotency.rs`）—— 补充边界用例

| 测试函数                                     | 验证场景                                      | 优先级 |
| -------------------------------------------- | --------------------------------------------- | ------ |
| `test_basic_idempotency`                     | 已有：相同 sequence_number 返回缓存响应       | P0     |
| `test_expiry`                                | 已有：超时后缓存失效                          | P0     |
| `test_capacity_eviction`                     | 已有：超过 10K LRU 逐出                       | P0     |
| `test_concurrent_same_sequence`              | **补充**：并发相同 sequence_number 只处理一次 | P1     |
| `test_cache_expired_return_device_not_found` | **补充**：缓存过期后重新转发到 device         | P1     |

##### 9.2.4 会话管理（`session.rs`）—— 补充边界用例

| 测试函数                                 | 验证场景                                       | 优先级 |
| ---------------------------------------- | ---------------------------------------------- | ------ |
| `test_register_device`                   | 已有：首次注册成功，返回 connection_id         | P0     |
| `test_heartbeat_update`                  | 已有：心跳更新 last_seen                       | P0     |
| `test_device_offline_after_timeout`      | **补充**：超时未心跳标记离线                   | P0     |
| `test_session_recovery_within_window`    | 已有：300s 内重连恢复会话                      | P0     |
| `test_session_recovery_after_expiry`     | **补充**：超过 300s 后重连创建新会话           | P0     |
| `test_graceful_disconnect_cleans`        | **补充**：正常断开立即清理                     | P0     |
| `test_duplicate_registration_replaces`   | **补充**：重新注册替换旧会话                   | P1     |
| `test_list_devices_region_filter`        | **补充**：`list_online_devices` 按 region 过滤 | P1     |
| `test_concurrent_register_and_heartbeat` | **补充**：并发操作无数据竞争                   | P1     |

##### 9.2.5 限流器（`rate_limiter.rs`）—— 补充边界用例

| 测试函数                                    | 验证场景                          | 优先级 |
| ------------------------------------------- | --------------------------------- | ------ |
| `test_device_rate_limit`                    | 已有：per-device 请求限流         | P0     |
| `test_sliding_window`                       | 已有：滑动窗口计数器重置          | P0     |
| `test_controller_rate_limit`                | **补充**：per-controller 连接限流 | P0     |
| `test_global_rate_limit`                    | **补充**：全局连接限流            | P0     |
| `test_concurrent_stream_limit_per_device`   | **补充**：单设备并发流超限        | P0     |
| `test_bandwidth_limit_per_device`           | **补充**：单设备带宽超限          | P1     |
| `test_cpu_threshold_rejects_connections`    | **补充**：CPU > 80% 拒绝新连接    | P1     |
| `test_memory_threshold_rejects_connections` | **补充**：内存 > 12GB 拒绝新连接  | P1     |

##### 9.2.6 输入验证（`validator.rs`）—— 新建 `#[cfg(test)] mod tests`

| 测试函数                              | 验证场景                       | 优先级 |
| ------------------------------------- | ------------------------------ | ------ |
| `test_valid_device_id_accepted`       | 合法 device_id 格式通过        | P0     |
| `test_invalid_device_id_rejected`     | 非法字符/超长 device_id 被拒绝 | P0     |
| `test_valid_payload_size`             | payload < 10 MB 通过           | P0     |
| `test_payload_size_exceeds_limit`     | payload >= 10 MB 被拒绝        | P0     |
| `test_empty_required_fields_rejected` | 必填字段为空被拒绝             | P0     |
| `test_method_name_whitelist_check`    | method_name 白名单校验         | P0     |

##### 9.2.7 审计日志（`audit.rs`）—— 补充测试

| 测试函数                         | 验证场景                              | 优先级 |
| -------------------------------- | ------------------------------------- | ------ |
| `test_token_sanitization_in_log` | 输出中 token 只有前 8 位              | P0     |
| `test_no_payload_in_audit_log`   | 审计日志不含 `encrypted_payload` 明文 | P0     |
| `test_audit_jsonl_format_valid`  | 每行输出为合法 JSON（JSONL）          | P0     |
| `test_file_rotation_on_size`     | 文件超 100 MB 自动轮转                | P1     |
| `test_async_write_non_blocking`  | 审计写入不阻塞调用方（mpsc 通道）     | P1     |
| `test_event_filter_config`       | 配置指定类型时只记录对应事件          | P1     |

##### 9.2.8 SDK 单元测试

**`crates/device-sdk/src/`** —— 新建 `#[cfg(test)] mod tests`

| 测试函数                        | 验证场景                               | 优先级 |
| ------------------------------- | -------------------------------------- | ------ |
| `test_backoff_sequence`         | 指数退避序列：0s → 2s → 4s → ... → 60s | P1     |
| `test_backoff_max_delay_capped` | 最大延迟不超过 60s                     | P1     |
| `test_config_defaults_valid`    | 默认配置可正常加载                     | P1     |
| `test_config_from_yaml`         | YAML 配置正确解析到 Config 结构体      | P1     |

**`crates/controller-sdk/src/`** —— 新建 `#[cfg(test)] mod tests`

| 测试函数                     | 验证场景                             | 优先级 |
| ---------------------------- | ------------------------------------ | ------ |
| `test_session_seq_increment` | 每次发送 sequence_number 自增        | P1     |
| `test_error_code_mapping`    | gRPC 错误码正确映射到 SDK Error 类型 | P1     |

---

#### 9.3 集成测试（Integration Tests）

##### 9.3.1 设备生命周期测试（`test_device_lifecycle.rs`）

| 测试用例                               | 验证内容                                                | 优先级 |
| -------------------------------------- | ------------------------------------------------------- | ------ |
| `test_register_and_heartbeat`          | 设备注册成功返回 connection_id；心跳维持在线            | P0     |
| `test_device_in_list_after_register`   | 注册后 `ListOnlineDevices` 可查到                       | P0     |
| `test_graceful_disconnect`             | 正常断开后从在线列表移除                                | P0     |
| `test_timeout_disconnect`              | 使用 `tokio::time::advance` 模拟超时，验证离线判定      | P0     |
| `test_reconnect_within_window`         | 断开后 300s 内重连，session_resumed=true                | P0     |
| `test_reconnect_after_window`          | 超过 300s 重连，session_resumed=false，新 connection_id | P0     |
| `test_concurrent_register_100_devices` | 100 个设备并发注册，全部成功                            | P1     |

##### 9.3.2 Controller 流程测试（`test_controller_flow.rs`）

| 测试用例                               | 验证内容                              | 优先级 |
| -------------------------------------- | ------------------------------------- | ------ |
| `test_list_empty_when_no_devices`      | 无设备时返回空列表                    | P0     |
| `test_list_returns_registered_devices` | 有设备时返回完整列表                  | P0     |
| `test_list_region_filter`              | region_filter 正确过滤设备            | P1     |
| `test_connect_to_device_invalid_jwt`   | 无效 JWT 返回 gRPC `UNAUTHENTICATED`  | P0     |
| `test_connect_to_device_no_permission` | 无权限访问返回 `UNAUTHORIZED`         | P0     |
| `test_connect_to_device_not_found`     | 目标设备不存在返回 `DEVICE_NOT_FOUND` | P0     |
| `test_connect_to_offline_device`       | 目标设备离线返回 `DEVICE_OFFLINE`     | P0     |

##### 9.3.3 数据中继测试（`test_data_relay.rs`）

| 测试用例                                 | 验证内容                                                      | 优先级 |
| ---------------------------------------- | ------------------------------------------------------------- | ------ |
| `test_bidirectional_relay`               | Controller 发送 → Device 收到 → Device 返回 → Controller 收到 | P0     |
| `test_relay_opaque_forward`              | Relay 转发时不修改加密 payload（字节完全一致）                | P0     |
| `test_idempotent_not_duplicated`         | 相同 sequence_number 重发，设备只收到一次                     | P0     |
| `test_large_payload_10mb`                | 10 MB 大 payload 正常转发                                     | P0     |
| `test_oversized_payload_rejected`        | 超过 10 MB payload 被拒绝（不进入转发逻辑）                   | P0     |
| `test_concurrent_streams_10_controllers` | 10 个 Controller 并发向同一设备发请求，全部正确路由           | P1     |

##### 9.3.4 会话恢复测试（`test_session_recovery.rs`）

| 测试用例                                         | 验证内容                                              | 优先级 |
| ------------------------------------------------ | ----------------------------------------------------- | ------ |
| `test_recovery_restores_connection`              | 重连后会话恢复，返回原 connection_id                  | P1     |
| `test_pending_messages_delivered_after_recovery` | 断线期间缓冲的消息在重连后投递                        | P1     |
| `test_controller_notified_on_recovery`           | 设备重连后 Controller 收到在线通知（via MQTT 或查询） | P1     |

##### 9.3.5 MQTT 服务发现测试（`test_mqtt_discovery.rs`）

使用 `testcontainers-modules::mosquitto` 启动真实 MQTT Broker 容器。需要 Docker 环境。

| 测试用例                                           | 验证内容                                                        | 优先级 |
| -------------------------------------------------- | --------------------------------------------------------------- | ------ |
| `test_device_online_published`                     | 设备注册后订阅端收到 `relay/device/online`，字段完整            | P1     |
| `test_device_offline_graceful_published`           | 正常断开后收到 `relay/device/offline`，reason=graceful_shutdown | P1     |
| `test_device_offline_timeout_published`            | 心跳超时后收到 `relay/device/offline`，reason=timeout           | P1     |
| `test_relay_telemetry_periodic_publish`            | `telemetry/relay/{relay_id}` 定期发布                           | P1     |
| `test_mqtt_broker_disconnect_graceful_degradation` | MQTT Broker 停止时 gRPC 继续工作                                | P1     |
| `test_list_fallback_when_mqtt_unavailable`         | MQTT 不可用时 `ListOnlineDevices` 作为补偿                      | P1     |

##### 9.3.6 认证授权端到端测试（`test_auth_integration.rs`）

| 测试用例                               | 验证内容                             | 优先级 |
| -------------------------------------- | ------------------------------------ | ------ |
| `test_device_invalid_token_rejected`   | 设备使用无效 token 注册被拒绝        | P1     |
| `test_controller_invalid_jwt_rejected` | Controller 使用无效 JWT 被拒绝       | P1     |
| `test_controller_expired_jwt_rejected` | Controller 使用过期 JWT 被拒绝       | P1     |
| `test_revoked_token_immediately_fails` | 调用撤销接口后该 token 立即失效      | P1     |
| `test_viewer_cannot_send_command`      | viewer 角色尝试控制命令被拒绝        | P1     |
| `test_cross_project_access_denied`     | Controller 跨 project 访问设备被拒绝 | P1     |
| `test_auth_failure_audit_event`        | 认证失败触发 `auth_failure` 审计事件 | P1     |

---

#### 9.4 故障注入测试（Fault Injection）

**目录**：`crates/relay/tests/test_fault_scenarios.rs`

**工具**：`tokio::time::pause/advance`、Docker 容器启停

| 测试用例                                    | 注入类型             | 验证内容                        | 优先级 |
| ------------------------------------------- | -------------------- | ------------------------------- | ------ |
| `test_device_reconnect_after_network_loss`  | 模拟网络中断后恢复   | 设备重连成功，会话可恢复        | P1     |
| `test_heartbeat_timeout_marks_offline`      | 时间快进 > 300s      | 设备标记离线，MQTT 离线通知发出 | P1     |
| `test_mqtt_broker_failure_relay_continues`  | MQTT Broker 容器停止 | gRPC 转发链路继续正常工作       | P1     |
| `test_mqtt_reconnect_after_broker_recovery` | MQTT Broker 恢复     | Relay 自动重连，恢复状态发布    | P1     |
| `test_batch_100_devices_disconnect`         | 100 个设备同时断连   | 无内存泄漏，资源正确释放        | P1     |
| `test_rate_limit_under_burst_traffic`       | 突发请求超过限流阈值 | 超限请求被拒绝，正常请求继续    | P1     |

---

#### 9.5 安全测试（Security Tests）

**目录**：`crates/relay/tests/test_security.rs`

| 测试用例                                      | 验证场景                                      | 优先级 |
| --------------------------------------------- | --------------------------------------------- | ------ |
| `test_unauthenticated_request_rejected`       | 不携带 token 的请求被拒绝                     | P1     |
| `test_forged_jwt_rejected`                    | 使用不同密钥签名的 JWT 被拒绝                 | P1     |
| `test_cross_device_access_rejected`           | Controller A 无法访问 Controller B 的专属设备 | P1     |
| `test_replay_with_duplicate_sequence`         | 重放相同 sequence_number 不触发重复执行       | P1     |
| `test_oversized_payload_rejected`             | payload > 10 MB 立即被拒绝（不进入转发逻辑）  | P1     |
| `test_device_impersonation_rejected`          | 设备 A 使用设备 B 的 device_id 被拒绝         | P1     |
| `test_connection_rate_limit_per_ip`           | 单 IP 连接速率超限后新连接被拒绝              | P1     |
| `test_token_revocation_immediately_effective` | 撤销后无缓存窗口内立即生效                    | P1     |

---

#### 9.6 性能测试（Performance Tests）

##### 9.6.1 Criterion 微基准测试

**目录**：`crates/relay/benches/`

| 基准测试                                                        | 指标目标                           | 优先级 |
| --------------------------------------------------------------- | ---------------------------------- | ------ |
| `bench_auth.rs` — `bench_jwt_verification`                      | 单次 JWT 验证 < 0.1ms              | P1     |
| `bench_idempotency.rs` — `bench_cache_hit` / `bench_cache_miss` | 缓存命中 < 0.01ms，未命中 < 0.05ms | P1     |
| `bench_session.rs` — `bench_session_lookup`                     | DashMap 查找 < 0.01ms              | P1     |
| `bench_rate_limiter.rs` — `bench_rate_check`                    | 单次限流检查 < 0.05ms              | P1     |

##### 9.6.2 负载场景测试（作为 example 手动运行）

**目录**：`crates/relay/examples/`

| 场景                                       | 目标                                      | 验收标准                                            | 优先级 |
| ------------------------------------------ | ----------------------------------------- | --------------------------------------------------- | ------ |
| **连接压力**（`load_test_connections.rs`） | 10,000 并发设备长连接                     | 全部建立成功；CPU < 80%；内存 < 2 GB；P99 建立 < 1s | P2     |
| **流吞吐量**（`load_test_streams.rs`）     | 1,000 并发活跃流 × 1MB 消息               | P99 延迟 < 20ms；平均吞吐 > 100 MB/s                | P2     |
| **延迟基线**（`load_test_latency.rs`）     | 1,000 设备 + 100 Controller 小消息（1KB） | P50 < 5ms；P99 < 20ms；P99.9 < 50ms                 | P2     |

---

#### 9.7 覆盖率与 CI 集成

##### 9.7.1 覆盖率工具

```bash
# 安装
cargo install cargo-llvm-cov

# 运行覆盖率分析（单元 + 集成测试）
cargo llvm-cov --workspace --html --output-dir coverage/

# 覆盖率门控（CI 中）
cargo llvm-cov --workspace --fail-under-lines 80
```

##### 9.7.2 CI 测试阶段划分

| CI 阶段            | 命令                                               | 触发时机              |
| ------------------ | -------------------------------------------------- | --------------------- |
| `test-unit`        | `cargo test --workspace --lib`                     | 每次 PR/push          |
| `test-integration` | `cargo test --workspace --test '*'`（需 Docker）   | 每次 PR               |
| `test-coverage`    | `cargo llvm-cov --workspace --fail-under-lines 80` | 每次 PR               |
| `bench-check`      | `cargo bench --no-run`（仅验证编译）               | 每次 PR               |
| `bench-run`        | `cargo bench` 完整运行                             | 手动 / 发版前         |
| `perf-test`        | `cargo run --example load_test_connections`        | 手动 / Week 12 验收前 |

---

#### 9.8 交付物清单

| 序号  | 交付物                           | 文件路径                                               | 优先级 |
| ----- | -------------------------------- | ------------------------------------------------------ | ------ |
| 9.1a  | 测试辅助模块                     | `crates/relay/src/test_helpers.rs`                     | P0     |
| 9.1b  | 集成测试公共目录                 | `crates/relay/tests/common/`                           | P0     |
| 9.2.1 | auth.rs 单元测试（10 个）        | `crates/relay/src/auth.rs`                             | P0     |
| 9.2.2 | rbac.rs 单元测试（7 个）         | `crates/relay/src/rbac.rs`                             | P0     |
| 9.2.3 | idempotency.rs 补充测试（2 个）  | `crates/relay/src/idempotency.rs`                      | P0     |
| 9.2.4 | session.rs 补充测试（5 个）      | `crates/relay/src/session.rs`                          | P0     |
| 9.2.5 | rate_limiter.rs 补充测试（6 个） | `crates/relay/src/rate_limiter.rs`                     | P0     |
| 9.2.6 | validator.rs 单元测试（6 个）    | `crates/relay/src/validator.rs`                        | P0     |
| 9.2.7 | audit.rs 补充测试（6 个）        | `crates/relay/src/audit.rs`                            | P0     |
| 9.2.8 | SDK 单元测试                     | `crates/device-sdk/src/`、`crates/controller-sdk/src/` | P1     |
| 9.3.1 | 设备生命周期集成测试（7 个）     | `crates/relay/tests/test_device_lifecycle.rs`          | P0     |
| 9.3.2 | Controller 流程集成测试（7 个）  | `crates/relay/tests/test_controller_flow.rs`           | P0     |
| 9.3.3 | 数据中继集成测试（6 个）         | `crates/relay/tests/test_data_relay.rs`                | P0     |
| 9.3.4 | 会话恢复集成测试（3 个）         | `crates/relay/tests/test_session_recovery.rs`          | P1     |
| 9.3.5 | MQTT 服务发现集成测试（6 个）    | `crates/relay/tests/test_mqtt_discovery.rs`            | P1     |
| 9.3.6 | 认证授权集成测试（7 个）         | `crates/relay/tests/test_auth_integration.rs`          | P1     |
| 9.4   | 故障注入测试（6 个）             | `crates/relay/tests/test_fault_scenarios.rs`           | P1     |
| 9.5   | 安全测试（8 个）                 | `crates/relay/tests/test_security.rs`                  | P1     |
| 9.6.1 | Criterion 基准测试（4 个 bench） | `crates/relay/benches/`                                | P1     |
| 9.6.2 | 负载压测 examples（3 个）        | `crates/relay/examples/`                               | P2     |
| 9.7   | CI 覆盖率门控配置                | `.github/workflows/` 或 CI 配置文件                    | P1     |

---

#### 9.9 执行顺序

```
前置依赖：核心功能实现完成（Section 3–8）
    │
    ├─ 步骤 1：补充 Cargo.toml dev-dependencies
    │
    ├─ 步骤 2：构建 test_helpers + tests/common 基础设施
    │
    ├─ 步骤 3：补齐单元测试
    │   ├─ auth.rs（10 个测试）
    │   ├─ rbac.rs（7 个测试）
    │   ├─ validator.rs（6 个测试）
    │   ├─ idempotency.rs 补充（2 个）
    │   ├─ session.rs 补充（5 个）
    │   ├─ rate_limiter.rs 补充（6 个）
    │   ├─ audit.rs 补充（6 个）
    │   └─ SDK 单元测试（4 + 2 个）
    │
    ├─ 步骤 4：集成测试
    │   ├─ test_device_lifecycle.rs
    │   ├─ test_controller_flow.rs
    │   ├─ test_data_relay.rs
    │   ├─ test_session_recovery.rs
    │   ├─ test_mqtt_discovery.rs（需 Docker）
    │   └─ test_auth_integration.rs
    │
    ├─ 步骤 5：故障注入 + 安全测试
    │   ├─ test_fault_scenarios.rs
    │   └─ test_security.rs
    │
    ├─ 步骤 6：基准测试（benches/）
    │
    └─ 步骤 7：CI 覆盖率门控 + 负载压测 examples
```

---

#### 9.10 验收标准

| 类别                    | 标准                                                                    |
| ----------------------- | ----------------------------------------------------------------------- |
| **单元测试覆盖率**      | 核心模块（auth/rbac/idempotency/session/rate_limiter）> 90%；整体 > 80% |
| **集成测试通过率**      | 所有 P0 集成测试 100% 通过                                              |
| **安全测试**            | 8 个安全场景全部通过，无高危绕过                                        |
| **故障恢复**            | MQTT 断连、设备重连场景测试通过，无崩溃                                 |
| **性能基线**            | 4 个 Criterion 基准可运行并建立历史比对基线                             |
| **负载验收（Week 12）** | P99 延迟 < 20ms；10K 连接建立成功；1 小时稳定性无内存泄漏               |

---

### 10) 部署与发布
**状态**：完成（含 release 自动构建发布）

**目标**：让系统可部署、可启动、可回滚，具备生产级 CI/CD 和监控基础设施。

> 部署形态包括 Docker、Kubernetes、以及 Linux 裸机（systemd）三条路径。

---

#### 10.1 Docker 镜像构建优化

**状态**：完成

**目标**：生产级 Docker 镜像，分层缓存、安全加固、可复现构建。

**交付物**:

1. **`rust-toolchain.toml`** — 固定 Rust 工具链版本（1.75），替代 Dockerfile 中的硬编码
2. **`Dockerfile`** 优化:
   - Layer caching：先复制 Cargo.toml/Cargo.lock，`cargo fetch`，再复制源码编译
   - `--locked` flag 保证可复现构建
   - 非 root 用户运行（`USER relay`）
   - `HEALTHCHECK` 指令内置于镜像
   - OCI labels（`org.opencontainers.image.*`）
3. **`.dockerignore`** — 排除 target/、.git/、doc/、.github/ 等不必要文件

**验收**:
- `docker build -t grpc-relay:latest .` 成功，镜像 < 200 MB
- 容器以 relay 用户（非 root）运行
- OCI labels 可查询: `docker inspect grpc-relay:latest | jq '.[0].Config.Labels'`

---

#### 10.2 docker-compose 完善

**目标**：本地一键启动完整开发/测试环境，包括监控栈。

**交付物**:

1. **`docker-compose.yml`** 增强 — 新增 3 个服务:
   - `prometheus` — 挂载 `deploy/prometheus/prometheus.yml`，抓取 relay:8080
   - `grafana` — 自动加载 datasource + dashboard provision
   - `jaeger` — OTLP tracing 收集器（可选）
   - relay 服务 `depends_on` mqtt，统一 `relay-network` 网络

2. **`deploy/prometheus/prometheus.yml`** — Prometheus 抓取配置
   - `relay` job: 抓取 `/metrics` endpoint at relay:8080
   - 15s 抓取间隔，30 天数据保留

3. **`deploy/grafana/datasources/prometheus.yml`** — Grafana 自动数据源配置

4. **`deploy/grafana/dashboards/relay-overview.json`** — 预置仪表板
   - Overview: 健康状态、活跃连接数、活跃流数
   - Performance: P50/P95/P99 延迟、吞吐量
   - Errors & Resources: 错误率、CPU、内存
   - MQTT: 发布速率、连接状态

**验收**:
- `docker-compose up -d` 全部服务启动
- `curl localhost:9090/targets` relay UP
- `curl localhost:3000` Grafana 可访问，仪表板自动加载

---

#### 10.3 裸机部署方案（systemd）

**目标**：提供不依赖容器编排的 Linux 主机部署路径，适用于边缘节点、单机房或已有主机运维体系的环境。

**交付物**:

1. **`deploy/bare-metal/relay.service`** — `systemd` unit
   - 非 root 用户 `relay`
   - `EnvironmentFile=/etc/grpc-relay/relay.env`
   - `Restart=on-failure`
   - 基础硬化：`NoNewPrivileges`、`ProtectSystem=strict`

2. **`deploy/bare-metal/relay.env.example`** — 裸机环境变量模板
   - `RELAY_ID`, `RUST_LOG`
   - `RELAY__MQTT__BROKER_ADDRESS`
   - `RELAY__AUTH__JWT__HS256_SECRET`
   - `RELAY__TLS__*`, `RELAY__OBSERVABILITY__TRACING__*`

3. **`deploy/bare-metal/install.sh` / `upgrade.sh` / `uninstall.sh`** — 裸机生命周期脚本
   - 安装：创建用户/目录、安装 binary/config/unit、启用服务
   - 升级：替换 binary，可选覆盖 config，重启服务
   - 卸载：停用服务，按需保留或清理宿主机数据

4. **`deploy/bare-metal/README.md`** — 安装目录约定与脚本使用方式
   - `install.sh`
   - `upgrade.sh`
   - `uninstall.sh`

**验收**:
- 在 Linux 主机执行手册步骤后，`systemctl status relay` 正常
- `curl http://127.0.0.1:8080/health` 返回健康状态

---

#### 10.4 Kubernetes Manifests 完善

**目标**：从单文件拆分为专业多文件布局，补齐 HPA/PDB/NetworkPolicy，提高安全性和可维护性。

**交付物** — 拆分 `deploy/kubernetes/relay.yaml` 为 10 个独立资源文件：

| 文件 | 内容 | 关键变更 |
|---|---|---|
| `namespace.yaml` | Namespace `relay-system` | 新增 `app.kubernetes.io/part-of` label |
| `serviceaccount.yaml` | ServiceAccount `relay` | 供 Deployment 显式绑定，避免依赖默认 ServiceAccount |
| `configmap.yaml` | ConfigMap `relay-config` | 与 `config/relay.yaml` 同步，包含 stream/rate_limiting/audit/alerting 配置 |
| `secret.yaml` | Secret `relay-secrets` | TLS 证书 + JWT 密钥模板（`stringData` 占位） |
| `deployment.yaml` | Deployment | 新增 `securityContext`（非 root、readOnlyRootFS），resources（2CPU/4Gi req → 8CPU/16Gi limit），Pod anti-affinity，startupProbe |
| `service.yaml` | Service | 类型 `ClusterIP`，3 端口（gRPC/QUIC/health） |
| `hpa.yaml` | HorizontalPodAutoscaler | CPU 70% + Memory 80% 触发，min 1 / max 10 |
| `pdb.yaml` | PodDisruptionBudget | `maxUnavailable: 1` |
| `networkpolicy.yaml` | NetworkPolicy | 仅放行 50051/TCP, 50052/UDP, 8080/TCP 入站 |
| `kustomization.yaml` | Kustomize 入口 | 统一管理所有资源，`namePrefix: grpc-relay-` |

**安全加固**:
- `securityContext.runAsNonRoot: true`, `readOnlyRootFilesystem: true`
- `allowPrivilegeEscalation: false`, `capabilities.drop: [ALL]`
- JWT secret 通过 `secretKeyRef` 注入，不写入 ConfigMap
- NetworkPolicy 限制入站端口

**验收**:
- `kubectl apply -k deploy/kubernetes/` 全部资源 reconciliation 成功
- Pod Ready，startup/liveness/readiness 探针通过
- HPA 评估正常：`kubectl get hpa -n relay-system`

---

#### 10.5 CI/CD Pipeline (GitHub Actions)

**目标**：每次 PR 自动执行检查、测试、覆盖率；tag 推送自动构建并发布 Docker 镜像。

**交付物** — `.github/workflows/ci.yml`（5 个 job） + `.github/workflows/release.yml`（1 个 release workflow）：

| Job | 触发条件 | 内容 |
|---|---|---|
| `check` | 每次 push/PR | `cargo fmt --check`, `cargo clippy -- -D warnings`, `cargo check` |
| `test` | 每次 push/PR | `cargo test --workspace --lib`, `cargo test --workspace --test '*'`（含 mosquitto service container） |
| `coverage` | 每次 push/PR | `cargo llvm-cov --workspace --fail-under-lines 80` |
| `build` | PR / tag push | `docker build`（GHA cache），tag push 时推送至 ghcr.io |
| `bench` | `workflow_dispatch` | `cargo bench --no-run`（编译检查） |

`.github/workflows/release.yml`：

| Workflow | 触发条件 | 内容 |
|---|---|---|
| `release` | GitHub Release published | 自动构建并推送 release 镜像到 ghcr.io（含 `latest`、tag、sha 标签） |

**关键配置**:
- `actions-rust-lang/setup-rust-toolchain@v1` 自动安装 `rust-toolchain.toml` 指定版本
- `Swatinem/rust-cache@v2` 缓存 `target/`（基于 Cargo.lock hash）
- Docker buildx + GHA cache 加速镜像构建
- Coverage job 使用 `taiki-e/install-action@cargo-llvm-cov` faster installation

**验收**:
- CI 全部 job 绿灯通过
- Docker 镜像构建成功（PR 触发 build，不 push）
- Tag push 时镜像推送到 ghcr.io

---

#### 10.6 配置管理完善

**目标**：文档化环境变量，确保配置一致性。

**交付物**:

1. **`.env.example`** — 文档化所有关键环境变量:
   - `RELAY_ID`, `RUST_LOG`
   - `RELAY__MQTT__BROKER_ADDRESS`
   - `RELAY__AUTH__JWT__HS256_SECRET`
   - `RELAY__TLS__*`, `RELAY__OBSERVABILITY__TRACING__*`

2. **`config/relay.yaml`** — 已与需求文档 §9.3 对齐，包含重连/带宽/CPU/内存阈值

3. **`deploy/kubernetes/configmap.yaml`** — 与 `config/relay.yaml` 结构一致

**验收**:
- 新开发者可从 `.env.example` 了解需配置的环境变量
- `config/relay.yaml` 与 K8s ConfigMap 结构、字段语义一致

---

#### 10.7 运维手册（`doc/operations.md`）

**交付物** — 完整的运维操作手册，对齐需求文档 §9.5：

1. **启动服务** — Docker / K8s 命令，含健康检查验证
2. **停止服务** — Docker / K8s / 裸机优雅关闭流程（SIGTERM → 30s 排空 → 退出）
3. **滚动更新** — `kubectl rollout` / `docker-compose up --no-deps` + 回滚命令
4. **扩容/缩容** — `kubectl scale` / HPA 调整
5. **日志查看** — `kubectl logs` / `docker-compose logs` + 审计日志路径
6. **指标查看** — PromQL 关键查询（连接数、P99 延迟、错误率、MQTT 等）
7. **故障排查** — 4 个高频问题诊断步骤（设备无法连接、高延迟、MQTT 断连、内存泄漏）
8. **备份和恢复** — ConfigMap/Secret 备份 + Redis 备份占位
9. **安全加固** — 证书轮换步骤、Token 撤销命令、JWT 密钥更新
10. **告警响应** — 10 类告警的 Warning/Critical 响应流程 + 抑制规则

**验收**:
- 按手册步骤可在干净环境完成一次部署、更新和基本故障排查

---

#### 10.8 发布检查清单

| 类别       | 检查项                               | 阻塞发布? |
| ---------- | ------------------------------------ | --------- |
| **功能**   | 所有 P0 功能通过集成测试             | 是        |
| **功能**   | gRPC 三个 RPC 正常响应               | 是        |
| **性能**   | P99 延迟 < 20ms（基准测试）          | 是        |
| **性能**   | 10K 连接压测通过，CPU < 80%          | 否        |
| **安全**   | TLS 1.3 全网有效                     | 是        |
| **安全**   | 安全测试 8 项全部通过                | 是        |
| **稳定性** | 24h 稳定性测试无内存/连接泄漏        | 否        |
| **观测性** | /health、/metrics、审计日志可用      | 是        |
| **部署**   | Docker 镜像构建并推送到 registry     | 是        |
| **部署**   | K8s manifests 在测试集群验证通过     | 是        |
| **部署**   | 裸机 `systemd` 方案按手册可启动      | 是        |
| **文档**   | `doc/operations.md` 手册可执行       | 是        |
| **文档**   | `CHANGELOG.md` 记录版本变更          | 是        |

---

#### 10.9 Helm Chart（P2 后续项）

首版不纳入。后续可通过 Helm Chart 替代 plain K8s manifests，支持模板化配置、多环境覆盖、一键安装/升级。

---

#### 交付物清单（Section 10）

| 文件                                      | 说明                  | 状态 |
| ----------------------------------------- | --------------------- | ---- |
| `rust-toolchain.toml`                     | Rust 工具链固定       | ✅    |
| `Dockerfile`                              | 生产级多阶段构建      | ✅    |
| `.dockerignore`                           | 构建上下文优化        | ✅    |
| `docker-compose.yml`                      | 本地全栈环境          | ✅    |
| `deploy/bare-metal/relay.service`        | 裸机 systemd unit     | ✅    |
| `deploy/bare-metal/relay.env.example`    | 裸机环境模板          | ✅    |
| `deploy/bare-metal/install.sh`           | 裸机安装脚本          | ✅    |
| `deploy/bare-metal/upgrade.sh`           | 裸机升级脚本          | ✅    |
| `deploy/bare-metal/uninstall.sh`         | 裸机卸载脚本          | ✅    |
| `deploy/bare-metal/README.md`            | 裸机部署手册          | ✅    |
| `.env.example`                            | 环境变量文档          | ✅    |
| `deploy/kubernetes/namespace.yaml`        | Namespace             | ✅    |
| `deploy/kubernetes/serviceaccount.yaml`   | ServiceAccount        | ✅    |
| `deploy/kubernetes/configmap.yaml`        | ConfigMap             | ✅    |
| `deploy/kubernetes/secret.yaml`           | Secret 模板           | ✅    |
| `deploy/kubernetes/deployment.yaml`       | Deployment            | ✅    |
| `deploy/kubernetes/service.yaml`          | Service               | ✅    |
| `deploy/kubernetes/hpa.yaml`              | HPA                   | ✅    |
| `deploy/kubernetes/pdb.yaml`              | PodDisruptionBudget   | ✅    |
| `deploy/kubernetes/networkpolicy.yaml`    | NetworkPolicy         | ✅    |
| `deploy/kubernetes/kustomization.yaml`    | Kustomize entry       | ✅    |
| `deploy/prometheus/prometheus.yml`        | Prometheus 抓取配置   | ✅    |
| `deploy/grafana/datasources/prometheus.yml` | Grafana 数据源      | ✅    |
| `deploy/grafana/dashboards/dashboards.yml`  | Dashboard provision | ✅    |
| `deploy/grafana/dashboards/relay-overview.json` | 预置仪表板       | ✅    |
| `.github/workflows/ci.yml`                | CI/CD pipeline        | ✅    |
| `.github/workflows/release.yml`           | Release 自动发布      | ✅    |
| `CHANGELOG.md`                            | 版本变更记录          | ✅    |
| `doc/operations.md`                       | 运维操作手册          | ✅    |

**依赖**
- 核心服务与观测性基本完成

---

## 三、推荐里程碑划分

### 阶段 1：架构冻结与工程化基础
**产出**
- 项目结构
- proto / topic / error code 定义
- 配置与日志框架
- 部署骨架

**验收**
- 项目可编译
- 配置可加载
- 空服务可启动
- 健康检查可返回

---

### 阶段 2：主连接链路打通
**产出**
- Device 注册 / 心跳 / 离线
- Controller 鉴权 / 查询在线设备
- 基础流转发

**验收**
- 设备可连上 Relay
- Controller 可查到在线设备
- Controller 可向设备转发一条消息并得到响应

---

### 阶段 3：安全与可靠性补齐
**产出**
- RBAC
- 幂等缓存
- 限流
- 断线重连
- 会话恢复

**验收**
- 未授权访问被拒绝
- 重复请求不重复执行
- 设备断线后可在窗口内恢复会话

---

### 阶段 4：发现、观测、运维
**产出**
- MQTT 通知
- Relay 遥测
- 指标 / 日志 / 审计
- 健康检查与运维文档

**验收**
- MQTT 可订阅在线/离线事件
- `/metrics` 和 `/health` 可用
- 审计日志可追溯关键操作

---

### 阶段 5：测试与发布准备
**产出**
- 单测 / 集成测试 / 压测脚本
- Docker / K8s 发布流程
- 发布前检查项

**验收**
- 核心测试通过
- 可完成一次端到端部署验证
- 形成 v1.0 发布候选

---

## 四、WBS 级任务清单

### A. 平台与工程基础
- 创建 Rust workspace
- 规划模块边界：transport / auth / rbac / session / stream / mqtt / observability / api
- 配置加载器与默认配置
- 错误类型与 Result 规范
- 日志与 tracing 基础
- CI 基础脚本

### B. 协议层
- 编写 proto（已完成）
- 生成 Rust gRPC 代码（已完成）
- 定义 MQTT topics 与 payload schema（已完成）
- 定义错误码映射（已完成）
- 定义健康检查与 metrics schema（部分完成：Health 已完成，`/metrics` 暂缓）

### C. 设备接入
- QUIC listener
- DeviceConnect 双向流
- 注册与心跳处理
- 连接状态机
- 断线检测与重连策略
- 会话恢复窗口

### D. Controller 接入
- HTTP/2 gRPC server
- JWT 验证
- ListOnlineDevices
- ConnectToDevice
- Controller request 元数据提取

### E. 中继与幂等
- Stream 路由映射
- sequence_number 去重
- 响应缓存
- 限流器
- payload 大小与字段校验

### F. 安全
- Device 证书 / Token 认证
- Controller JWT 认证
- RBAC policy engine
- method 白名单
- E2E payload opaque forwarding

### G. MQTT 与发现
- MQTT publisher / subscriber
- 上线/离线事件发布
- stationService 状态上报
- 查询补偿逻辑
- MQTT 断连降级

### H. 观测性
- `/health`
- `/metrics`
- 审计日志结构
- Relay telemetry publisher
- tracing 预留

### I. 测试
- 单元测试
- 集成测试
- 压测脚本
- 故障注入测试
- 安全测试用例

### J. 部署与文档
- Dockerfile
- docker-compose
- K8s manifests
- 配置示例
- 运维手册
- API 文档

---

## 五、优先级建议

### P0 必做
- 项目骨架
- proto / 接口定义
- Device 注册、心跳、离线
- Controller 鉴权与设备查询
- 流中继
- RBAC
- 健康检查
- 基础指标和日志

### P1 首版必须
- MQTT 上下线通知
- 会话恢复
- 幂等缓存
- 限流
- Relay 遥测
- 集成测试
- Docker / K8s 部署

### P2 后续版本
- 会话持久化
- 多 Relay
- 负载均衡
- Controller QUIC
- 0-RTT
- 更细粒度 ABAC

---

## 六、建议的执行顺序

1. 冻结接口契约  
2. 搭建工程骨架  
3. 实现认证与基础状态机  
4. 打通 Device 注册/心跳/离线  
5. 打通 Controller 查询与流转发  
6. 加上幂等、限流、RBAC  
7. 接入 MQTT 与遥测  
8. 接入健康检查、指标、审计  
9. 补齐测试  
10. 做部署与发布验证  

---

## 七、里程碑验收标准

### Week 2
- 接口定义完成
- 工程可启动
- 配置与日志可用

### Week 4
- Device 可稳定连接 Relay
- 心跳和离线流程可用
- Controller 可查询在线设备

### Week 6
- 双向流中继可用
- 授权与幂等逻辑可用
- 断线重连与会话恢复可用

### Week 9
- MQTT 通知与补偿查询可用
- 指标、日志、审计可用
- Relay 遥测可发布

### Week 12
- 测试、部署、文档完整
- 可进入 v1.0 发布候选

---

## 八、风险与控制点

- **QUIC 实现风险**：先保留 TCP fallback，不在首版追求高级 QUIC 特性
- **权限模型复杂**：优先 RBAC + 设备归属，ABAC 放后续
- **MQTT 可靠性**：用“Relay 发布为主、设备自报备份、Controller 查询补偿”三路兜底
- **性能目标过高**：尽早做压测基线，尽量在 Week 4-6 就发现瓶颈
- **会话恢复误判**：状态机必须显式化，避免隐式状态散落各处

---

如果你要，我下一步可以把这个计划进一步整理成：
1. **按周拆分的甘特式计划**，或  
2. **按团队角色拆分的任务分配表**。
