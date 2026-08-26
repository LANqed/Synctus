# Synctus

和另一个人同步彼此的状态：TA 在用什么应用、在听什么歌、电量还剩多少、
是不是在休息、番茄钟走到哪了。支持 **Windows / Linux / Android** 三端，
内容全程端到端加密，中继服务器读不到任何状态。

```
┌─────────────┐        密文        ┌──────────────┐        密文        ┌─────────────┐
│  你的电脑    │ ◀───────────────▶ │ synctus-server│ ◀───────────────▶ │  TA 的手机   │
│  悬浮窗+托盘 │                   │  只转发，不解密│                   │  通知栏保活  │
└─────────────┘                    └──────────────┘                    └─────────────┘
```

## 功能

| | 说明 |
| --- | --- |
| **状态同步** | 前台应用、电量、正在播放的音乐、是否休息、番茄钟阶段 |
| **展示方式** | 电脑端常驻悬浮窗（左键敲对方、右键菜单）；手机端常驻通知栏 |
| **番茄钟** | 只同步截止时间戳，休眠/息屏后依然准确，双方互相可见 |
| **待办清单** | 本地管理，可选同步给对方 |
| **互动** | 敲一敲 / 抱抱 / 请喝咖啡 / 去休息 / 一起专注，对方立刻收到提醒 |
| **托盘** | 电脑端右键设置状态、番茄钟、隐私开关、检查更新 |
| **保活** | 手机端前台服务 + 通知栏；划掉任务后自动拉回 |
| **加密** | Argon2id + HKDF + XChaCha20-Poly1305，外层再套 TLS |
| **占用** | 桌面端 ~7.7 MB、服务器 ~1.7 MB；空闲时几乎不耗 CPU |
| **自启** | Windows 注册表 / Linux XDG autostart / Android 开机广播 |
| **更新** | 通过 GitHub Releases 检查新版本 |

## 快速开始

### 1. 部署中继服务器

需要一台双方都能访问的机器。服务器**不持有任何密钥**，只是转发密文。

```bash
# 从 Release 下载，或自行编译
cargo build --release -p synctus-server

# 直接跑（明文，仅限内网测试）
./synctus-server

# 生产环境：配置 TLS 证书
./synctus-server --config server.toml
```

`server.toml` 参考 [`deploy/server.example.toml`](deploy/server.example.toml)。
也可以用环境变量：

```bash
SYNCTUS_BIND=0.0.0.0:8787 \
SYNCTUS_CERT=/etc/letsencrypt/live/example.com/fullchain.pem \
SYNCTUS_KEY=/etc/letsencrypt/live/example.com/privkey.pem \
./synctus-server
```

Docker 与 systemd 单元见 [`deploy/`](deploy/)。

> **关于 TLS**：不配 TLS 时消息内容仍然是端到端加密的，服务器和中间人都读不到；
> 但房间标识与设备标识会以明文经过网络。公网部署请配上证书，
> 或放在 Nginx / Caddy 后面。

### 2. 配对

打开任意一端 → 设置 → 点「生成」得到配对码，例如 `K7QM-3XVP-9RTB-2HJW`。

**双方三端都填入同一个配对码**，以及同一个服务器地址。没有账号，没有注册。

> 配对码就是密钥。通过一个安全渠道告诉对方，不要发在公开的地方。

### 3. 桌面端

```bash
./synctus              # 正常启动
./synctus --minimised  # 隐藏悬浮窗启动（自启用）
```

- **左键点对方头像** → 敲一敲
- **右键悬浮窗 / 托盘** → 状态、番茄钟、设置、检查更新
- 拖动悬浮窗可移动，位置自动记住
- 关闭悬浮窗不会退出程序，托盘仍在运行

Linux 需要系统托盘支持（GNOME 用户需装 AppIndicator 扩展）。
没有托盘时程序仍可用，只是只有悬浮窗。

### 4. 手机端

安装 APK 后填入配对码。为了拿到完整信息，可选授予：

| 权限 | 换来什么 | 不给的后果 |
| --- | --- | --- |
| 通知权限 | 通知栏显示对方状态 | 同步照常，但看不到通知 |
| 使用情况访问 | 同步前台应用 | 不上报前台应用 |
| 通知访问 | 同步正在播放的音乐 | 不上报音乐 |
| 取消电池优化 | 后台不被杀 | 部分 ROM 会掐掉连接 |

通知栏三个按钮：**敲一敲**、**去休息 / 回来了**、**开始专注 / 暂停**。

## 隐私

默认不上报的：**窗口标题**。它常含文件名或聊天对象，需要手动打开。

设置里可以逐项关闭前台应用、电量、音乐、番茄钟、待办的同步——
**关掉的项目根本不会离开本机**，不是发出去再让对方不显示。

还可以设置应用黑名单：这些应用在前台时只显示「（隐藏）」，
而不是完全不上报（那样看起来像掉线了）。

## 加密

```
配对码 ──Argon2id(64MiB, t=3)──▶ 房间根密钥
                                     │ HKDF-SHA256
                    ┌────────────────┼────────────────┐
                 room_id          auth_key         msg_key
              （明文给服务器）    （证明成员身份）  （XChaCha20-Poly1305）
```

服务器看得到：房间标识、设备标识、消息大小和时间。
服务器看不到：状态、应用名、歌名、电量、待办、互动内容——任何一个字节。

完整设计、威胁模型（**包括做不到的部分**）与线路格式见
[`docs/PROTOCOL.md`](docs/PROTOCOL.md)。

## 从源码构建

```bash
# 桌面端 + 服务器
cargo build --release -p synctus-desktop -p synctus-server

# 全部测试
cargo test --workspace
```

Linux 构建依赖：

```bash
sudo apt install -y libgtk-3-dev libxdo-dev libayatana-appindicator3-dev \
                    libdbus-1-dev libx11-dev libxcb1-dev libxkbcommon-dev \
                    libwayland-dev pkg-config
```

Android：

```bash
cargo install cargo-ndk
cargo ndk -t arm64-v8a -t x86_64 -o android/app/src/main/jniLibs \
    build --release -p synctus-mobile
cd android && ./gradlew assembleRelease
```

## 项目结构

```
crates/
  core/      协议、加密、番茄钟、配置、可复用的客户端引擎（无 UI、无平台代码）
  server/    中继服务器：房间路由、限速、保留状态
  desktop/   Windows/Linux 客户端：egui 悬浮窗、托盘、传感器
  mobile/    Android 原生库：JSON 命令/事件桥，逻辑与桌面端共享
android/     Kotlin + Compose 前端：前台服务、通知栏、设置
deploy/      server 示例配置、systemd 单元、Dockerfile
docs/        协议与加密设计
```

三端共用 `core` 里同一套状态模型、加密实现和番茄钟状态机，
平台层只负责读取传感器和画界面。

## 发布

推一个 `v*` 标签，GitHub Actions 会构建并发布三端产物：

```bash
git tag v0.1.0 && git push origin v0.1.0
```

产物：`synctus-windows-x86_64.zip`、`synctus-linux-x86_64.tar.gz`、
`synctus-android.apk`，附 `SHA256SUMS.txt`。

APK 签名是可选的：配置了 `ANDROID_KEYSTORE_BASE64` 等 secrets 才签名，
否则产出未签名 APK（两人自用场景够了，手动安装即可）。

## 许可

MIT
