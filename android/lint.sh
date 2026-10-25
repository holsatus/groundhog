#!/usr/bin/env bash
# Lints the library as it is built for Android.
#
#   android/lint.sh
#
# `platform/android.rs` and the `android_main` export only exist for
# `target_os = "android"`, so the desktop lint run never sees them: this is
# what keeps them to the same rules (and to the crate's `unsafe_code` deny,
# whose only two exceptions are documented in place).
#
# Lints the library only. The `groundhog` binary is a desktop entry point
# (`main.rs` calls `run`, which is not compiled for Android); `build.sh` builds
# the library too.
#
# Needs an Android SDK with an NDK, and `rustup target add
# aarch64-linux-android`.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
SDK="${ANDROID_HOME:-${ANDROID_SDK_ROOT:-$HOME/Android/Sdk}}"

newest() {
    # The match for $1 that sorts last, or nothing.
    compgen -G "$1" | sort -V | tail -1
}

NDK="${ANDROID_NDK_ROOT:-$(newest "$SDK/ndk/*")}"

if [ -z "$NDK" ] || [ ! -d "$NDK" ]; then
    echo "error: no NDK found under $SDK" >&2
    echo "       set ANDROID_HOME, or ANDROID_NDK_ROOT" >&2
    exit 1
fi

# Android 8.0, the minimum the manifest declares (see build.sh).
API=26
TARGET=aarch64-linux-android

HOST_TAG=linux-x86_64
case "$(uname -s)" in
    Darwin) HOST_TAG=darwin-x86_64 ;;
esac
TOOLCHAIN="$NDK/toolchains/llvm/prebuilt/$HOST_TAG"
CLANG="$TOOLCHAIN/bin/$TARGET$API-clang"

export ANDROID_NDK_ROOT="$NDK"
export ANDROID_NDK_HOME="$NDK"
export CC_aarch64_linux_android="$CLANG"
export CXX_aarch64_linux_android="$TOOLCHAIN/bin/$TARGET$API-clang++"
export AR_aarch64_linux_android="$TOOLCHAIN/bin/llvm-ar"
export RANLIB_aarch64_linux_android="$TOOLCHAIN/bin/llvm-ranlib"
export CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER="$CLANG"

cd "$ROOT"
cargo clippy --lib -p groundhog-gui-iced --target "$TARGET" -- -D warnings
