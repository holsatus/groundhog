//! Android paths, logging, the application handle, and the window insets.
//!
//! Android gives the app no `$HOME` to derive directories from, app `stderr`
//! goes nowhere, and since Android 15 the system draws the status bar, the
//! display cutout, the navigation bar and the soft keyboard *over* the
//! application. All of that comes from the [`AndroidApp`] that `android_main`
//! receives.
//!
//! This is the one module allowed `unsafe`: Java objects cross here as raw JNI
//! pointers that only this glue knows how to pair with their `JavaVM`.

#![expect(
    unsafe_code,
    reason = "JNI hands raw pointers that only this module can pair with their VM"
)]

use std::{
    path::PathBuf,
    sync::{
        Mutex, OnceLock,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use android_activity::AndroidApp;
use jni::{
    JavaVM,
    objects::{JObject, JValue},
};
use log::LevelFilter;

use super::Insets;

/// The handle Android gives `android_main`, kept for the parts of the
/// application that need platform APIs after boot (the data paths and the
/// window insets).
static APP: OnceLock<AndroidApp> = OnceLock::new();

/// How often the window insets are read.
///
/// The system reports them only to a Java `View.OnApplyWindowInsetsListener`,
/// which a `NativeActivity` does not have, so the cheapest way to notice the
/// keyboard, a rotation or a system-bar change is to ask again. The
/// subscription below only publishes a message when the values actually
/// change, so an idle application does not redraw.
const INSETS_POLL: Duration = Duration::from_millis(150);

/// Whether reading the insets has already failed, so a broken JNI call does
/// not log on every poll.
static INSETS_FAILED: AtomicBool = AtomicBool::new(false);

/// The insets last read on the Java main thread.
static READ_INSETS: Mutex<Insets> = Mutex::new(Insets::ZERO);

/// The insets last published, so only a change becomes a message.
static LAST_INSETS: Mutex<Insets> = Mutex::new(Insets::ZERO);

/// Prepares logging and remembers the application handle.
///
/// Called first thing in `android_main`, before iced starts.
pub fn init(app: AndroidApp) {
    drop(APP.set(app));

    android_logger::init_once(
        android_logger::Config::default()
            .with_tag("groundhog")
            // `log`'s runtime maximum starts at `Off`; `with_max_level` raises
            // it. The filter then drops the noise from dependencies.
            .with_max_level(LevelFilter::Debug)
            .with_filter(
                android_logger::FilterBuilder::new()
                    .filter_level(LevelFilter::Error)
                    .filter_module("groundhog_logic", LevelFilter::Debug)
                    .filter_module("groundhog_gui_iced", LevelFilter::Debug)
                    .build(),
            ),
    );

    // A Rust panic otherwise writes to stderr, which Android discards; log it
    // so `adb logcat -s groundhog` shows what went wrong.
    std::panic::set_hook(Box::new(|info| {
        log::error!("panic: {info}");
    }));
}

/// Where the configuration is written: a private directory inside the app's
/// internal storage.
pub fn config_dir() -> Option<PathBuf> {
    APP.get()?
        .internal_data_path()
        .map(|dir| dir.join("config"))
}

/// Where application data (telemetry logs) is kept.
pub fn data_dir() -> Option<PathBuf> {
    APP.get()?.internal_data_path().map(|dir| dir.join("data"))
}

/// The window insets in logical pixels, as last read.
///
/// Reading them is posted to the Java main thread, which owns the view
/// hierarchy (`getRootWindowInsets` computes and caches on first use, so it is
/// not safe to call from another thread); this returns what the last read
/// found, and asks for a new one.
pub fn insets() -> Insets {
    let Some(app) = APP.get() else {
        return Insets::ZERO;
    };

    let posted = app.clone();
    app.run_on_java_main_thread(Box::new(move || {
        let insets = match read_insets(&posted) {
            Ok(physical) => physical.to_logical(scale(&posted)),
            Err(error) => {
                if !INSETS_FAILED.swap(true, Ordering::Relaxed) {
                    log::warn!("Could not read the window insets: {error}");
                }
                Insets::ZERO
            }
        };
        *lock(&READ_INSETS) = insets;
    }));

    *lock(&READ_INSETS)
}

/// Every change of the window insets.
///
/// The poll itself is cheap (one JNI query on the Java main thread); only a
/// change becomes a message, so an idle application is not redrawn. A change
/// shows at most two polls after it happens, because each poll publishes the
/// previous poll's read. The last value is process state rather than a
/// captured variable because iced's `map` and `filter_map` closures may not
/// capture.
pub fn insets_subscription() -> iced::Subscription<Insets> {
    iced::time::every(INSETS_POLL).filter_map(|_| {
        let now = insets();
        let mut last = lock(&LAST_INSETS);

        if *last == now {
            None
        } else {
            *last = now;
            Some(now)
        }
    })
}

/// `mutex`, whether or not a panic poisoned it: the insets are plain values.
fn lock(mutex: &Mutex<Insets>) -> std::sync::MutexGuard<'_, Insets> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Chooses light or dark status- and navigation-bar icons to match the
/// application's theme.
pub fn set_system_bars(dark: bool) {
    let Some(app) = APP.get() else {
        return;
    };

    // Before API 30 the appearance is set with deprecated view flags that
    // this build does not use; the system's default is good enough there.
    if AndroidApp::sdk_version() < 30 {
        return;
    }

    // `WindowInsetsController` belongs to the activity's view hierarchy, so
    // the call is posted to the Java main thread; `android_main` runs on its
    // own thread.
    let app = app.clone();
    let posted = app.clone();
    app.run_on_java_main_thread(Box::new(move || {
        if let Err(error) = apply_system_bars(&posted, dark) {
            log::warn!("Could not style the system bars: {error}");
        }
    }));
}

/// Sends the application to the background, as back from an app's first
/// screen does. (winit takes every back key as handled, so the system never
/// does this itself.) The process keeps running, and with it the links.
pub fn leave() {
    let Some(app) = APP.get() else {
        return;
    };

    let posted = app.clone();
    app.run_on_java_main_thread(Box::new(move || {
        if let Err(error) = move_task_to_back(&posted) {
            log::warn!("Could not leave the application: {error}");
        }
    }));
}

/// Calls `Activity.moveTaskToBack(true)`.
fn move_task_to_back(app: &AndroidApp) -> jni::errors::Result<()> {
    // SAFETY: the VM and activity belong to the app that `android_main`
    // received, and outlive the call.
    let vm = unsafe { JavaVM::from_raw(app.vm_as_ptr().cast()) }?;
    let mut env = vm.attach_current_thread()?;
    let activity = unsafe { JObject::from_raw(app.activity_as_ptr().cast()) };

    env.call_method(&activity, "moveTaskToBack", "(Z)Z", &[JValue::Bool(1)])?;
    Ok(())
}

/// Sets `WindowInsetsController.setSystemBarsAppearance` on the window.
fn apply_system_bars(app: &AndroidApp, dark: bool) -> jni::errors::Result<()> {
    // `APPEARANCE_LIGHT_STATUS_BARS` and `APPEARANCE_LIGHT_NAVIGATION_BARS`:
    // "light bars" means dark icons.
    const LIGHT_BARS: i32 = 8 | 16;
    let appearance = if dark { 0 } else { LIGHT_BARS };

    // SAFETY: the VM and activity belong to the app that `android_main`
    // received, and outlive the call.
    let vm = unsafe { JavaVM::from_raw(app.vm_as_ptr().cast()) }?;
    let mut env = vm.attach_current_thread()?;
    let activity = unsafe { JObject::from_raw(app.activity_as_ptr().cast()) };

    // The Java main thread is never detached, so its local references live
    // until the frame they were made in is popped.
    env.with_local_frame(8, |env| {
        let window = env
            .call_method(&activity, "getWindow", "()Landroid/view/Window;", &[])?
            .l()?;
        let controller = env
            .call_method(
                &window,
                "getInsetsController",
                "()Landroid/view/WindowInsetsController;",
                &[],
            )?
            .l()?;

        env.call_method(
            &controller,
            "setSystemBarsAppearance",
            "(II)V",
            &[JValue::Int(appearance), JValue::Int(LIGHT_BARS)],
        )?;

        Ok(())
    })
}

/// The density the system uses, as a scale factor: 160 dpi is 1.0.
fn scale(app: &AndroidApp) -> f32 {
    app.config()
        .density()
        .filter(|dpi| *dpi > 0)
        .map_or(1.0, |dpi| dpi as f32 / 160.0)
}

/// The insets as Android reports them, in physical pixels.
#[derive(Debug, Clone, Copy, Default)]
struct PhysicalInsets {
    top: i32,
    right: i32,
    bottom: i32,
    left: i32,
}

impl PhysicalInsets {
    fn to_logical(self, scale: f32) -> Insets {
        Insets {
            top: self.top as f32 / scale,
            right: self.right as f32 / scale,
            bottom: self.bottom as f32 / scale,
            left: self.left as f32 / scale,
        }
    }
}

/// Reads `Window.getDecorView().getRootWindowInsets()` through the JVM.
///
/// Must run on the Java main thread, which owns the view hierarchy.
fn read_insets(app: &AndroidApp) -> jni::errors::Result<PhysicalInsets> {
    // SAFETY: the VM and the activity belong to the app that `android_main`
    // received, and outlive the call.
    let vm = unsafe { JavaVM::from_raw(app.vm_as_ptr().cast()) }?;
    let mut env = vm.attach_current_thread()?;
    let activity = unsafe { JObject::from_raw(app.activity_as_ptr().cast()) };

    // The Java main thread is never detached, so its local references live
    // until the frame they were made in is popped.
    env.with_local_frame(16, |env| read_insets_in(env, &activity))
}

fn read_insets_in(
    env: &mut jni::JNIEnv<'_>,
    activity: &JObject<'_>,
) -> jni::errors::Result<PhysicalInsets> {
    let window = env
        .call_method(activity, "getWindow", "()Landroid/view/Window;", &[])?
        .l()?;
    let decor = env
        .call_method(&window, "getDecorView", "()Landroid/view/View;", &[])?
        .l()?;
    let insets = env
        .call_method(
            &decor,
            "getRootWindowInsets",
            "()Landroid/view/WindowInsets;",
            &[],
        )?
        .l()?;

    // Before the first layout pass the window has no insets yet.
    if insets.is_null() {
        return Ok(PhysicalInsets::default());
    }

    if AndroidApp::sdk_version() >= 30 {
        // `WindowInsets.Type.systemBars()` is status, navigation and caption
        // bars; `displayCutout()` covers the camera cutout. `ime()` is the
        // soft keyboard.
        const SYSTEM_BARS_AND_CUTOUT: i32 = 7 | 128;
        const IME: i32 = 8;

        let bars = env
            .call_method(
                &insets,
                "getInsets",
                "(I)Landroid/graphics/Insets;",
                &[JValue::Int(SYSTEM_BARS_AND_CUTOUT)],
            )?
            .l()?;
        let ime = env
            .call_method(
                &insets,
                "getInsets",
                "(I)Landroid/graphics/Insets;",
                &[JValue::Int(IME)],
            )?
            .l()?;

        Ok(PhysicalInsets {
            top: env.get_field(&bars, "top", "I")?.i()?,
            right: env.get_field(&bars, "right", "I")?.i()?,
            left: env.get_field(&bars, "left", "I")?.i()?,
            bottom: env
                .get_field(&bars, "bottom", "I")?
                .i()?
                .max(env.get_field(&ime, "bottom", "I")?.i()?),
        })
    } else {
        // Before API 30 the same values are the deprecated system-window
        // insets; the bottom one includes the soft keyboard.
        Ok(PhysicalInsets {
            top: env
                .call_method(&insets, "getSystemWindowInsetTop", "()I", &[])?
                .i()?,
            right: env
                .call_method(&insets, "getSystemWindowInsetRight", "()I", &[])?
                .i()?,
            bottom: env
                .call_method(&insets, "getSystemWindowInsetBottom", "()I", &[])?
                .i()?,
            left: env
                .call_method(&insets, "getSystemWindowInsetLeft", "()I", &[])?
                .i()?,
        })
    }
}
