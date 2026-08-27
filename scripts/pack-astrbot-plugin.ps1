# 打包 AstrBot 插件为可上传的 ZIP。
#
# AstrBot 的「从本地上传」需要一个包含插件目录的归档，且目录名必须与
# metadata.yaml 的 name 一致。这个脚本只做三件事：校验必需文件、
# 排除测试与缓存、生成 dist/<插件名>-<版本>.zip。
#
# 用法：
#   pwsh scripts/pack-astrbot-plugin.ps1
#   pwsh scripts/pack-astrbot-plugin.ps1 -OutDir D:\somewhere

[CmdletBinding()]
param(
    [string]$OutDir = ""
)

$ErrorActionPreference = "Stop"

$RepoRoot = Split-Path -Parent $PSScriptRoot
$PluginName = "astrbot_plugin_synctus_companion"
$PluginDir = Join-Path $RepoRoot $PluginName

if (-not (Test-Path -LiteralPath $PluginDir)) {
    throw "找不到插件目录: $PluginDir"
}

# 缺了这些文件 AstrBot 装上也跑不起来，宁可在打包时失败。
$Required = @(
    "metadata.yaml",
    "_conf_schema.json",
    "__init__.py",
    "main.py",
    "companion_bridge.py",
    "battery.py",
    "tasks.py",
    "presence.py",
    "synctus/__init__.py",
    "synctus/client.py",
    "synctus/crypto.py",
    "synctus/model.py",
    "synctus/proto.py"
)
$Missing = @()
foreach ($rel in $Required) {
    if (-not (Test-Path -LiteralPath (Join-Path $PluginDir $rel))) {
        $Missing += $rel
    }
}
if ($Missing.Count -gt 0) {
    throw "插件缺少必需文件: $($Missing -join ', ')"
}

$Version = (Select-String -LiteralPath (Join-Path $PluginDir "metadata.yaml") `
    -Pattern '^version:\s*(.+)$').Matches[0].Groups[1].Value.Trim()
if (-not $Version) { throw "metadata.yaml 里读不到 version" }

if (-not $OutDir) { $OutDir = Join-Path $RepoRoot "dist" }
New-Item -ItemType Directory -Force -Path $OutDir | Out-Null
$ZipPath = Join-Path $OutDir "$PluginName-$Version.zip"

# 先复制到临时目录再压缩：Compress-Archive 无法按模式排除文件，
# 而 __pycache__ 和 tests 不应进入用户的插件目录。
$Staging = Join-Path ([System.IO.Path]::GetTempPath()) "astrbot-pack-$(Get-Random)"
$StagedPlugin = Join-Path $Staging $PluginName
try {
    New-Item -ItemType Directory -Force -Path $StagedPlugin | Out-Null
    Get-ChildItem -LiteralPath $PluginDir -Force |
        Copy-Item -Destination $StagedPlugin -Recurse -Force
    foreach ($junk in @("tests", "__pycache__", "synctus\__pycache__", ".ruff_cache")) {
        $target = Join-Path $StagedPlugin $junk
        if (Test-Path -LiteralPath $target) {
            Remove-Item -LiteralPath $target -Recurse -Force
        }
    }

    if (Test-Path -LiteralPath $ZipPath) { Remove-Item -LiteralPath $ZipPath -Force }
    Compress-Archive -Path $StagedPlugin -DestinationPath $ZipPath -CompressionLevel Optimal

    $size = [math]::Round((Get-Item -LiteralPath $ZipPath).Length / 1KB, 1)
    Write-Output "已生成 $ZipPath ($size KB)"
    Write-Output "在 AstrBot WebUI 的插件管理里选「从本地上传」，选这个文件即可。"
    Write-Output "别忘了在 AstrBot 环境执行: pip install argon2-cffi pynacl"
}
finally {
    if (Test-Path -LiteralPath $Staging) {
        Remove-Item -LiteralPath $Staging -Recurse -Force -ErrorAction SilentlyContinue
    }
}
