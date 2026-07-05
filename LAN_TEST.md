## LAN 传输测试步骤

### 本机信息
- IP: **192.168.0.103**
- Hostname: `spectrum`
- Binary: `target/release/quickshare-cli` (7.4 MB, Linux x86_64)

### 步骤

**1. 把 binary 传到另一台机器**

```bash
# 方法一：scp（需要对方 ssh 账号）
scp target/release/quickshare-cli user@另一台IP:/tmp/

# 方法二：本机起 HTTP 服务，对方 wget（不需账号）
cd /home/spectrum/my_projects/quickshare/target/release/
python3 -m http.server 8080
# 在另一台机器上：wget http://192.168.0.103:8080/quickshare-cli && chmod +x quickshare-cli
```

**2. 在接收方启动 server**

```bash
# 接收方（保存到 Downloads 目录）
/tmp/quickshare-cli serve --port 8877 --save ~/Downloads
```

**3. 发送方建造测试文件并发起传输**

```bash
# 发送方建 256MB 测试文件
dd if=/dev/zero of=/tmp/test_256m.bin bs=1M count=256

# 发送
/tmp/quickshare-cli send "192.168.0.103:8877" /tmp/test_256m.bin
```

**4. 观察结果**

工具会自动打印传输速度：
```
[send] Done! 256 MB in 2.15s = 998 Mbps
[receive] Done! 256 MB in 2.17s = 990 Mbps
```

### 场景建议

| 场景 | 对方 | 方法 |
|------|------|------|
| 另一台 Linux PC | SSH 通 | `scp` 传 binary |
| 笔记本 / 手机 | 同局域网无 SSH | Python HTTP server |
| macOS | 同局域网 | `python3 -m http.server` 同理 |
