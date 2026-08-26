#!/bin/sh
# Synctus 中继服务器一键安装
#
#   curl -fsSL https://raw.githubusercontent.com/LANqed/Synctus/main/deploy/install.sh | sh
#
# Written for /bin/sh, not bash: Alpine ships busybox ash by default, and an
# installer that needs bash installed first is not one-command.
#
# What it does, in order, so nothing is a surprise:
#   1. Detects the CPU architecture and the C library.
#   2. Downloads the matching static binary from GitHub Releases.
#   3. Verifies its SHA-256 against the published checksum file.
#   4. Installs a service (systemd or OpenRC) and a default config.
#   5. Starts it and prints what to type into the clients.
#
# Re-running upgrades in place, keeping the existing config.

set -eu

REPO="${SYNCTUS_REPO:-LANqed/Synctus}"
VERSION="${SYNCTUS_VERSION:-latest}"
PREFIX="${SYNCTUS_PREFIX:-/usr/local/bin}"
CONFIG_DIR="${SYNCTUS_CONFIG_DIR:-/etc/synctus}"
CONFIG="$CONFIG_DIR/server.toml"
PORT="${SYNCTUS_PORT:-8787}"
SERVICE_USER="synctus"

# ------------------------------------------------------------------ output

# Colour only when stdout is a terminal: piped into a log, escape codes are noise.
if [ -t 1 ]; then
    B='\033[1m'; R='\033[31m'; G='\033[32m'; Y='\033[33m'; N='\033[0m'
else
    B=''; R=''; G=''; Y=''; N=''
fi

say()  { printf '%b\n' "$*"; }
step() { printf '%b\n' "${B}==>${N} $*"; }
warn() { printf '%b\n' "${Y}警告:${N} $*" >&2; }
die()  { printf '%b\n' "${R}错误:${N} $*" >&2; exit 1; }

# ------------------------------------------------------------------ checks

[ "$(id -u)" = "0" ] || die "需要 root 权限。请用: curl -fsSL <url> | sudo sh"

# One of these is needed to download; both are absent only on a very bare system.
if command -v curl >/dev/null 2>&1; then
    DOWNLOAD="curl -fsSL"
    DOWNLOAD_TO="curl -fsSL -o"
elif command -v wget >/dev/null 2>&1; then
    DOWNLOAD="wget -qO-"
    DOWNLOAD_TO="wget -qO"
else
    die "需要 curl 或 wget，请先安装其中之一"
fi

# ------------------------------------------------------------------ detect

step "检测系统"

ARCH="$(uname -m)"
case "$ARCH" in
    x86_64 | amd64) ARCH=x86_64 ;;
    aarch64 | arm64) ARCH=aarch64 ;;
    *) die "不支持的架构: $ARCH（目前提供 x86_64 与 aarch64）" ;;
esac

# The musl build is static and runs anywhere, including glibc systems. It is the
# default for exactly that reason: one fewer thing that can go wrong. The glibc
# build exists for anyone who prefers dynamic linking against the system libc.
LIBC=musl
if [ "${SYNCTUS_LIBC:-}" = "gnu" ]; then
    LIBC=gnu
fi

OS_NAME="$(. /etc/os-release 2>/dev/null && echo "${PRETTY_NAME:-$ID}" || echo unknown)"
say "  系统    $OS_NAME"
say "  架构    $ARCH"
say "  构建    $LIBC（静态链接，无运行时依赖）"

# Init system. Checked by what is actually running, not by what is installed:
# a container may have systemd files present with something else as PID 1.
INIT=none
if [ -d /run/systemd/system ]; then
    INIT=systemd
elif command -v rc-update >/dev/null 2>&1; then
    INIT=openrc
fi
say "  服务    $(case $INIT in systemd) echo systemd ;; openrc) echo OpenRC ;; *) echo '未检测到（将只安装二进制）' ;; esac)"

# ------------------------------------------------------------------ version

step "查询版本"

if [ "$VERSION" = "latest" ]; then
    # Ask the API rather than following the /latest redirect, so the resolved
    # version can be printed and used in the checksum lookup.
    VERSION="$($DOWNLOAD "https://api.github.com/repos/$REPO/releases/latest" \
        | sed -n 's/.*"tag_name":[[:space:]]*"\([^"]*\)".*/\1/p' \
        | head -n1)"
    [ -n "$VERSION" ] || die "无法获取最新版本号。检查网络，或用 SYNCTUS_VERSION=v0.1.0 指定版本"
