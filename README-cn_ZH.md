# gRPC-Relay | [English Version](README.md)

gRPC-Relay 是一个用于跨网域通信中继的系统，目标是在内网设备与外部控制端之间建立安全、可控、高性能的 gRPC 通信通道。

它面向以下场景：

- 内网设备通过 Relay 被公网或办公网中的 Controller 管理
- 支持双向流式数据传输，包括控制命令和文件/数据上传
- 通过 MQTT 提供设备上线/离线通知、状态上报与遥测数据
- 通过 gRPC 提供在线设备查询和流式中继能力

---

## 目录

- [背景与目标](#背景与目标)
- [核心角色](#核心角色)
- [系统架构](#系统架构)
- [核心流程](#核心流程)
- [接口设计](#接口设计)
- [安全与权限模型](#安全与权限模型)
- [非功能需求](#非功能需求)
- [部署与运维](#部署与运维)
- [测试策略](#测试策略)
- [MVP 范围与路线图](#mvp-范围与路线图)
- [参考资料](#参考资料)

---

## 背景与目标

gRPC-Relay 的核心目标是实现跨网域的 gRPC 中继能力，使处于内网、无公网 IP 的设备也能够被外部控制端安全访问和管理。

### 设计原则

- **可控中继**：Relay 只可见元数据，不解密业务 payload
- **端到端加密**：业务数据由 Device 与 Controller 端加密/解密
- **高可用基础优先**：先实现单节点 MVP，再扩展到多节点
- **可观测**：内置健康检查、指标、日志、审计和追踪
- **可降级**：QUIC 不可用时可降级到 TLS/TCP

---

## 核心角色

| 角色 | 说明 | 职责 |
|------|------|------|
| Device | 物理设备，如 IoT 设备或工作站 | 运行 stationService，执行业务逻辑 |
| stationService | 设备上的代理进程 | 与 Relay 长连接、注册、心跳、重连、上报状态 |
| Controller | 人机交互控制端 | 查询设备、发起连接、发送控制命令、接收响应 |
| Relay | 中继服务器 | 管理长连接、转发流量、发布通知、提供查询接口 |
| MQTT Broker | 消息代理服务 | 传输遥测数据和设备上下线通知 |

---

## 系统架构

### 协议分层

| 链路 | 协议 | 用途 |
|------|------|------|
| Device ↔ Relay | gRPC over QUIC | 设备长连接、低延迟传输 |
| Controller ↔ Relay | gRPC over HTTP/2 + TLS 1.3 | 控制端访问和查询 |
| Relay ↔ MQTT Broker | MQTT over TLS 1.3 | 设备通知和遥测 |
| 降级方案 | TLS/TCP | QUIC 不可用时回退 |

### 架构特点

- Relay 仅处理元数据、鉴权、授权、限流和流转发
- Device 与 Controller 之间的业务 payload 端到端加密
- MQTT Broker 独立部署，与 Relay 解耦
- 首版采用单 Relay 节点，后续支持多节点和负载均衡

---

## 核心流程

### 1. 设备注册与上线

1. Device 启动 stationService
2. stationService 连接到 Relay
3. Relay 验证设备身份
4. Relay 分配 `connection_id`
5. Relay 发布设备上线消息到 MQTT
6. stationService 可选发布自身状态作为备份验证

### 2. 心跳与保活

- stationService 每 30 秒发送一次心跳
- Relay 更新设备 `last_seen`
- 120 秒未收到心跳时标记为疑似离线
- 300 秒未收到心跳时关闭连接并发布离线消息

### 3. Controller 发现设备

支持三种方式互为备份：

- Relay 通过 MQTT 发布上线/离线事件
- stationService 通过 MQTT 上报状态
- Controller 通过 gRPC 查询在线设备列表

### 4. Controller 发起会话

1. Controller 获取目标设备信息
2. Controller 连接 Relay 并指定 `target_device_id`
3. Relay 验证 Controller 身份与权限
4. Relay 建立 Controller 与 Device 之间的流映射
5. Relay 开始转发双向流数据

### 5. 设备重连与会话恢复

- 设备断线后自动重连
- 重连时携带 `previous_connection_id`
- Relay 在会话恢复窗口内尝试恢复状态
- 如果恢复失败，则创建新会话并分配新 `connection_id`

### 6. 幂等性

- 请求携带全局唯一 `sequence_number`
- Relay 缓存最近处理过的序列号
- 重复请求直接返回缓存响应，避免重复执行

---

## 接口设计

### gRPC Service

核心服务包括：

- `DeviceConnect(stream DeviceMessage) returns (stream RelayMessage)`
- `ListOnlineDevices(ListOnlineDevicesRequest) returns (ListOnlineDevicesResponse)`
- `ConnectToDevice(stream ControllerMessage) returns (stream DeviceResponse)`

### 关键消息

- `DeviceMessage`：设备注册、心跳、数据上报
- `RelayMessage`：注册响应、心跳响应、数据请求
- `ControllerMessage`：控制端发往设备的请求
- `DeviceResponse`：设备返回的响应
- `ListOnlineDevicesRequest/Response`：在线设备查询

### MQTT Topics

| Topic | 用途 |
|------|------|
| `relay/device/online` | 设备上线通知 |
| `relay/device/offline` | 设备离线通知 |
| `device/{device_id}/status` | 设备自身状态上报 |
| `telemetry/{device_id}` | 设备遥测数据 |
| `telemetry/relay/{relay_id}` | Relay 遥测数据 |

### 错误码

- `OK`
- `DEVICE_OFFLINE`
- `UNAUTHORIZED`
- `DEVICE_NOT_FOUND`
- `RATE_LIMITED`
- `INTERNAL_ERROR`

---

## 安全与权限模型

### 认证

- **Device**：推荐使用 mTLS 设备证书，也支持预置 Token
- **Controller**：使用 JWT Token

### 授权

采用 **RBAC + 设备归属**：

- `admin`：访问所有设备
- `operator`：访问授权设备，执行控制和数据传输
- `viewer`：只读访问

### 安全要求

- 所有连接必须使用 TLS 1.3
- 业务 payload 必须端到端加密
- Relay 不得记录加密 payload 内容
- 限流策略按设备、Controller 和全局维度控制
- 需要验证 `device_id`、`controller_id`、`method_name` 等元数据

---

## 非功能需求

### 性能目标

| 指标 | 目标 |
|------|------|
| 单实例长连接 | 10,000 |
| 并发活跃流 | 1,000 |
| Relay 单跳额外延迟 | P50 < 5ms, P99 < 20ms |
| 单流带宽上限 | 10 MB/s |
| 内存预算 | < 2 GB（10K 连接） |
| CPU 使用率 | < 80%（10K 连接，1K 活跃流） |

### 可用性目标

- 服务可用性：99.9%
- 设备重连时间：< 10 秒
- 会话恢复成功率：> 95%
- MTTR：< 5 分钟

### 可观测性

系统提供：

- `/health` 健康检查
- `/metrics` 指标导出（当前 MVP 暂缓）
- 结构化日志
- 审计日志
- OpenTelemetry tracing
- MQTT 遥测发布

---

## 部署与运维

### 单节点部署

首版采用单 Relay 节点，配套：

- MQTT Broker
- Prometheus
- Grafana
- 认证服务

### 推荐端口

- `50051/TCP`：gRPC
- `50052/UDP`：gRPC over QUIC
- `8080/TCP`：健康检查（`/metrics` endpoint 在当前 MVP 暂缓）
- `8883/TCP`：MQTT over TLS

### 配置方式

支持 YAML 配置文件，主要包含：

- Relay 地址与连接参数
- 心跳与超时配置
- 限流配置
- TLS 配置
- MQTT 配置
- 会话恢复配置
- 观测性配置
- 告警配置

### 运维能力

- 启停服务
- 滚动更新
- 查看日志
- 查看指标
- 故障排查
- 证书轮换
- Token 撤销

---

## 测试策略

### 单元测试

覆盖：

- 认证与授权
- 序列号去重
- 会话管理
- 限流
- 错误处理

### 集成测试

覆盖：

1. 设备连接和注册
2. Controller 发起会话
3. 双向数据传输
4. 设备重连和会话恢复
5. MQTT 通知与查询
6. 认证失败处理
7. 授权拒绝处理
8. 限流触发

### 性能测试

- 10K 并发连接
- 1K 并发活跃流
- 延迟目标验证
- 长时间稳定性验证

### 安全测试

- 未认证访问
- 伪造 Token
- 跨设备越权
- DDoS 模拟
- 大 payload 攻击
- 重放攻击

---

## MVP 范围与路线图

### v1.0 MVP

首版重点交付：

- Device ↔ Relay QUIC 连接
- Controller ↔ Relay HTTP/2 连接
- 双向流中继
- 注册、心跳、重连、离线
- MQTT 上下线通知
- Controller 查询在线设备
- RBAC 授权
- 幂等性
- E2E 加密
- 基础限流与输入校验
- 指标、日志、审计
- Relay 遥测
- 健康检查
- Docker / Kubernetes 部署

### 后续版本

- v1.1：会话状态持久化、错误恢复增强
- v1.2：多 Relay 节点、高可用、负载均衡
- v2.0：Controller QUIC、连接迁移、0-RTT、ABAC

---

## 参考资料

- [gRPC 官方文档](https://grpc.io/docs/)
- [QUIC RFC 9000](https://www.rfc-editor.org/rfc/rfc9000.html)
- [MQTT v5.0 规范](https://docs.oasis-open.org/mqtt/mqtt/v5.0/mqtt-v5.0.html)
- [OpenTelemetry 文档](https://opentelemetry.io/docs/)
- [Prometheus 最佳实践](https://prometheus.io/docs/practices/)

---

## 文档说明

本 README 基于以下项目文档整理：

- `doc/requirements.md`
- `doc/architecture.md`
- `doc/action_plan.md`

内容侧重于对外介绍、架构概览和落地路径，适合作为项目入口文档。
