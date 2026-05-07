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
**目标**：实现 stationService 与 Relay 的长连接、注册、心跳、断线重连、会话恢复。

**交付物**
- DeviceConnect 流程
- Register / Heartbeat / Disconnect 逻辑
- QUIC 连接与 TCP fallback
- 心跳超时、离线判定、重连退避
- session / connection_id 管理

**依赖**
- 协议与接口定义
- 基础工程与项目骨架

---

### 4) Controller 接入链路
**目标**：实现 Controller 到 Relay 的认证、在线设备查询、会话建立。

**交付物**
- JWT 鉴权
- ListOnlineDevices RPC
- ConnectToDevice 流程
- 设备在线状态查询与过滤
- Controller 侧错误码处理

**依赖**
- 协议与接口定义
- 认证与授权基础能力

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

### 6) 认证、授权与安全
**目标**：把系统的安全边界建立起来，满足需求中的控制中继模式。

**交付物**
- Device 认证：mTLS / Token
- Controller 认证：JWT
- RBAC 授权模型
- device_id / controller_id / method_name 校验
- E2E payload 透明转发，不解密内容
- 黑名单 / token revocation 支持的接口预留

**依赖**
- 协议定义
- 基础工程
- Controller / Device 接入链路

---

### 7) MQTT 服务发现与遥测
**目标**：实现设备上下线通知、设备自报状态、Relay 遥测发布。

**交付物**
- relay/device/online
- relay/device/offline
- device/{device_id}/status
- telemetry/{device_id}
- telemetry/relay/{relay_id}
- MQTT 客户端重连与降级策略
- Controller 侧订阅/验证逻辑（如果项目包含）

**依赖**
- 设备注册/离线流程
- 基础工程与配置体系

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