fi
say "  版本    $VERSION"

ASSET="synctus-server-${ARCH}-linux-${LIBC}.tar.gz"
BASE="https://github.com/$REPO/releases/download/$VERSION"

# ------------------------------------------------------------------ download

step "下载 $ASSET"

TMP="$(mktemp -d)"
# Clean up on any exit path, including a failed download.
trap 'rm -rf "$TMP"' EXIT INT TERM

$DOWNLOAD_TO "$TMP/$ASSET" "$BASE/$ASSET" \
    || die "下载失败: $BASE/$ASSET
该版本可能没有提供这个平台的产物，或网络不可达。"

# Integrity check. A truncated download produces a binary that fails in confusing
# ways, and an unverified one is a supply-chain hole.
if $DOWNLOAD_TO "$TMP/SHA256SUMS.txt" "$BASE/SHA256SUMS.txt" 2>/dev/null; then
    EXPECTED="$(grep " $ASSET\$" "$TMP/SHA256SUMS.txt" | awk '{print $1}' | head -n1)"
    if [ -n "$EXPECTED" ]; then
        if command -v sha256sum >/dev/null 2>&1; then
            ACTUAL="$(sha256sum "$TMP/$ASSET" | awk '{print $1}')"
        elif command -v shasum >/dev/null 2>&1; then
            ACTUAL="$(shasum -a 256 "$TMP/$ASSET" | awk '{print $1}')"
        else
            ACTUAL=""
            warn "没有 sha256sum，跳过校验"
        fi
        if [ -n "$ACTUAL" ]; then
            [ "$ACTUAL" = "$EXPECTED" ] || die "校验失败。
  期望 $EXPECTED
  实际 $ACTUAL
下载可能被截断或篡改，请重试。"
            say "  校验    ${G}通过${N}"
        fi
    else
        warn "校验文件中没有 $ASSET 的记录，跳过校验"
    fi
else
    warn "无法下载校验文件，跳过校验"
fi

tar -xzf "$TMP/$ASSET" -C "$TMP" || die "解压失败"

# The archive has a top-level directory; find the binaries wherever they landed.
SERVER_BIN="$(find "$TMP" -type f -name synctus-server | head -n1)"
CTL_BIN="$(find "$TMP" -type f -name synctus | head -n1)"
[ -n "$SERVER_BIN" ] || die "压缩包里没有 synctus-server"

# ------------------------------------------------------------------ install

step "安装到 $PREFIX"

# Stop first if running: replacing a running binary is allowed on Linux, but the
# service keeps the old one until restarted, which makes "did the upgrade work"
# unanswerable.
UPGRADING=no
if [ -x "$PREFIX/synctus-server" ]; then
    UPGRADING=yes
    case "$INIT" in
        systemd) systemctl stop synctus-server 2>/dev/null || true ;;
        openrc)  rc-service synctus-server stop 2>/dev/null || true ;;
    esac
fi

install -d -m 0755 "$PREFIX"
install -m 0755 "$SERVER_BIN" "$PREFIX/synctus-server"
if [ -n "$CTL_BIN" ]; then
    install -m 0755 "$CTL_BIN" "$PREFIX/synctus"
fi
say "  已安装  synctus-server$([ -n "$CTL_BIN" ] && echo ', synctus')"

# Whether a group with the given name exists, read straight from /etc/group so
# the check does not depend on `getent`, whose presence varies across busybox
# builds.
group_exists() {
    awk -F: -v g="$1" '$1 == g { found = 1 } END { exit !found }' /etc/group
}

# A service account with no login shell and no home: the relay needs a network
# socket and nothing else.
if [ "$INIT" != "none" ] && ! id "$SERVICE_USER" >/dev/null 2>&1; then
    if command -v useradd >/dev/null 2>&1; then
        useradd --system --no-create-home --shell /sbin/nologin "$SERVICE_USER" 2>/dev/null || true
    else
        # busybox (Alpine). Create the group explicitly first: some busybox
        # builds do not auto-create a same-named group, and OpenRC's checkpath
        # refuses to start the service when it cannot resolve the owner.
        addgroup -S "$SERVICE_USER" 2>/dev/null || true
        adduser -S -H -s /sbin/nologin -G "$SERVICE_USER" "$SERVICE_USER" 2>/dev/null || true
    fi

    if id "$SERVICE_USER" >/dev/null 2>&1 && group_exists "$SERVICE_USER"; then
        say "  已创建  服务账号 $SERVICE_USER"
    else
        warn "无法创建服务账号 $SERVICE_USER，将用 root 运行（可在安装后手动修复）"
        SERVICE_USER="root"
    fi
