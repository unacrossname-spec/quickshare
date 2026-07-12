# QuickShare

<p align="center">
  <img src="src-tauri/icons/128x128.png" alt="QuickShare" width="128" height="128"/>
</p>

<p align="center">
  <strong>极简 · 高速 · 安全的局域网文件分享工具</strong>
</p>

<p align="center">
  <a href="https://github.com/unacrossname-spec/quickshare/releases"><img src="https://img.shields.io/github/v/release/unacrossname-spec/quickshare?color=%23006875" alt="Release"/></a>
  <img src="https://img.shields.io/badge/platform-Windows%20%7C%20Linux-%23006875" alt="Platform"/>
  <img src="https://img.shields.io/badge/license-MIT-%23006875" alt="License"/>
</p>

---

## 简介

QuickShare 是一款基于 **Tauri v2** 的跨平台局域网文件分享桌面应用。无需互联网、无需注册、无需配置服务器——同一局域网内的设备互相发现，拖拽文件即可高速传输。

### 核心特性

- 🔍 **自动设备发现** — UDP 广播自动扫描局域网内其他 QuickShare 设备
- ⚡ **流式传输** — 4MB 分块流式读写，支持超大文件（几百 GB）不占满内存
- 🗜️ **LZ4 压缩** — 每块独立压缩，压缩率与速度兼得
- 🔐 **AES-256-GCM 加密** — 预共享密码，端到端加密，密码永不进入网络
- 📦 **目录打包** — 文件夹自动打包传输，保留目录结构
- 📊 **实时速度显示** — 指数移动平均速度，支持多种单位切换
- 📋 **历史记录** — 传输历史自动保存，含速度、hash 校验
- 🎨 **Material Design 3** — 现代简洁 UI，响应式布局

### 技术栈

| 层 | 技术 |
|---|------|
| 桌面框架 | Tauri v2 (Rust + WebView) |
| 前端 | Vanilla JS + Tailwind CSS + Material Symbols |
| 传输协议 | 自定义二进制协议 (56 字节头 + JSON 握手) |
| 压缩 | LZ4 (每块独立压缩) |
| 加密 | AES-256-GCM (SHA-256 密钥派生) |
| 完整校验 | BLAKE3 (每块 + 全文件) |
| 发现 | UDP 广播 (端口 8879) |

### 代码规模

```
src-tauri/src/lib.rs     1690 行  ← Rust 后端 (所有命令、协议、加密)
src/js/app.js             933 行  ← 前端逻辑 (零框架)
src/index.html            311 行  ← UI 结构
libs/core/src/            338 行  ← 核心库 (类型、协议、压缩、加密)
─────────────────────────────────
总计                     3272 行
```

---

## 安装

### 下载预构建版本

