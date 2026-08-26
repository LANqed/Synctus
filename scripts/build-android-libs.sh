#!/usr/bin/env bash
# Build the Android native libraries into android/app/src/main/jniLibs.
#
# CI does this with cargo-ndk directly; this script is the local equivalent so a
# contributor does not have to remember the flags.
#
# Requires: cargo-ndk (`cargo install cargo-ndk`) and an NDK, located through
# ANDROID_NDK_HOME or ANDROID_HOME/ndk/<version>.

set -euo pipefail

cd "$(dirname "$0")/.."

if ! command -v cargo-ndk >/dev/null 2>&1; then
    echo "cargo-ndk not found. Install it with: cargo install cargo-ndk" >&2
    exit 1
fi

# Locate an NDK if the caller did not point at one.
if [[ -z "${ANDROID_NDK_HOME:-}" ]]; then
    sdk="${ANDROID_HOME:-${ANDROID_SDK_ROOT:-$HOME/Android/Sdk}}"
    if [[ -d "$sdk/ndk" ]]; then
        # Highest version present.
        ANDROID_NDK_HOME="$(find "$sdk/ndk" -maxdepth 1 -mindepth 1 -type d | sort -V | tail -n1)"
        export ANDROID_NDK_HOME
        echo "Using NDK at $ANDROID_NDK_HOME"
    else
        echo "No NDK found. Set ANDROID_NDK_HOME." >&2
        exit 1
    fi
fi

# Only 64-bit ABIs, matching abiFilters in app/build.gradle.kts.
cargo ndk \
    -t arm64-v8a \
    -t x86_64 \
    -o android/app/src/main/jniLibs \
    build --release -p synctus-mobile

echo
echo "Built:"
find android/app/src/main/jniLibs -name '*.so' -exec ls -lh {} \;
