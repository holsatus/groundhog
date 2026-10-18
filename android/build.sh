#!/usr/bin/env bash
# Builds the debug APK for an arm64 Android phone.
#
#   android/build.sh
#
# Needs an Android SDK with an NDK, build-tools and a platform (see
# docs/android-port.md). The Rust target must be installed:
#   rustup target add aarch64-linux-android
#
# The APK is written to android/out/groundhog-debug.apk.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
SDK="${ANDROID_HOME:-${ANDROID_SDK_ROOT:-$HOME/Android/Sdk}}"

newest() {
    # The match for $1 that sorts last, or nothing.
    compgen -G "$1" | sort -V | tail -1
}

NDK="${ANDROID_NDK_ROOT:-$(newest "$SDK/ndk/*")}"
BUILD_TOOLS="${ANDROID_BUILD_TOOLS:-$(newest "$SDK/build-tools/*")}"
PLATFORM="${ANDROID_PLATFORM:-$(newest "$SDK/platforms/android-*")}"

for required in NDK BUILD_TOOLS PLATFORM; do
    if [ -z "${!required}" ] || [ ! -d "${!required}" ]; then
        echo "error: $required not found under $SDK" >&2
        echo "       set ANDROID_HOME, or ANDROID_NDK_ROOT / ANDROID_BUILD_TOOLS / ANDROID_PLATFORM" >&2
        exit 1
    fi
done

# Android 8.0, the minimum the manifest declares.
API=26
ABI=arm64-v8a
TARGET=aarch64-linux-android
LIB=libgroundhog_gui_iced.so

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
# Android 15+ devices can use 16 KiB pages; the NDK r28+ linker aligns
# segments for that by default, this makes it explicit and is harmless on
# older devices.
export CARGO_TARGET_AARCH64_LINUX_ANDROID_RUSTFLAGS="-C link-arg=-Wl,-z,max-page-size=16384"

mkdir -p "$ROOT/android/out"
OUT="$ROOT/android/out"
STAGING="$OUT/staging"

cd "$ROOT"
echo "==> building $LIB for $TARGET (release)"
# The manifest builds the library as an `rlib` only; the shared object
# Android loads is asked for here.
cargo rustc --release --lib -p groundhog-gui-iced --target "$TARGET" --crate-type cdylib

echo "==> staging"
rm -rf "$STAGING" "$OUT/app-unsigned.apk" "$OUT/app-aligned.apk" "$OUT/groundhog-debug.apk"
mkdir -p "$STAGING/lib/$ABI"
cp "$ROOT/target/$TARGET/release/$LIB" "$STAGING/lib/$ABI/$LIB"

echo "==> linking manifest"
"$BUILD_TOOLS/aapt2" link \
    -o "$OUT/app-unsigned.apk" \
    -I "$PLATFORM/android.jar" \
    --manifest "$ROOT/android/AndroidManifest.xml" \
    --min-sdk-version "$API" \
    --target-sdk-version 36

echo "==> adding native library"
# Stored uncompressed: with extractNativeLibs=false Android maps it from the
# APK, and zipalign below page-aligns it.
(
    cd "$STAGING"
    zip -q -0 -r "$OUT/app-unsigned.apk" "lib"
)

echo "==> aligning"
"$BUILD_TOOLS/zipalign" -f -P 16 4 "$OUT/app-unsigned.apk" "$OUT/app-aligned.apk"

echo "==> signing"
KEYSTORE="$ROOT/android/debug.keystore"
if [ ! -f "$KEYSTORE" ]; then
    keytool -genkeypair \
        -keystore "$KEYSTORE" \
        -storepass android -keypass android \
        -alias androiddebugkey \
        -keyalg RSA -keysize 2048 -validity 10000 \
        -dname "CN=Android Debug,O=Android,C=US" \
        -noprompt >/dev/null
fi
"$BUILD_TOOLS/apksigner" sign \
    --ks "$KEYSTORE" \
    --ks-pass pass:android --key-pass pass:android \
    --ks-key-alias androiddebugkey \
    --out "$OUT/groundhog-debug.apk" \
    "$OUT/app-aligned.apk"

"$BUILD_TOOLS/apksigner" verify --print-certs "$OUT/groundhog-debug.apk" | head -6 || true
echo "==> $OUT/groundhog-debug.apk"
