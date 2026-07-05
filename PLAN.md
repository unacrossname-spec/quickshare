# QuickShare — 高性能跨平台文件传输应用

## 项目概述

基于 Rust + Flutter 构建的局域网文件传输工具，目标是在传输性能上显著超越 LocalSend。核心思路：**QUIC 多流并发 + 流水线分片 + 零拷贝数据路径**。

---

## 一、架构总览

```
┌───────────────────────────────────────┐
│           Flutter UI 层                │
│  (设备列表 / 传输队列 / 历史 / 设置)     │
├───────────────────────────────────────┤
│    flutter_rust_bridge (FFI 桥接层)     │
├───────────────────────────────────────┤
│           Rust 传输核心                 │
│  ┌──────────┐ ┌──────────┐ ┌───────┐  │
│  │ QUIC     │ │ mDNS     │ │ File  │  │
│  │ Engine   │ │ Discovery│ │ Mgmt  │  │
│  │(quinn)   │ │(libmdns) │ │       │  │
│  └──────────┘ └──────────┘ └───────┘  │
│  ┌──────────┐ ┌──────────┐            │
│  │ Crypto   │ │ Storage  │            │
│  │(chacha20)│ │(SQLite)  │            │
│  └──────────┘ └──────────┘            │
└───────────────────────────────────────┘
```

---

## 二、核心技术设计

### 2.1 QUIC 传输引擎 (Rust + `quinn` crate)

| 特性 | 实现方式 | 对性能的影响 |
|------|---------|------------|
| 多流并发 | 单个 QUIC 连接内开 N 个独立 stream，同传不同文件分片 | 消除单流瓶颈，利用多核 |
| 0-RTT 握手 | 首次连接 1-RTT，重连 0-RTT 发数据 | 减少连接建立延迟 |
| 无队头阻塞 | 丢一个 stream 不影响其他 stream | 多文件传输更稳定 |
| 内置 TLS 1.3 | QUIC 协议自带加密，无需额外 TLS 封装 | 减少加密层数 |
| 用户态流控 | 自适应调整 stream 并发数 | 动态适配网络条件 |

**连接流程：**
```
设备 A                           设备 B
  |                                |
  |--- mDNS: _quickshare._udp --->|
  |<--- mDNS response ------------|
  |                                |
  |--- QUIC 0-RTT connect ------->|
  |<--- QUIC accept --------------|
  |                                |
  |[Stream 1: control/metadata]   |
  |[Stream 2: chunk 1]            |
  |[Stream 3: chunk 2]            |
  |[Stream N: chunk N]           |
```

### 2.2 文件分片与流水线

- **分片大小自适应**：根据当前 RTT 和历史吞吐量动态调整（4MB ~ 16MB）
- **流水线深度**：保持 4-8 个 in-flight 分片
- **分片独立校验**：丢包/损坏只重传对应分片，而非整个文件

流水线：
```
读块ₙ → SHA256校验 | 加密ₙ₊₁ | 发送ₙ₊₂ 同时进行
```

### 2.3 断点续传

- 传输前：控制流协商已接收的分片列表
- 传输中：收到一分片写入 SQLite 记录
- 传输完成：合并分片、校验全文件哈希、清理临时记录

### 2.4 加密方案

| 层 | 协议 | 密钥 |
|----|------|------|
| 传输层 | QUIC 内置 TLS 1.3 | 自动协商 |
| 文件层 | ChaCha20-Poly1305 per chunk | X25519 ECDH 派生 |

### 2.5 设备发现

- **mDNS** (RFC 6762)：服务类型 `_quickshare._udp.local`
- 发现信息：设备名、设备 ID、IP 地址、端口
- 证书交换在 QUIC 连接阶段完成

### 2.6 持久存储

| 表 | 主要字段 | 用途 |
|----|---------|------|
| `transfers` | id, peer, file_name, size, status, progress | 传输记录 |
| `chunks` | transfer_id, index, size, hash, received | 分片进度（续传） |
| `peers` | id, name, public_key, last_seen | 已知设备缓存 |

---

## 三、项目结构

