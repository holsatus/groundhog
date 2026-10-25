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

## Approach: an iced fork, patched in

The fork is the `groundhog-compat` branch of `github.com/peterkrull/iced`:
iced master with the six `iced-mobile` commits cherry-picked on top, and a
documentation fix iced's `missing_docs` needs:

```
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

The workspace root (`Cargo.toml`) redirects **every iced crate a dependency
requests directly** to that branch. `iced` alone is not enough: `slippery`
also depends on `iced_core` and `iced_graphics`, and the frontend on
`iced_runtime`, so each needs its own `[patch]` entry or Cargo builds two
copies of iced and the types stop matching. `Cargo.lock` pins the branch's
revision.

```toml
[patch."https://github.com/iced-rs/iced"]
iced      = { git = "https://github.com/peterkrull/iced", branch = "groundhog-compat" }
iced_core = { git = "https://github.com/peterkrull/iced", branch = "groundhog-compat" }
# ... and so on for every iced crate, as in Cargo.toml
```

To work on the fork itself, point these entries at a local clone with
`path = "../iced"` (one per crate, e.g. `path = "../iced/core"`) for the
time being, and do not commit that.

`slippery` fetches map tiles through `reqwest`, whose Android certificate
verifier (`rustls-platform-verifier`) needs a JVM-side Kotlin component
that only a Gradle build can add; without one the verifier **panics** and
every tile fetch fails. `slippery` itself gives Android a `rustls` client
trusting Mozilla's bundled roots (`webpki-roots`, an Android-only
dependency), which needs no Java, so it needs no patch here.

Because `rfd` cannot be made to build for Android, it is a desktop-only
dependency (`[target.'cfg(not(target_os = "android"))'.dependencies]`) and the
`file_dialog` helper gets an Android shim that reports "cancelled". That is a
placeholder, not a design; the real file picker is a later pass.

The Android entry point lives in the frontend library:

```rust
#[cfg(target_os = "android")]
#[unsafe(no_mangle)]
pub fn android_main(app: android_activity::AndroidApp) {
    platform::android::init(app.clone());
    if let Err(error) = application().run_android(app) {
        log::error!("The application stopped: {error}");
    }
}
```

It keeps the Rust ABI: `android-activity`'s glue declares it as
`extern "Rust" { fn android_main(app: AndroidApp); }`.

`android-activity = { features = ["native-activity"] }` is added as a direct
dependency on Android. This is what turns on winit's Android backend: winit's
own feature is just `android-activity/native-activity`, and Cargo unifies
features across the graph.

## What the first pass changes

| Area | Change |
|---|---|
| Crates | `groundhog-gui-iced` gains a library next to its binary; all modules move from `main.rs` into `lib.rs`. It is an `rlib` in the manifest, and `build.sh` asks for the `cdylib` (`cargo rustc --crate-type cdylib`), so desktop builds link no shared object |
| Entry | `src/main.rs` calls `groundhog_gui_iced::run()`; `android_main` starts the same `Application` through `run_android` |
| Paths | new `platform` module: desktop uses `directories`, Android uses `AndroidApp::internal_data_path()` (`<files>/config/config.ron`, `<files>/data/tlogs`) |
| Logging | desktop keeps `env_logger`; Android uses `android_logger` (`adb logcat -s groundhog`) plus a panic hook, so a panic is logged instead of vanishing into stderr |
| Files | `rfd` is desktop-only; the Android shim returns "cancelled" for pick/save |
| Fonts | the iced fork loads `/system/fonts`, which Android needs: cosmic-text finds fonts through fontconfig, which Android does not have |
| Tiles | the `slippery` fork gives Android a `rustls` client with Mozilla's bundled roots, because `reqwest`'s default Android verifier panics without a JVM-side component |
| Lifecycle | the iced fork creates the window on `Resumed` and recreates the wgpu surface there, because Android destroys the native window when the app is backgrounded |
| Packaging | `android/` holds the manifest and a Gradle-free build script: `cargo rustc`, `aapt2 link`, `zip`, `zipalign`, `apksigner` |

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
library as a shared object
(`cargo rustc --release --lib -p groundhog-gui-iced --crate-type cdylib`), links the
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
compiled activity subclass, so the insets are polled every 150 ms; the
subscription only publishes a message when the values change, so an idle
application is not redrawn (`dumpsys cpuinfo` reports 0% while idle). Each
read is posted to the Java main thread, which owns the view hierarchy, and
runs in its own JNI local frame, because that thread is never detached and
would otherwise keep every local reference.

The status- and navigation-bar icon colour follows the app theme through
`WindowInsetsController.setSystemBarsAppearance`, posted to the Java main
thread because the controller belongs to that thread.

Verified on the Pixel 7: in portrait and landscape the content starts below the
status bar and above the navigation bar, and the soft keyboard pushes the
content up while it is open.

### Pass 2 — phone layout (done)

A phone gets the floating shell (see "Two shells over the same panels" in
AGENTS.md): the map fills the screen, under the status and navigation bars,
and a navigation bar picks one panel to show in a sheet over it — along the
bottom in portrait, along the side in landscape. The tiled workspace stays
for tablets and the desktop, and either can be chosen in the settings.

The shell is chosen by the screen's shortest side, so rotating the phone
keeps the shell and the open sheet. (A first attempt switched to a
single-panel mode below 640 pixels of width, which gave a phone in
landscape the whole three-column workspace.)

The insets are not padding around the window any more: the floating shell
lets the map run under the bars and keeps only what floats clear of them,
and the map is told how much of each edge is covered, so the attitude
instrument clears the status bar and a followed vehicle stays in the part
of the map that shows.

Floating panels capture the presses that land on them (`widget::surface`):
the map follows a finger by its own position, so a press on a sheet would
otherwise pan the map beneath it.

### Pass 3 — touch (done)

The map overlay's pointer state machine treats a finger like the mouse, with
two differences that matter on a phone: a larger slop before a press turns
into a drag (`TOUCH_DRAG_THRESHOLD`), and a larger hit area around every
handle (`TOUCH_TARGET_RADIUS`, a 44 logical pixel target), so a mission
marker or a vehicle can be tapped without precision. A press on empty map is
still not captured, so the tiles below keep panning and pinching, and its tap
becomes a click — which is how the mission editor places a waypoint. A finger
the system takes away cancels the drag in progress instead of leaving it
stuck.

Only the first finger down drives the overlay. A second one makes the
gesture a pinch: a press waiting to become a click is handed to the map (so
a two-finger tap places no waypoint), and a drag in progress keeps its
handle, the second finger captured so the map does not pan under it.

The rail's drag source handles touch as well, so panels can be dragged onto
the grid of the tiled shell on a tablet: the grid drops them where the finger
is lifted, and the label of what is dragged is drawn a touch target's height
above the finger, where the finger does not cover it. A finger the system
takes away drops nothing, from the rail or from a pane's title bar. The
floating shell's navigation bar is tap-only.

Verified on the Pixel 7: a swipe pans the map (the overlay does not steal the
gesture), and the widget tests cover the finger paths for both the overlay and
the drag source.

## Known limitations

- Pick/save file actions do nothing on Android.
- Serial links cannot open; UDP/TCP work (INTERNET permission is declared).
- Soft keyboard text entry is basic (key events); IME composition, clipboard
  and long-press selection are not implemented in the minimal fork.
- Tooltips are off on touch screens (a tap would open one that lingers), so
  icon buttons explain themselves only by their icon there.
- Back is taken as the back key (`enableOnBackInvokedCallback="false"` in
  the manifest), so the system's predictive-back animation does not play.
- No foreground service: telemetry stops if Android freezes the process in the
  background.
- The release library is not stripped, so the debug APK is ~34 MB; a
  distribution build should strip the `.so` (and split per ABI).
- iced comes from a fork branch (`groundhog-compat`) until upstream iced
  supports Android; each update of iced means rebasing that branch.

## Verification

1. `cargo rustc --release --lib -p groundhog-gui-iced --target aarch64-linux-android --crate-type cdylib`
   succeeds. ✅
2. `android/out/groundhog-debug.apk` passes `apksigner verify` and contains
   `lib/arm64-v8a/libgroundhog_gui_iced.so` plus a binary manifest. ✅
3. `adb install` succeeds and the activity starts. ✅ (Pixel 7, Android 16;
   the map loads OpenStreetMap tiles, the UI draws, and the insets keep the
   content clear of the system bars and the keyboard.)
4. Host gates still pass: `cargo test --workspace`,
   `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`.
   The Android target also passes `cargo clippy --target aarch64-linux-android
   -p groundhog-gui-iced --lib -- -D warnings`. ✅

## Later passes

1. SAF import/export for missions and parameters, USB serial through the
   Android USB Host API, and a foreground service for background
   telemetry/recording.
2. Full IME/clipboard by porting the `iced_android` Java activity and the
   remaining iced widget patches.
