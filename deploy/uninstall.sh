#!/bin/sh
# Synctus 中继服务器卸载
#
#   curl -fsSL https://raw.githubusercontent.com/LANqed/Synctus/main/deploy/uninstall.sh | sudo sh
#
# Keeps the config by default: someone uninstalling to reinstall a different
# version should not have to retype their settings. `--purge` removes it too.

set -eu

PREFIX="${SYNCTUS_PREFIX:-/usr/local/bin}"
CONFIG_DIR="${SYNCTUS_CONFIG_DIR:-/etc/synctus}"
SERVICE_USER="synctus"
PURGE=no

for arg in "$@"; do
    case "$arg" in
        --purge) PURGE=yes ;;
        *) echo "未知参数: $arg" >&2; exit 2 ;;
    esac
done

if [ -t 1 ]; then
    B='\033[1m'; Y='\033[33m'; N='\033[0m'
else
    B=''; Y=''; N=''
fi

say()  { printf '%b\n' "$*"; }
step() { printf '%b\n' "${B}==>${N} $*"; }

[ "$(id -u)" = "0" ] || { echo "需要 root 权限。" >&2; exit 1; }

step "停止并禁用服务"
if [ -d /run/systemd/system ]; then
    systemctl stop synctus-server 2>/dev/null || true
    systemctl disable synctus-server 2>/dev/null || true
    rm -f /etc/systemd/system/synctus-server.service
    systemctl daemon-reload
    say "  已移除 systemd 服务"
elif command -v rc-update >/dev/null 2>&1; then
    rc-service synctus-server stop 2>/dev/null || true
    rc-update del synctus-server default 2>/dev/null || true
    rm -f /etc/init.d/synctus-server
    say "  已移除 OpenRC 服务"
fi

step "移除二进制"
rm -f "$PREFIX/synctus-server" "$PREFIX/synctus"
say "  已移除 $PREFIX/synctus-server, $PREFIX/synctus"

# The socket directory is runtime state; systemd normally cleans it up, but a
# manual run would have left it behind.
rm -rf /run/synctus

if [ "$PURGE" = "yes" ]; then
    step "移除配置与日志"
    rm -rf "$CONFIG_DIR"
    rm -f /var/log/synctus-server.log
    say "  已移除 $CONFIG_DIR"

    if id "$SERVICE_USER" >/dev/null 2>&1; then
        if command -v userdel >/dev/null 2>&1; then
            userdel "$SERVICE_USER" 2>/dev/null || true
        elif command -v deluser >/dev/null 2>&1; then
            deluser "$SERVICE_USER" 2>/dev/null || true
        fi
        say "  已移除服务账号 $SERVICE_USER"
    fi
else
    say ""
    say "${Y}保留了配置${N} $CONFIG_DIR"
    say "要一并删除，重新运行并加上 --purge。"
fi

say ""
say "${B}卸载完成。${N}"
