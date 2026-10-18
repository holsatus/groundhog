# Android port — plan and first-pass report

Status: **first-pass APK builds and host gates pass**. This pass targets a
single outcome: an installable `.apk` for `aarch64` (arm64-v8a) that builds
from this repository and starts the existing iced frontend on a phone. No UI
redesign, no serial/USB work, no file dialogs. Those are later passes.

## Result of this pass

- `android/build.sh` builds `android/out/groundhog-debug.apk` (~34 MB,
  `arm64-v8a`, `minSdk 26`, `targetSdk 36`).
- The APK verifies with APK Signature Scheme v2 and v3 (debug keystore).
- `lib/arm64-v8a/libgroundhog_gui_iced.so` is stored uncompressed and its
  `LOAD` segments are 16 KB-aligned (`0x4000`), as Android 15+ expects.
- The `.so` exports both `android_main` and `ANativeActivity_onCreate`, and
  depends only on system libraries (`liblog`, `libandroid`, `libdl`, `libm`,
  `libc`) — no `libc++_shared.so` to ship.
- **Installed and run on a Pixel 7 (Android 16, arm64)**: the frontend starts,
  the rail and panes draw, and the map loads OpenStreetMap tiles.
- Panels, config and telemetry recording work: the persisted `config.ron` is
  read back on the next start, and a `.tlog` is written under the app's
  private data directory.
- The status bar is not inset yet, so its icons (including the system battery
  pill) draw over the top of the map; that is the next pass.

## Why this is not just "add a target"

`groundhog-logic` already cross-compiles for `aarch64-linux-android` (verified:
`cargo check -p groundhog-logic --target aarch64-linux-android`). The frontend
does not, for two independent reasons:

1. **iced has no Android entry point.** `iced_winit::run` creates
   `EventLoop::with_user_event().build()` without the `AndroidApp` from
   `android_main`, which winit requires on Android and which is only available
   there. There is no `main()` on Android at all: the application is a
   `cdylib` with an exported `android_main`.