fi

# ------------------------------------------------------------------ config

step "配置"

install -d -m 0755 "$CONFIG_DIR"

if [ -f "$CONFIG" ]; then
    say "  保留    已有配置 $CONFIG"
else
    cat > "$CONFIG" <<EOF
# Synctus 中继服务器配置
#
# 改完运行 \`synctus\` 选择重启，或 systemctl restart synctus-server。

# 监听地址。0.0.0.0 表示接受来自任何网卡的连接。
bind = "0.0.0.0:$PORT"

# TLS 证书链与私钥（PEM）。两者都填写时服务器自行终止 TLS。
#
# 不配置 TLS 时消息内容仍是端到端加密的，服务器和中间人都读不到；
# 但房间标识与设备标识会以明文经过网络。公网部署建议配上证书。
# cert_path = "/etc/letsencrypt/live/example.com/fullchain.pem"
# key_path  = "/etc/letsencrypt/live/example.com/privkey.pem"

# 单个房间允许的设备数。两人使用时 2 就够，默认留出余量
# 以便一个人同时使用电脑和手机。
max_devices_per_room = 8

# 内存中保留的房间上限，防止陌生人用随机房间号消耗内存。
max_rooms = 10000

# 超过该秒数没有收到任何帧就断开连接。必须大于 heartbeat_secs。
idle_timeout_secs = 90
heartbeat_secs = 25

# 每设备每秒可转发的消息数（令牌桶）与突发额度。
rate_limit_per_sec = 10
rate_limit_burst = 30

# 客户端完成握手的时限。
handshake_timeout_secs = 10
EOF
    chmod 0644 "$CONFIG"
    say "  已生成  $CONFIG"
fi

# The config may name a private key, so keep the directory readable only by the
# service account and root.
if id "$SERVICE_USER" >/dev/null 2>&1; then
    chown -R "root:$SERVICE_USER" "$CONFIG_DIR" 2>/dev/null || true
    chmod 0750 "$CONFIG_DIR" 2>/dev/null || true
fi
# ------------------------------------------------------------------ service

case "$INIT" in
systemd)
    step "安装 systemd 服务"
    cat > /etc/systemd/system/synctus-server.service <<EOF
[Unit]
Description=Synctus relay server
Documentation=https://github.com/$REPO
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
ExecStart=$PREFIX/synctus-server --config $CONFIG
Restart=on-failure
RestartSec=5s

User=$SERVICE_USER
Group=$SERVICE_USER

# The admin socket lives here; systemd creates and cleans it up.
RuntimeDirectory=synctus
RuntimeDirectoryMode=0750

# The relay holds no persistent state, so almost nothing needs to be reachable.
ReadOnlyPaths=$CONFIG_DIR
NoNewPrivileges=yes
PrivateTmp=yes
PrivateDevices=yes
ProtectSystem=strict
ProtectHome=yes
ProtectKernelTunables=yes
ProtectKernelModules=yes
ProtectControlGroups=yes
RestrictAddressFamilies=AF_INET AF_INET6 AF_UNIX
RestrictNamespaces=yes
RestrictRealtime=yes
LockPersonality=yes
MemoryDenyWriteExecute=yes
SystemCallArchitectures=native
SystemCallFilter=@system-service
SystemCallFilter=~@privileged @resources

# A few hundred KiB of state per room; a generous ceiling that still stops a
# runaway from taking the host down.
MemoryMax=256M

Environment=SYNCTUS_LOG=synctus_server=info

[Install]
WantedBy=multi-user.target
EOF

    # Reading a certificate from Let's Encrypt needs that path allowed through
    # ProtectSystem=strict.
    if [ -d /etc/letsencrypt ]; then
        sed -i "s|^ReadOnlyPaths=.*|ReadOnlyPaths=$CONFIG_DIR /etc/letsencrypt|" \
            /etc/systemd/system/synctus-server.service
    fi

    systemctl daemon-reload
    systemctl enable synctus-server >/dev/null 2>&1
    systemctl restart synctus-server
    say "  已启用开机自启并启动"
    ;;

