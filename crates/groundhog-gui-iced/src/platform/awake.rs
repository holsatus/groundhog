//! Keeping the screen on and the computer awake while the application runs,
//! on the desktop.
//!
//! An operator watches telemetry and video for long stretches without
//! touching the keyboard or mouse, and a screen that blanks, or a computer
//! that suspends, mid-flight hides the vehicle. Each platform has its own
//! request:
//!
//! - **Linux** asks over the session bus: the XDG desktop portal first
//!   (GNOME, KDE and sandboxes such as Flatpak implement it), then GNOME's
//!   session manager directly, then `org.freedesktop.ScreenSaver` (KDE,
//!   Xfce, Cinnamon and others). Each releases the inhibition when its
//!   caller leaves the bus, so the connection is kept for the life of the
//!   process, and quitting (or crashing) gives the screen back without any
//!   cleanup.
//! - **Windows** marks the main thread as needing the display and the
//!   system (`SetThreadExecutionState`), which holds until the thread ends.
//! - **macOS** begins an `NSProcessInfo` activity that disables idle display
//!   and system sleep, kept until the process ends.
//!
//! None of these stops a sleep the user asks for (closing a laptop's lid).
//! Failing to keep the session awake is logged, since the application works
//! without it.
//!
//! On Windows and macOS this module uses nothing of the rest of the
//! frontend, so it can be checked on its own for the platforms CI does not
//! build.

/// Why the session is kept awake, where the desktop shows it to the user.
#[cfg(any(target_os = "linux", target_os = "macos"))]
const REASON: &str = "Keeping telemetry and video alive";

/// Keeps the screen on and the computer awake while the application runs.
///
/// Call it once, from the main thread: on Windows the request belongs to the
/// thread that makes it, and on macOS to the thread that holds it. The
/// main thread runs the event loop until the process ends.
#[cfg(target_os = "linux")]
pub fn keep_awake() {
    // The session bus may be slow to answer, or absent; neither should hold
    // up the window.
    let spawned = std::thread::Builder::new()
        .name("keep-awake".into())
        .spawn(|| match linux::keep_awake() {
            Ok(via) => log::info!("Keeping the screen on and the computer awake ({via})"),
            Err(reasons) => log::warn!("Could not keep the screen on: {reasons}"),
        });

    if let Err(error) = spawned {
        log::warn!("Could not keep the screen on: {error}");
    }
}

/// Keeps the screen on and the computer awake while the application runs.
///
/// Call it once, from the main thread: on Windows the request belongs to the
/// thread that makes it, and on macOS to the thread that holds it. The
/// main thread runs the event loop until the process ends.
#[cfg(windows)]
pub fn keep_awake() {
    use windows_sys::Win32::System::Power::{
        ES_CONTINUOUS, ES_DISPLAY_REQUIRED, ES_SYSTEM_REQUIRED, SetThreadExecutionState,
    };

    // `ES_CONTINUOUS` keeps the state until the thread ends or sets another;
    // without it the call would only reset the idle timers once.
    #[expect(
        unsafe_code,
        reason = "a Win32 call; Windows has no safe binding for it"
    )]
    // SAFETY: the function takes its flags by value and touches no memory of
    // the caller's.
    let previous = unsafe {
        SetThreadExecutionState(ES_CONTINUOUS | ES_DISPLAY_REQUIRED | ES_SYSTEM_REQUIRED)
    };

    // The previous state, or zero if Windows refused (it sets no error code).
    if previous == 0 {
        log::warn!("Could not keep the screen on: Windows refused the execution state");
    } else {
        log::info!("Keeping the screen on and the computer awake");
    }
}

