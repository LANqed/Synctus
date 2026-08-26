# Synctus

**两个人互相督促学习和工作。**

看得见对方在不在真的干活：现在专注了多久、目标还差多少、连了几天、有没有在番茄钟
里偷偷开 B 站。支持 **Windows / Linux / Android** 三端，内容全程端到端加密，
中继服务器读不到任何状态。

```
┌─────────────┐        密文        ┌──────────────┐        密文        ┌─────────────┐
│  你的电脑    │ ◀───────────────▶ │ synctus-server│ ◀───────────────▶ │  TA 的手机   │
│  悬浮窗+托盘 │                   │  只转发，不解密│                   │  通知栏保活  │
└─────────────┘                    └──────────────┘                    └─────────────┘
```

## 它凭什么能督促

| | 怎么起作用 |
| --- | --- |
| **今日专注对比** | 悬浮窗和通知栏常驻显示「我 50 / TA 75 分钟」。看到这行数字比任何提醒都有用 |
| **每日目标 + 连续天数** | 默认 100 分钟（约四个番茄钟）。达标自动告诉对方，连续达标累计 🔥 天数 |
| **摸鱼检测** | 专注回合中开了摸鱼应用超过宽限时间就提醒你；可选同时告诉对方 |
| **「别摸鱼了」** | 对方在专注却挂着 B 站时，按钮自动出现并带上证据。允许穿透免打扰 |
| **「一起专注」** | 对方空闲时会自动跟着开一轮，两个番茄钟对齐 |
| **自动夸奖** | 对方达标时自动祝贺——靠人记得发的鼓励不会发生，所以让它自动 |

督促之外的部分：前台应用、电量、正在播放的音乐、是否休息、待办清单同步，
电脑端托盘与悬浮窗，手机端通知栏保活，开机自启，GitHub 更新检查。

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

### 3. 设定目标与摸鱼清单

设置 → **督促**：

- **每日目标**：默认 100 分钟。设成 0 就完全关闭目标与连续天数，只同步状态。
- **摸鱼应用清单**：默认给了 bilibili / steam / youtube 之类，**请按自己的情况改**。
  匹配进程名或包名的一部分，不区分大小写。
- **宽限时间**：默认 30 秒。切过去查资料不算摸鱼，只有停留超过这个时间才提醒。
- **顺便告诉对方**：默认**关闭**。被盯着应该是自己选的，不是工具默认的。

### 4. 桌面端

```bash
./synctus              # 正常启动
./synctus --minimised  # 隐藏悬浮窗启动（自启用）
```

- 悬浮窗常驻显示两人今日专注进度条与分钟数
- **左键点对方头像** → 敲一敲；对方摸鱼时会多出「👀 抓到了」按钮
- **右键悬浮窗 / 托盘** → 督促、番茄钟、设置、检查更新
- 托盘提示（悬浮窗隐藏时）也带今日对比
- 关闭悬浮窗不会退出程序，托盘仍在运行

Linux 需要系统托盘支持（GNOME 用户需装 AppIndicator 扩展）。
没有托盘时程序仍可用，只是只有悬浮窗。

### 5. 手机端

安装 APK 后填入配对码。为了拿到完整信息，可选授予：

| 权限 | 换来什么 | 不给的后果 |
| --- | --- | --- |
| 通知权限 | 通知栏显示状态对比 | 同步照常，但看不到通知 |
| 使用情况访问 | 同步前台应用、摸鱼检测 | 不上报前台应用，摸鱼检测失效 |
| 通知访问 | 同步正在播放的音乐 | 不上报音乐 |
| 取消电池优化 | 后台不被杀 | 部分 ROM 会掐掉连接 |

通知栏折叠时显示「今日专注 我 X / TA Y 分钟」和目标进度条；
三个按钮会按情况变化：对方摸鱼时是**别摸鱼了**，对方空闲时是**一起专注**，
另外两个是番茄钟开始/暂停与去休息/回来了。

## 隐私

督促和监控之间只隔一个开关，所以每个开关都写清楚了谁能看到什么：

- **摸鱼提醒默认只给自己**。「顺便告诉对方」要手动打开。
- **只在专注回合中检查前台应用**。休息和空闲时间不监控——监视别人的闲暇时间
  不叫督促。
