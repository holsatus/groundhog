# Android build

The first-pass Android packaging for the iced frontend. See
[`docs/android-port.md`](../docs/android-port.md) for the plan and the
limitations.

## Requirements

- Rust target: `rustup target add aarch64-linux-android`
- Android SDK with:
  - NDK (r28 or newer; r30 is what this was built with)
  - build-tools (`aapt2`, `zipalign`, `apksigner`)
  - platform `android-36`
- `ANDROID_HOME` (defaults to `$HOME/Android/Sdk`), or set
  `ANDROID_NDK_ROOT`, `ANDROID_BUILD_TOOLS` and `ANDROID_PLATFORM` explicitly.
- A JDK (`keytool`; `apksigner` runs on the JVM).

## Build

```sh
./android/build.sh
```

The script builds `libgroundhog_gui_iced.so` for `arm64-v8a`, links the
manifest with `aapt2`, stores the library uncompressed and page-aligned
(`zipalign -P 16`), and signs a debug APK:

```
android/out/groundhog-debug.apk
```

The debug keystore is generated once at `android/debug.keystore` (both
`out/` and the keystore are gitignored).

## Install

```sh
adb install -r android/out/groundhog-debug.apk
adb logcat -s groundhog
```

`groundhog` is the log tag used by `android_logger`; Rust panics are logged
there too.

The application id is `org.holsatus.groundhog`.