/// Keeps the screen on and the computer awake while the application runs.
///
/// Call it once, from the main thread: on Windows the request belongs to the
/// thread that makes it, and on macOS to the thread that holds it. The
/// main thread runs the event loop until the process ends.
#[cfg(target_os = "macos")]
pub fn keep_awake() {
    use std::cell::OnceCell;

    use objc2::{rc::Retained, runtime::ProtocolObject};
    use objc2_foundation::{NSActivityOptions, NSObjectProtocol, NSProcessInfo, NSString};

    thread_local! {
        /// The activity, which lasts while its token is held: on the main
        /// thread, until the process ends.
        static ACTIVITY: OnceCell<Retained<ProtocolObject<dyn NSObjectProtocol>>> =
            const { OnceCell::new() };
    }

    let activity = NSProcessInfo::processInfo().beginActivityWithOptions_reason(
        NSActivityOptions::IdleDisplaySleepDisabled | NSActivityOptions::IdleSystemSleepDisabled,
        &NSString::from_str(REASON),
    );
    ACTIVITY.with(|held| drop(held.set(activity)));

    log::info!("Keeping the screen on and the computer awake");
}

/// No implementation here yet; the platform's power settings apply.
#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
pub fn keep_awake() {}

#[cfg(target_os = "linux")]
mod linux {
    use std::{collections::HashMap, sync::OnceLock};

    use zbus::{blocking::Connection, zvariant::Value};

    use super::REASON;
    use crate::platform::APP_ID;

    /// The session-bus connection that holds the inhibition, for the life of
    /// the process.
    static HELD: OnceLock<Connection> = OnceLock::new();