- **窗口标题默认不上报**。它常含文件名或聊天对象。
- 前台应用、电量、音乐、番茄钟、待办可以逐项关闭，**关掉的项目根本不会离开本机**，
  不是发出去再让对方不显示。
- 应用黑名单：这些应用在前台时只显示「（隐藏）」，而不是完全不上报
  （那样看起来像掉线了）。

专注分钟数、每日目标与连续天数跟「同步番茄钟状态」是同一个开关——
关掉它督促功能就没有数据可用了。

## 加密

```
配对码 ──Argon2id(64MiB, t=3)──▶ 房间根密钥
                                     │ HKDF-SHA256
                    ┌────────────────┼────────────────┐
                 room_id          auth_key         msg_key
              （明文给服务器）    （证明成员身份）  （XChaCha20-Poly1305）
```

服务器看得到：房间标识、设备标识、消息大小和时间。
服务器看不到：状态、应用名、歌名、电量、待办、专注分钟数、互动内容——任何一个字节。

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
./scripts/build-android-libs.sh          # 或 scripts\build-android-libs.ps1
cd android && gradle assembleRelease
```

> 仓库里**没有** Gradle Wrapper（`gradlew` / `gradle-wrapper.jar`）：
> 二进制文件既是审查盲区也是供应链风险。请自行安装 Gradle 8.11+，
> CI 中由 `gradle/actions/setup-gradle` 固定版本安装。

## 项目结构

```
crates/
  core/      协议、加密、番茄钟、目标与连续天数、摸鱼检测、可复用客户端引擎
  server/    中继服务器：房间路由、限速、保留状态
  desktop/   Windows/Linux 客户端：egui 悬浮窗、托盘、传感器
  mobile/    Android 原生库：JSON 命令/事件桥，逻辑与桌面端共享
android/     Kotlin + Compose 前端：前台服务、通知栏、设置
deploy/      server 示例配置、systemd 单元、Dockerfile
docs/        协议与加密设计
```

督促逻辑（`core/store.rs` 的目标与连续天数、`core/focus.rs` 的摸鱼检测）
三端共用一套实现，平台层只负责读取传感器和画界面。

## 发布

版本号只有一处：`Cargo.toml` 的 `[workspace.package] version`。Android 的
`versionName` / `versionCode` 从它推导，CI 会拦住任何把版本号写死回 Gradle 的改动。

**方式一：网页上点一下**

Actions → Release → Run workflow，填入新版本号（如 `0.2.0`）→ Run。
它会自动改 `Cargo.toml`、提交、打标签、构建三端、发布 Release。
本地什么都不用做。

**方式二：本地打标签**

```bash
# 先把 Cargo.toml 的版本改成 0.2.0，提交
git tag v0.2.0 && git push origin v0.2.0
```

标签与 `Cargo.toml` 不一致时会**直接失败**——否则构建出的程序会报告一个
与所在 Release 不同的版本号，客户端的更新检查会永久提示有新版。

产物：`synctus-windows-x86_64.zip`、`synctus-linux-x86_64.tar.gz`、
`synctus-android.apk`，附 `SHA256SUMS.txt`。Release 说明里自动列出自上个
版本以来的提交。

发布前会核对三件事：Windows/Linux 二进制的 `--version`、APK manifest 里的
`versionName`、以及产物是否真的存在（缺任何一个就失败，而不是发一个空 Release）。

APK 签名是可选的：配置了下面这些 secrets 才签名，否则产出
`synctus-android-unsigned.apk` 并在 Release 说明里注明。

| Secret | 内容 |
| --- | --- |
| `ANDROID_KEYSTORE_BASE64` | keystore 文件的 base64（`base64 -w0 release.jks`） |
| `ANDROID_KEYSTORE_PASSWORD` | keystore 密码 |
| `ANDROID_KEY_ALIAS` | 密钥别名 |
| `ANDROID_KEY_PASSWORD` | 密钥密码 |

预发布版本勾选 `prerelease` 即可——客户端的更新检查会忽略预发布，
所以可以先发一版给自己试。

## 许可

MIT