从 [Releases](https://github.com/unacrossname-spec/quickshare/releases) 页面下载对应平台的可执行文件：

| 平台 | 文件 |
|------|------|
| Windows x64 | `quickshare-windows-x64.exe` |
| Linux x64 | `quickshare-linux-x64` |

Linux 版本运行前请确保安装依赖：

```bash
sudo apt install libwebkit2gtk-4.1-0 libgtk-3-0 libsoup-3.0-0
```

### 从源码构建

**前置条件：**

- Rust 1.70+
- Linux: `libwebkit2gtk-4.1-dev libgtk-3-dev libayatana-appindicator3-dev librsvg2-dev libsoup-3.0-dev libjavascriptcoregtk-4.1-dev`

```bash
git clone https://github.com/unacrossname-spec/quickshare.git
cd quickshare
cargo build --release --manifest-path src-tauri/Cargo.toml
```

构建产物位于 `src-tauri/target/release/quickshare`。

---

## 使用指南

### 基本流程

1. **启动** — 两台设备在同一局域网下打开 QuickShare
2. **发现** — 自动扫描，点击目标设备或手动输入 IP
3. **选择文件** — 拖拽到窗口或点击传输页面上传区域
4. **确认接收** — 接收方弹出确认对话框，可选择接收或拒绝
5. **传输** — 实时显示进度和速度

### 加密传输

1. 进入**设置** → 打开「传输加密」
2. 两端设置**相同的密码**，点击「保存」
3. 发送文件时，数据逐块 AES-256-GCM 加密
4. 接收方自动使用本地密码解密
5. 密码不匹配 → 解密失败，传输中止

> ⚠️ **安全说明**：密码仅存储在本地磁盘，**永不通过网络传输**。SHA-256 派生为 256-bit 密钥，每块使用随机 12 字节 nonce。

### 传输速度单位

在设置中可选择：
- B/s、KB/s、MB/s、GB/s、Mbps、Gbps

### 自定义端口

默认通信端口为 **8877**（TCP）+ **8879**（UDP 发现）。可在设置中修改，服务器自动重启。

---

## 架构

### 传输协议

```
┌─────────────────────────────────────────────┐
│                握手阶段 (JSON)                │
│  Sender ──TransferRequest──▶ Receiver        │
│  Sender ◀──TransferAccept─── Receiver        │
│  Sender ◀──TransferReject─── Receiver        │
├─────────────────────────────────────────────┤
│              数据传输 (二进制)                │
│  ┌──────┬──────────────────────────────────┐ │
│  │ 56 B │           chunk data             │ │
│  │ 头部 │         (最大 8 MiB)             │ │
│  └──────┴──────────────────────────────────┘ │
│  头部: type(4) + index(8) + offset(8)        │
│        + size(4) + hash(32)                  │
└─────────────────────────────────────────────┘
```

### 数据流

```
发送:  文件 ──▶ ChunkReader(4MB) ──▶ LZ4压缩 ──▶ AES加密 ──▶ BLAKE3 ──▶ TCP

接收:  TCP ──▶ BLAKE3校验 ──▶ AES解密 ──▶ LZ4解压 ──▶ 写入文件
```

### 加密格式（每块）

```
┌────────────────┬─────────────────────────────┐
│  12 B nonce    │  AES-256-GCM 密文 + 16 B tag │
│  (随机明文)     │  (认证加密)                   │
└────────────────┴─────────────────────────────┘
```

### 项目结构

```
quickshare/
├── src/                          # 前端 (WebView)
│   ├── index.html                # 单页 UI
│   ├── js/app.js                 # 全部前端逻辑
│   ├── styles/app.css            # 自定义样式
│   └── vendor/                   # Tailwind CDN + 字体
├── src-tauri/                    # Tauri 应用
│   ├── src/
│   │   ├── main.rs               # 入口
│   │   └── lib.rs                # 后端 (所有命令、服务、协议)
│   ├── tauri.conf.json           # Tauri 配置
│   ├── capabilities/default.json # 权限声明
│   └── Cargo.toml
├── libs/core/                    # 核心库 (协议无关)
│   ├── src/
│   │   ├── types.rs              # 共享类型
│   │   ├── crypto.rs             # AES-256-GCM
│   │   ├── compress.rs           # LZ4 压缩
│   │   ├── bundle.rs             # 文件打包
│   │   └── transfer/             # 传输协议
│   │       ├── chunk.rs          # 分块读写
│   │       ├── sender.rs         # 发送端协议
│   │       ├── receiver.rs       # 接收端协议
│   │       └── batch.rs          # 批量传输
│   └── Cargo.toml
├── cli/                          # 命令行工具 (独立)
│   ├── src/main.rs
│   └── Cargo.toml
└── .github/workflows/release.yml # CI/CD
```

---

## 构建发布

推送 tag 自动触发 GitHub Actions 构建 Windows + Linux 二进制文件：

```bash
git tag -a v0.x.x -m "v0.x.x: description"
git push origin v0.x.x
```

构建产物自动发布到 GitHub Releases。

---

## 版本历史

| 版本 | 日期 | 主要变更 |
|------|------|---------|
| v0.4.2 | 2026-07 | 代码审查修复：CSP 安全头、全文件 hash 校验、加密覆盖打包传输、端口热重启、chunk 大小上限 |
| v0.4.1 | 2026-07 | 修复 Linux 加密开关卡死、transitionend 优雅方案、debounce 磁盘写入 |
| v0.4.0 | 2026-07 | AES-256-GCM 加密、预共享密码、流式传输重连 |
| v0.3.0 | 2026-07 | 流式传输（connect-first 架构）、实时速度显示、LZ4 每块压缩 |
| v0.2.x | 2026-07 | 修复 TCP 连接超时、握手确认、接收确认对话框、Windows 发现兼容 |
| v0.1.0 | 2026-06 | 初始版本：设备发现、手动连接、文件传输 |

---

## 许可

MIT License