```
quickshare/
├── Cargo.toml                       # 工作空间
├── libs/
│   └── core/                        # Rust 传输核心
│       ├── Cargo.toml
│       └── src/
│           ├── lib.rs               # 公共 API 导出
│           ├── types.rs             # 共享数据类型
│           ├── transport/            # QUIC 连接管理
│           │   ├── mod.rs
│           │   ├── connection.rs
│           │   ├── stream.rs
│           │   └── config.rs
│           ├── discovery/            # mDNS 发现
│           │   ├── mod.rs
│           │   └── mdns.rs
│           ├── transfer/             # 文件传输
│           │   ├── mod.rs
│           │   ├── sender.rs
│           │   ├── receiver.rs
│           │   ├── chunk.rs
│           │   └── resume.rs
│           ├── crypto/               # 加密
│           │   ├── mod.rs
│           │   └── keyx.rs
│           └── storage/              # SQLite
│               ├── mod.rs
│               ├── db.rs
│               └── models.rs
├── app/                              # Flutter 应用
│   ├── pubspec.yaml
│   ├── lib/
│   │   ├── main.dart
│   │   ├── src/
│   │   │   ├── models/                # Dart 数据模型
│   │   │   │   ├── device.dart
│   │   │   │   ├── transfer.dart
│   │   │   │   └── history.dart
│   │   │   ├── pages/                 # 页面
│   │   │   │   ├── home_page.dart
│   │   │   │   ├── transfer_page.dart
│   │   │   │   ├── history_page.dart
│   │   │   │   └── settings_page.dart
│   │   │   ├── widgets/               # 可复用组件
│   │   │   │   ├── device_card.dart
│   │   │   │   ├── transfer_tile.dart
│   │   │   │   └── progress_indicator.dart
│   │   │   ├── bridge/                # Rust FFI 调用层
│   │   │   │   └── core_bridge.dart
│   │   │   └── providers/             # 状态管理
│   │   │       ├── device_provider.dart
│   │   │       ├── transfer_provider.dart
│   │   │       └── settings_provider.dart
│   │   └── generated/                 # flutter_rust_bridge 生成
│   ├── android/
│   ├── ios/
│   ├── linux/
│   ├── macos/
│   └── windows/
```

---

## 四、开发阶段划分

### Phase 1：Rust 核心库

| # | 内容 | 关键依赖 |
|---|------|---------|
| 1 | 项目骨架 + Cargo 工作空间 + 基础类型 | - |
| 2 | QUIC 连接管理（client/server、证书、0-RTT） | `quinn` + `rcgen` |
| 3 | 数据流读写（双向流、并发控制、流控） | `quinn` + `tokio` |
| 4 | 文件分片（分片→流水线发送→接收→重组） | `sha2` |
| 5 | 断点续传（分片追踪、协商、跳过已接收） | `rusqlite` |
| 6 | mDNS 设备发现（注册/监听/解析） | `libmdns` |
| 7 | 加密（X25519 + ChaCha20-Poly1305） | `x25519-dalek` + `chacha20` |
| 8 | 集成测试（mock mDNS + 本地回环 QUIC 传输） | - |

### Phase 2：Flutter UI + FFI 桥接

| # | 内容 | 关键机制 |
|---|------|---------|
| 1 | Flutter 项目创建 + 依赖 | `flutter_rust_bridge` |
| 2 | 生成 Dart FFI 绑定 | codegen |
| 3 | Provider 状态管理（设备/传输/设置） | `provider` |
| 4 | 主页面：设备列表 + 拖拽/选择文件 | - |
| 5 | 传输页面：发送/接收队列、实时进度、速度曲线 | Rust→Dart stream |
| 6 | 历史页面：传输记录列表 | SQLite 查询 |
| 7 | 设置页面：设备名、存储路径、加密开关 | `shared_preferences` |
| 8 | 移动端适配：权限、后台传输 | `permission_handler` |

### Phase 3：集成测试 + 性能调优

| # | 内容 |
|---|------|
| 1 | 端到端传输测试（各种大小/类型的文件） |
| 2 | 性能基准：对比 LocalSend |
| 3 | 边界测试：>10GB 大文件、大量小文件、弱网、断线恢复 |
| 4 | 崩溃和错误处理打磨 |

---

## 五、预期性能目标

| 场景 | LocalSend | QuickShare |
|------|-----------|-----------|
| 千兆以太网 1GB 文件 | ~400-600 Mbps | ≥900 Mbps |
| Wi-Fi 6 (866 Mbps) | ~200-400 Mbps | ≥700 Mbps |
| 手机 ↔ 笔记本 | ~100-300 Mbps | ≥500 Mbps |

