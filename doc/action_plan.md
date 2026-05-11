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
**状态**：部分完成（已提供 stationService SDK；Relay 侧 DeviceConnect 服务尚未落地）

**目标**：实现 stationService 与 Relay 的长连接、注册、心跳、断线重连、会话恢复。

**交付物**
- DeviceConnect 流程：SDK client 已实现；Relay server 尚未实现
- Register / Heartbeat / Disconnect 逻辑：SDK 已发送 Register/Heartbeat，并在断线后重连；Relay 侧处理尚未实现
- QUIC 连接与 TCP fallback：SDK 已保留 QUIC 配置并实现 tonic HTTP/2 TCP fallback；QUIC transport 尚未实现
- 心跳超时、离线判定、重连退避：SDK 已实现重连退避；Relay 心跳超时/离线判定尚未实现
- session / connection_id 管理：SDK 支持 recovery 窗口内携带 `previous_connection_id`；Relay session registry 尚未实现
- stationService SDK：见 `crates/device-sdk` 与 `doc/device_sdk.md`

**验收备注**
- stationService 是外部应用，不包含在本仓库内；本仓库交付其接入 SDK、示例代码和文档。
- 当前可验证项：`cargo check -p device-sdk --examples`。
- 端到端验收需要后续实现 Relay 的 `RelayService::DeviceConnect`、session 管理和 stream router。

**依赖**
- 协议与接口定义
- 基础工程与项目骨架

---

### 4) Controller 接入链路（外部程序 + Controller SDK）
**状态**：部分完成（Controller SDK 已交付；Relay 侧 `ListOnlineDevices` / `ConnectToDevice` 尚未落地）

**目标**：打通 Controller（外部程序）到 Relay 的认证、在线设备查询、会话建立，并交付一个可供外部 Controller 程序直接集成的客户端 SDK。

> 说明：Controller 是外部程序，不在本仓库内；当前仓库仅交付其接入 SDK。Relay 服务端仍处于 skeleton（尚未落地 `RelayService::ListOnlineDevices` / `RelayService::ConnectToDevice`），因此本章节在 MVP 阶段以“SDK 可编译、契约对齐、示例可运行（需配套 Relay 落地）”为主要验收标准；端到端联调在 Relay 实现阶段完成。

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

**依赖**
- 协议与接口定义（`crates/relay-proto/proto/relay/v1/relay.proto` / `doc/protocol_spec.md`）
- 认证与授权基础能力（Relay 落地后由服务端完成校验；SDK 先完成参数注入与契约对齐）


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

**交付物**
- `/health` 健康检查
- `/metrics` 指标导出
- 结构化日志
- 审计日志
- tracing 接入预留
- 告警阈值配置结构

**依赖**
- 核心服务框架基本成型

---

### 9) 测试体系
**目标**：为 MVP 的正确性、稳定性和性能建立验证闭环。

**交付物**
- 单元测试：认证、授权、幂等、会话管理、限流
- 集成测试：设备注册、Controller 查询、数据中继、MQTT 通知
- 故障测试：断连、重连、MQTT 失败、认证失败
- 基础性能测试：连接数、吞吐、延迟基线

**依赖**
- 核心功能实现完成后逐步补齐

---

### 10) 部署与发布
**目标**：让系统可部署、可启动、可回滚。

**交付物**
- Docker 镜像构建
- docker-compose 示例
- Kubernetes manifests
- 配置样例
- 运维手册草案
- 发布检查清单

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
