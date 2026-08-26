# Build the Android native libraries into android/app/src/main/jniLibs.
#
# CI does this with cargo-ndk directly; this script is the local equivalent so a
# contributor does not have to remember the flags.
#
# Requires: cargo-ndk (`cargo install cargo-ndk`) and an NDK, located through
# ANDROID_NDK_HOME or ANDROID_HOME\ndk\<version>.

$ErrorActionPreference = 'Stop'

Set-Location (Join-Path $PSScriptRoot '..')

if (-not (Get-Command cargo-ndk -ErrorAction SilentlyContinue)) {
    Write-Error 'cargo-ndk not found. Install it with: cargo install cargo-ndk'
}

# Locate an NDK if the caller did not point at one.
if (-not $env:ANDROID_NDK_HOME) {
    $sdk = if ($env:ANDROID_HOME) { $env:ANDROID_HOME }
           elseif ($env:ANDROID_SDK_ROOT) { $env:ANDROID_SDK_ROOT }
           else { Join-Path $env:LOCALAPPDATA 'Android\Sdk' }

    $ndkRoot = Join-Path $sdk 'ndk'
    if (-not (Test-Path -LiteralPath $ndkRoot)) {
        Write-Error 'No NDK found. Set ANDROID_NDK_HOME.'
    }

    # Highest version present.
    $latest = Get-ChildItem -LiteralPath $ndkRoot -Directory |
        Sort-Object { [version]($_.Name -replace '[^0-9.].*$', '') } |
        Select-Object -Last 1

    $env:ANDROID_NDK_HOME = $latest.FullName
    Write-Host "Using NDK at $env:ANDROID_NDK_HOME"
}

# Only 64-bit ABIs, matching abiFilters in app/build.gradle.kts.
cargo ndk -t arm64-v8a -t x86_64 -o android/app/src/main/jniLibs build --release -p synctus-mobile
if ($LASTEXITCODE -ne 0) { Write-Error "cargo ndk failed with exit code $LASTEXITCODE" }

Write-Host ''
Write-Host 'Built:'
Get-ChildItem android/app/src/main/jniLibs -Recurse -Filter '*.so' |
    Select-Object FullName, @{ Name = 'MB'; Expression = { [math]::Round($_.Length / 1MB, 2) } }