openrc)
    step "安装 OpenRC 服务"
    cat > /etc/init.d/synctus-server <<EOF
#!/sbin/openrc-run

name="synctus-server"
description="Synctus relay server"

command="$PREFIX/synctus-server"
command_args="--config $CONFIG"
command_user="$SERVICE_USER:$SERVICE_USER"
command_background=true
pidfile="/run/\${RC_SVCNAME}.pid"

# OpenRC has no journal, so logs go to a file that \`synctus\` knows how to read.
output_log="/var/log/synctus-server.log"
error_log="/var/log/synctus-server.log"

export SYNCTUS_LOG="synctus_server=info"

depend() {
    need net
    after firewall
}

start_pre() {
    # /run is tmpfs and cleared on reboot, so the admin-socket directory is
    # recreated on every start.
    #
    # The \`|| true\` is deliberate: a failure to resolve the owner (or anything
    # else) must not abort the start and leave the service dead. At worst the
    # socket falls back to root ownership and \`synctus\` still works when run
    # with sudo.
    checkpath --directory --owner "$SERVICE_USER:$SERVICE_USER" --mode 0750 /run/synctus 2>/dev/null || true
    checkpath --file --owner "$SERVICE_USER:$SERVICE_USER" --mode 0640 /var/log/synctus-server.log 2>/dev/null || true
    return 0
}
EOF
    chmod 0755 /etc/init.d/synctus-server
    rc-update add synctus-server default >/dev/null 2>&1
    rc-service synctus-server restart >/dev/null 2>&1 || rc-service synctus-server start
    say "  已启用开机自启并启动"
    ;;

*)
    warn "未检测到 systemd 或 OpenRC，只安装了二进制。"
    say "  手动运行: $PREFIX/synctus-server --config $CONFIG"
    ;;
esac

# ------------------------------------------------------------------ verify

step "验证"

sleep 1
RUNNING=no
case "$INIT" in
    systemd) systemctl is-active --quiet synctus-server && RUNNING=yes ;;
    openrc)  rc-service synctus-server status 2>/dev/null | grep -q started && RUNNING=yes ;;
esac

INSTALLED_VERSION="$("$PREFIX/synctus-server" --version 2>/dev/null || echo '?')"
say "  二进制  $INSTALLED_VERSION"

if [ "$INIT" != "none" ]; then
    if [ "$RUNNING" = "yes" ]; then
        say "  服务    ${G}运行中${N}"
    else
        warn "服务没有保持运行。"
        case "$INIT" in
            systemd) say "  查看原因: journalctl -u synctus-server -n 30" ;;
            openrc)  say "  查看原因: tail -n 30 /var/log/synctus-server.log" ;;
        esac
    fi
fi

# ------------------------------------------------------------------ next steps

# The public address is a best-effort lookup: it is a convenience for filling in
# the client, not something the installer depends on.
PUBLIC_IP="$($DOWNLOAD https://api.ipify.org 2>/dev/null || echo '<服务器地址>')"

say ""
if [ "$UPGRADING" = "yes" ]; then
    say "${G}${B}升级完成。${N}"
else
    say "${G}${B}安装完成。${N}"
fi
say ""
say "在三端的「设置 → 服务器」里填入："
say ""
say "    地址    ${B}$PUBLIC_IP:$PORT${N}"
say "    TLS     关闭"
say ""
say "两个人必须填入${B}同一个配对码${N}，在任一端点「生成」得到。"
say ""
say "日常管理直接运行 ${B}synctus${N} —— 会出现一个数字菜单。"
say ""

if [ "$PORT" != "" ]; then
    say "${Y}别忘了放行端口 $PORT：${N}"
    if command -v ufw >/dev/null 2>&1; then
        say "    ufw allow $PORT/tcp"
    elif command -v firewall-cmd >/dev/null 2>&1; then
        say "    firewall-cmd --add-port=$PORT/tcp --permanent && firewall-cmd --reload"
    else
        say "    在云服务商的安全组里放行 TCP $PORT"
    fi
    say "云服务器还需要在控制台的安全组里放行。"
fi

say ""
say "${Y}生产环境建议配置 TLS：${N}运行 synctus 选「修改配置」填入证书路径。"
say "不配也能用，消息内容始终是端到端加密的。"