2. **Two dependencies do not build for Android.**
   - `arboard` (iced's clipboard) has no Android backend and fails to compile.
   - `rfd` (our file dialogs) implements its traits only for
     Linux/BSD, macOS, Windows and wasm; it fails to compile on Android.

iced upstream treats mobile as out of scope (`iced-rs/iced#302`). The
community fork `iced-mobile/iced` tracks iced master and carries the minimal
shell support as six small commits. That is the basis of this pass.

## Approach: a local iced fork, patched in

A local clone of iced at the revision this repository pins
(`08eb93ad476597889eb4597f22b4198f5545abb9`) with the six `iced-mobile`
commits cherry-picked on top:

```
/home/peterkrull/git/iced-android     branch "android", base 08eb93a
  85dc904 fix: make arboard compile on Android
  7e05b15 fix conversion of event
  9828383 add `run_android` method and recreate surface when `Resumed` …
  8108155 include fonts
  cc29074 android object
  8356483 public export
  11ac39a document the Android application handle   (needed: iced denies missing docs)
```

The `arboard` fix is in the fork: `arboard` is redirected to the
`iced-mobile/arboard` fork (a dummy backend that returns "clipboard
unavailable"), so the iced clipboard API still exists on Android.

The workspace root redirects **every iced crate a dependency requests directly**
to that clone. `iced` alone is not enough: `slippery` also depends on
`iced_core` and `iced_graphics`, and the frontend on `iced_runtime`, so each
needs its own `[patch]` entry or Cargo builds two copies of iced and the types
stop matching.

```toml
[patch."https://github.com/iced-rs/iced"]
iced           = { path = "../iced-android" }
iced_core      = { path = "../iced-android/core" }
iced_debug     = { path = "../iced-android/debug" }
iced_futures   = { path = "../iced-android/futures" }
iced_graphics  = { path = "../iced-android/graphics" }
iced_program   = { path = "../iced-android/program" }
iced_renderer  = { path = "../iced-android/renderer" }
iced_runtime   = { path = "../iced-android/runtime" }
iced_test      = { path = "../iced-android/test" }
iced_tiny_skia = { path = "../iced-android/tiny_skia" }
iced_widget    = { path = "../iced-android/widget" }
iced_winit     = { path = "../iced-android/winit" }
iced_wgpu      = { path = "../iced-android/wgpu" }
```

`slippery` needs the same treatment for a different reason: it fetches map
tiles through `reqwest`, whose Android certificate verifier
(`rustls-platform-verifier`) needs a JVM-side Kotlin component that only a
Gradle build can add — without one the verifier **panics** and every tile
fetch fails. The local `slippery` fork gives Android a `rustls` client
trusting Mozilla's bundled roots (`webpki-roots`), which needs no Java.

```toml
[patch."https://github.com/peterkrull/slippery"]
slippery = { path = "../slippery-android" }
```

Because `rfd` cannot be made to build for Android, it is a desktop-only
dependency (`[target.'cfg(not(target_os = "android"))'.dependencies]`) and the
`file_dialog` helper gets an Android shim that reports "cancelled". That is a
placeholder, not a design; the real file picker is a later pass.

The Android entry point lives in the frontend library:

```rust
#[cfg(target_os = "android")]
#[unsafe(no_mangle)]
pub extern "C" fn android_main(app: android_activity::AndroidApp) {
    platform::android::init(app.clone());
    let _ = application().run_android(app);
}
```

`android-activity = { features = ["native-activity"] }` is added as a direct
dependency on Android. This is what turns on winit's Android backend: winit's
own feature is just `android-activity/native-activity`, and Cargo unifies
features across the graph.

## What the first pass changes

| Area | Change |
|---|---|
| Crates | `groundhog-gui-iced` gains a `cdylib` library next to its binary; all modules move from `main.rs` into `lib.rs` |
| Entry | `src/main.rs` calls `groundhog_gui_iced::run()`; `android_main` starts the same `Application` through `run_android` |
| Paths | new `platform` module: desktop uses `directories`, Android uses `AndroidApp::internal_data_path()` (`<files>/config/config.ron`, `<files>/data/tlogs`) |
| Logging | desktop keeps `env_logger`; Android uses `android_logger` (`adb logcat -s groundhog`) plus a panic hook, so a panic is logged instead of vanishing into stderr |
| Files | `rfd` is desktop-only; the Android shim returns "cancelled" for pick/save |
| Fonts | the iced fork loads `/system/fonts`, which Android needs: cosmic-text finds fonts through fontconfig, which Android does not have |
| Tiles | the `slippery` fork gives Android a `rustls` client with Mozilla's bundled roots, because `reqwest`'s default Android verifier panics without a JVM-side component |
| Lifecycle | the iced fork creates the window on `Resumed` and recreates the wgpu surface there, because Android destroys the native window when the app is backgrounded |
| Packaging | `android/` holds the manifest and a Gradle-free build script: `cargo build`, `aapt2 link`, `zip`, `zipalign`, `apksigner` |

Explicitly **not** in this pass: touch behaviour for the app's own widgets,
phone layout, insets/soft-keyboard polish, USB serial, SAF import/export,
foreground service, icons/signing for release.

## Toolchain

- Android SDK at `$HOME/Android/Sdk` (installed for this pass):
  - NDK `30.0.16248370` (16 KB page alignment by default)
  - Build-tools `37.0.0` (`aapt2`, `zipalign`, `apksigner`)
  - Platform `android-36` (`android.jar`)
- `ANDROID_HOME=$HOME/Android/Sdk`, `ANDROID_NDK_ROOT=$SDK/ndk/30.0.16248370`
- Rust target `aarch64-linux-android` (installed)
- `minSdk 26`, `targetSdk 36`, ABI `arm64-v8a`

## Build

```sh
android/build.sh            # release .so + signed debug .apk
adb install -r android/out/groundhog-debug.apk
adb logcat -s groundhog
```

The script sets the target compiler/linker from the NDK, builds only the
library (`cargo build --release --lib -p groundhog-gui-iced`), links the
manifest with `aapt2`, adds `lib/arm64-v8a/libgroundhog_gui_iced.so`
uncompressed, aligns it with `zipalign -P 16` (16 KiB pages, so it also works
on Android 15+ devices) and signs with a generated debug keystore.

## Passes

### Pass 1 — window insets (done)

Android 15+ draws the application edge to edge, so the status bar, the display
cutout, the navigation bar and the soft keyboard cover it. `platform::Insets`
carries the four sides in logical pixels and `Application::view` pads the root
by them, so the rail, the panes, the toasts and the confirmation dialog all
move out of the system's way together.

The values come from `Window.getDecorView().getRootWindowInsets()` over JNI —
the same call `iced_android` makes from Java — with the physical pixels
divided by the display density (`Configuration::density() / 160`). API 30+
reads `WindowInsets.getInsets(Type)` for the bars, the cutout and the IME;
older releases use the deprecated `getSystemWindowInset*` methods.

A Java `View.OnApplyWindowInsetsListener` would push changes, but that needs a
compiled activity subclass, so the insets are polled every 250 ms; the
subscription only publishes a message when the values change, so an idle
application is not redrawn (`dumpsys cpuinfo` reports 0% while idle).

The status- and navigation-bar icon colour follows the app theme through
`WindowInsetsController.setSystemBarsAppearance`, posted to the Java main
thread because the controller belongs to that thread.

Verified on the Pixel 7: in portrait and landscape the content starts below the
status bar and above the navigation bar, and the soft keyboard pushes the
content up while it is open.

### Pass 2 — phone layout (done)

Below 640 logical pixels of grid width — a phone in portrait, at the default
scale — the workspace is replaced by one panel at a time. The rail grows a Map
button and selects the panel that fills the window; tapping the selected panel
returns to the map. The persisted `Workspace` is left untouched (the compact
panel is view state), so a wide window — a tablet, or the phone in landscape —
restores the three-column arrangement exactly as it was.

`Layout` owns the switch (`COMPACT_BELOW`, `Message::Select`); the application
draws the rail accordingly and hides the pane's maximize and close controls,
which mean nothing with a single pane.

Verified on the Pixel 7: portrait shows one panel, the rail switches panels and
returns to the map, and rotating to landscape restores the workspace.

## Known limitations

- Pick/save file actions do nothing on Android.
- Serial links cannot open; UDP/TCP work (INTERNET permission is declared).
- Soft keyboard text entry is basic (key events); IME composition, clipboard
  and long-press selection are not implemented in the minimal fork.
- No foreground service: telemetry stops if Android freezes the process in the
  background.
- The release library is not stripped, so the debug APK is ~34 MB; a
  distribution build should strip the `.so` (and split per ABI).
- The forks are local paths (`../iced-android`, `../slippery-android`); CI or
  another checkout needs the same directories, submodules, or published forks
  with matching `[patch]` URLs.

## Verification

1. `cargo build --release --lib -p groundhog-gui-iced --target aarch64-linux-android`
   succeeds. ✅
2. `android/out/groundhog-debug.apk` passes `apksigner verify` and contains
   `lib/arm64-v8a/libgroundhog_gui_iced.so` plus a binary manifest. ✅
3. `adb install` succeeds and the activity starts. ✅ (Pixel 7, Android 16;
   the map loads OpenStreetMap tiles, the UI draws, and the insets keep the
   content clear of the system bars and the keyboard.)
4. Host gates still pass: `cargo test --workspace` (188 tests),
   `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`.
   The Android target also passes `cargo clippy --target aarch64-linux-android
   -p groundhog-gui-iced --lib -- -D warnings`. ✅

## Later passes

1. Touch behaviour for the map overlay, the rail and the drag sources.
2. SAF import/export for missions and parameters, USB serial through the
   Android USB Host API, and a foreground service for background
   telemetry/recording.
3. Full IME/clipboard by porting the `iced_android` Java activity and the
   remaining iced widget patches.