---

## 六、外部依赖

### Rust

```toml
quinn = "0.11"          # QUIC 协议
tokio = { version = "1", features = ["full"] }
rustls = "0.23"         # QUIC 的 TLS 后端
rcgen = "0.13"          # 自签名证书
libmdns = "0.9"         # mDNS
sha2 = "0.10"           # 哈希
chacha20 = "0.9"        # 加密
poly1305 = "0.8"
x25519-dalek = "2"      # 密钥交换
rusqlite = { version = "0.31", features = ["bundled"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
tracing = "0.1"
```

### Flutter

```yaml
flutter_rust_bridge: ^2.0     # Rust FFI 桥接
provider: ^6.0               # 状态管理
file_picker: ^8.0            # 文件选择
path_provider: ^2.0          # 存储路径
shared_preferences: ^2.0     # 设置存储
permission_handler: ^11.0    # 权限管理
```

---

## 七、Rust 核心 API

```rust
// 生命周期
pub fn start_runtime(config: AppConfig) -> Result<()>;
pub fn shutdown();

// 设备发现
pub fn start_discovery() -> Stream<DiscoveredDevice>;
pub fn stop_discovery();

// 文件传输
pub fn send_file(peer_id: DeviceId, path: PathBuf) -> TransferId;
pub fn cancel_transfer(id: TransferId);
pub fn pause_transfer(id: TransferId);
pub fn resume_transfer(id: TransferId);

// 传输事件流（Dart Stream 监听）
pub fn transfer_events() -> Stream<TransferEvent>;
// events: Progress, Completed, Failed, Paused

// 历史
pub fn get_history(limit: u32, offset: u32) -> Vec<TransferRecord>;

// 设置
pub fn set_storage_path(path: String);
pub fn set_encryption(enabled: bool);
pub fn set_max_streams(n: u32);
```

---

## 八、验证方法

1. **单元测试**：每个 Rust 模块的 `#[cfg(test)]`（分片、加密、续传状态机）
2. **集成测试**：localhost 两个 QUIC endpoint 互传
3. **端到端**：物理设备或容器间实际传输
4. **性能基准**：iperf3 测底噪 → QuickShare vs LocalSend 对比
5. **Flutter 测试**：integration test 覆盖核心交互路径

---

## 九、文件清单

### 新建文件（按实现顺序）

| # | 路径 | 说明 |
|---|------|------|
| 1 | `quickshare/Cargo.toml` | 工作空间定义 |
| 2 | `quickshare/libs/core/Cargo.toml` | 核心库依赖 |
| 3 | `quickshare/libs/core/src/lib.rs` | 模块声明 + 公共 API |
| 4 | `quickshare/libs/core/src/types.rs` | DeviceId, TransferId, TransferEvent 等 |
| 5 | `quickshare/libs/core/src/transport/mod.rs` | 传输模块 |
| 6 | `quickshare/libs/core/src/transport/config.rs` | QUIC 参数 |
| 7 | `quickshare/libs/core/src/transport/connection.rs` | 连接管理 |
| 8 | `quickshare/libs/core/src/transport/stream.rs` | 数据流读写 |
| 9 | `quickshare/libs/core/src/discovery/mod.rs` | 发现模块 |
| 10 | `quickshare/libs/core/src/discovery/mdns.rs` | mDNS 实现 |
| 11 | `quickshare/libs/core/src/transfer/mod.rs` | 传输模块 |
| 12 | `quickshare/libs/core/src/transfer/chunk.rs` | 分片逻辑 |
| 13 | `quickshare/libs/core/src/transfer/sender.rs` | 发送端 |
| 14 | `quickshare/libs/core/src/transfer/receiver.rs` | 接收端 |
| 15 | `quickshare/libs/core/src/transfer/resume.rs` | 续传逻辑 |
| 16 | `quickshare/libs/core/src/crypto/mod.rs` | 加密模块 |
| 17 | `quickshare/libs/core/src/crypto/keyx.rs` | 密钥交换 |
| 18 | `quickshare/libs/core/src/storage/mod.rs` | 存储模块 |
| 19 | `quickshare/libs/core/src/storage/db.rs` | 数据库初始化 |
| 20 | `quickshare/libs/core/src/storage/models.rs` | 数据模型 |