    /// Inhibits idling and suspend through the first interface that takes
    /// the request, and keeps the connection that holds it.
    ///
    /// Returns which interface took it, or every interface's reason for not.
    pub(super) fn keep_awake() -> Result<&'static str, String> {
        let connection = Connection::session().map_err(|error| error.to_string())?;
        let via = inhibit(&connection)?;
        drop(HELD.set(connection));
        Ok(via)
    }

    /// The `Inhibit` flags of both the portal and GNOME's session manager:
    /// `4` suspending, `8` the session going idle (which blanks and locks
    /// the screen).
    const SUSPEND_AND_IDLE: u32 = 4 | 8;

    /// Asks one interface to inhibit idling.
    type Inhibitor = fn(&Connection) -> zbus::Result<()>;

    /// The interfaces asked, in order, and how each is asked.
    const INHIBITORS: [(&str, Inhibitor); 3] = [
        ("desktop portal", portal),
        ("GNOME session", gnome_session),
        ("screen saver", screen_saver),
    ];

    /// Asks each interface in turn until one inhibits idling, and returns
    /// which did, or every interface's reason for not.
    pub(super) fn inhibit(connection: &Connection) -> Result<&'static str, String> {
        let mut refusals = Vec::new();

        for (name, inhibit) in INHIBITORS {
            match inhibit(connection) {
                Ok(()) => return Ok(name),
                Err(error) => refusals.push(format!("{name}: {error}")),
            }
        }

        Err(refusals.join("; "))
    }

    /// `org.freedesktop.portal.Inhibit.Inhibit`, after naming the
    /// application to the portal.
    ///
    /// No parent window: the identifier only places a dialog the portal
    /// might show, and an inhibition needs none.
    fn portal(connection: &Connection) -> zbus::Result<()> {
        // The desktop shows the name with the inhibition ("Groundhog is
        // blocking sleep"). Only an application installed on the host, with
        // its `.desktop` file, can register; a Flatpak is known by its ID
        // already, and one run from a build tree stays nameless. Either
        // way the inhibition goes ahead.
        if let Err(error) = register(connection) {
            log::debug!("The desktop portal does not know this application: {error}");
        }

        let options = HashMap::from([("reason", Value::from(REASON))]);

        connection.call_method(
            Some("org.freedesktop.portal.Desktop"),
            "/org/freedesktop/portal/desktop",
            Some("org.freedesktop.portal.Inhibit"),
            "Inhibit",
            &("", SUSPEND_AND_IDLE, options),
        )?;
        Ok(())
    }

    /// `org.freedesktop.host.portal.Registry.Register`: tells the portal
    /// which application this is. It must come before any other portal call
    /// on the connection.
    fn register(connection: &Connection) -> zbus::Result<()> {
        connection.call_method(
            Some("org.freedesktop.portal.Desktop"),
            "/org/freedesktop/portal/desktop",
            Some("org.freedesktop.host.portal.Registry"),
            "Register",
            &(APP_ID, HashMap::<&str, Value<'_>>::new()),
        )?;
        Ok(())
    }

    /// `org.gnome.SessionManager.Inhibit`, for GNOME without a portal.
    ///
    /// The cookie it returns would only end the inhibition early, which
    /// nothing does; the same holds for the screen saver's.
    fn gnome_session(connection: &Connection) -> zbus::Result<()> {
        // No toplevel window (an X11 window id, which Wayland has none of).
        connection.call_method(
            Some("org.gnome.SessionManager"),
            "/org/gnome/SessionManager",
            Some("org.gnome.SessionManager"),
            "Inhibit",
            &(APP_ID, 0_u32, REASON, SUSPEND_AND_IDLE),
        )?;
        Ok(())
    }

    /// `org.freedesktop.ScreenSaver.Inhibit`: idling (blanking and locking
    /// the screen) only; KDE, for one, still suspends. It takes a name to
    /// show rather than an application ID.
    fn screen_saver(connection: &Connection) -> zbus::Result<()> {
        connection.call_method(
            Some("org.freedesktop.ScreenSaver"),
            "/org/freedesktop/ScreenSaver",
            Some("org.freedesktop.ScreenSaver"),
            "Inhibit",
            &("Groundhog", REASON),
        )?;
        Ok(())
    }

    #[cfg(test)]
    mod tests {
        //! A peer-to-peer connection to mock desktop services: no bus, so
        //! nothing reaches the user's session.

        use std::{
            collections::HashMap,
            os::unix::net::UnixStream,
            sync::{Arc, Mutex},
            time::Duration,
        };

        use zbus::{
            Guid,
            blocking::{Connection, connection::Builder},
            zvariant::{OwnedObjectPath, OwnedValue},
        };

        use super::{APP_ID, REASON, SUSPEND_AND_IDLE, inhibit};

        /// What the mocks were asked, in order.
        type Calls = Arc<Mutex<Vec<String>>>;

        struct Portal(Calls);

        #[zbus::interface(name = "org.freedesktop.portal.Inhibit")]
        impl Portal {
            fn inhibit(
                &self,
                _window: &str,
                flags: u32,
                options: HashMap<String, OwnedValue>,
            ) -> OwnedObjectPath {
                let reason = options
                    .get("reason")
                    .and_then(|reason| String::try_from(reason.clone()).ok());
                self.0
                    .lock()
                    .unwrap()
                    .push(format!("portal {flags} {reason:?}"));
                OwnedObjectPath::try_from("/org/freedesktop/portal/desktop/request/1/t").unwrap()
            }
        }

        struct Registry(Calls);

        #[zbus::interface(name = "org.freedesktop.host.portal.Registry")]
        impl Registry {
            fn register(&self, app_id: &str, _options: HashMap<String, OwnedValue>) {
                self.0.lock().unwrap().push(format!("register {app_id}"));
            }
        }

        struct GnomeSession(Calls);

        #[zbus::interface(name = "org.gnome.SessionManager")]
        impl GnomeSession {
            fn inhibit(&self, app_id: &str, _toplevel_xid: u32, reason: &str, flags: u32) -> u32 {
                self.0
                    .lock()
                    .unwrap()
                    .push(format!("gnome {app_id} {flags} {reason}"));
                1
            }
        }

        /// An interface no inhibitor uses.
        struct Nothing;

        #[zbus::interface(name = "org.groundhog.Nothing")]
        impl Nothing {}

        struct ScreenSaver(Calls);

        #[zbus::interface(name = "org.freedesktop.ScreenSaver")]
        impl ScreenSaver {
            fn inhibit(&self, _application_name: &str, reason: &str) -> u32 {
                self.0
                    .lock()
                    .unwrap()
                    .push(format!("screen saver {reason}"));
                1
            }
        }

        /// Which mock services the desktop offers.
        #[derive(Clone, Copy)]
        struct Desktop {
            portal: bool,
            /// Whether the portal can name a host application (a newer
            /// portal, and an installed `.desktop` file).
            registry: bool,
            gnome: bool,
            screen_saver: bool,
        }

        /// Connects to a desktop serving the given mocks, and returns the
        /// connection, the server's (which must stay open), and the calls.
        fn desktop(offers: Desktop) -> (Connection, Connection, Calls) {
            let calls = Calls::default();
            let (ours, theirs) = UnixStream::pair().unwrap();

            let served = calls.clone();
            let server = std::thread::spawn(move || {
                // Something is always served, so the server answers a call
                // for anything else with an error, as a desktop's bus does;
                // without an object server it would not answer at all.
                let mut builder = Builder::async_io_unix_stream(theirs)
                    .server(Guid::generate())
                    .unwrap()
                    .p2p()
                    .serve_at("/", Nothing)
                    .unwrap();
                if offers.portal {
                    builder = builder
                        .serve_at("/org/freedesktop/portal/desktop", Portal(served.clone()))
                        .unwrap();
                }
                if offers.registry {
                    builder = builder
                        .serve_at("/org/freedesktop/portal/desktop", Registry(served.clone()))
                        .unwrap();
                }
                if offers.gnome {
                    builder = builder
                        .serve_at("/org/gnome/SessionManager", GnomeSession(served.clone()))
                        .unwrap();
                }
                if offers.screen_saver {
                    builder = builder
                        .serve_at("/org/freedesktop/ScreenSaver", ScreenSaver(served))
                        .unwrap();
                }
                builder.build().unwrap()
            });

            // A mistake in the mocks then fails the test rather than hanging it.
            let client = Builder::async_io_unix_stream(ours)
                .p2p()
                .method_timeout(Duration::from_secs(5))
                .build()
                .unwrap();
            (client, server.join().unwrap(), calls)
        }

        fn calls(calls: &Calls) -> Vec<String> {
            calls.lock().unwrap().clone()
        }

        #[test]
        fn the_portal_is_told_who_asks_then_asked_first_for_idling_and_suspend() {
            let (client, _server, made) = desktop(Desktop {
                portal: true,
                registry: true,
                gnome: true,
                screen_saver: true,
            });

            assert_eq!(inhibit(&client), Ok("desktop portal"));
            assert_eq!(
                calls(&made),
                [
                    format!("register {APP_ID}"),
                    format!("portal {SUSPEND_AND_IDLE} Some({REASON:?})"),
                ]
            );
        }

        #[test]
        fn a_portal_that_cannot_name_the_application_still_inhibits() {
            let (client, _server, made) = desktop(Desktop {
                portal: true,
                registry: false,
                gnome: true,
                screen_saver: true,
            });

            assert_eq!(inhibit(&client), Ok("desktop portal"));
            assert_eq!(
                calls(&made),
                [format!("portal {SUSPEND_AND_IDLE} Some({REASON:?})")]
            );
        }

        #[test]
        fn gnome_without_a_portal_is_asked_directly() {
            let (client, _server, made) = desktop(Desktop {
                portal: false,
                registry: false,
                gnome: true,
                screen_saver: true,
            });

            assert_eq!(inhibit(&client), Ok("GNOME session"));
            assert_eq!(
                calls(&made),
                [format!("gnome {APP_ID} {SUSPEND_AND_IDLE} {REASON}")]
            );
        }

        #[test]
        fn the_screen_saver_is_the_last_resort() {
            let (client, _server, made) = desktop(Desktop {
                portal: false,
                registry: false,
                gnome: false,
                screen_saver: true,
            });

            assert_eq!(inhibit(&client), Ok("screen saver"));
            assert_eq!(calls(&made), [format!("screen saver {REASON}")]);
        }

        #[test]
        fn a_desktop_with_none_says_why_for_each() {
            let (client, _server, _) = desktop(Desktop {
                portal: false,
                registry: false,
                gnome: false,
                screen_saver: false,
            });

            let refusals = inhibit(&client).unwrap_err();

            for name in ["desktop portal", "GNOME session", "screen saver"] {
                assert!(refusals.contains(name), "{refusals}");
            }
        }
    }
}
