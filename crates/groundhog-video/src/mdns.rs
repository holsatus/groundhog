//! Finding a `.local` host, such as `camera.local`, through Avahi.
//!
//! Cameras and companion computers usually announce themselves over
//! multicast DNS. A Linux host resolves those names through Avahi, which
//! the system's resolver asks for them (`nss-mdns`), and GStreamer asks the
//! system's resolver. A Flatpak's runtime has no `nss-mdns`, though, so in
//! the sandbox a `.local` name would not resolve at all. Asking Avahi over
//! the system bus, as the host's resolver would, gives the sandbox the same
//! answer the host gets. It is done everywhere on Linux, so a source
//! behaves the same in and out of the sandbox.
//!
//! Only `.local` names are looked up, and only an IPv4 address is taken:
//! cameras are addressed over IPv4, and a link-local IPv6 address would
//! need its interface in the URI. Where Avahi cannot be asked (no system
//! bus, no daemon, a sandbox not allowed to talk to it), the name is left
//! to GStreamer, so a host that resolves `.local` some other way (such as
//! systemd-resolved) still can.
//!
//! Windows and macOS resolve `.local` names themselves.

use std::net::Ipv4Addr;

use zbus::blocking::Connection;

use crate::source::Source;

/// `source`, with a `.local` host replaced by the address Avahi has for it.
///
/// Blocks until Avahi answers, which for a name nothing answers to takes a
/// few seconds.
///
/// # Errors
///
/// What to tell the operator when nothing on the network answers to the
/// host's name; worth trying again, as the camera may not be up yet.
pub(crate) fn resolve(source: &Source) -> Result<Source, String> {
    if host(source).is_none_or(|host| !is_mdns(host)) {
        return Ok(source.clone());
    }

    match Connection::system() {
        Ok(connection) => resolve_on(&connection, source),
        Err(error) => {
            log::debug!("Video: the system bus is unavailable, so Avahi is not asked: {error}");
            Ok(source.clone())
        }
    }
}

/// [`resolve`], asking Avahi over `connection`.
fn resolve_on(connection: &Connection, source: &Source) -> Result<Source, String> {
    let Some(host) = host(source).filter(|host| is_mdns(host)) else {
        return Ok(source.clone());
    };

    match lookup(connection, host) {
        Lookup::Found(address) => {
            log::debug!("Video: {host} is {address}");
            Ok(with_host(source, &address.to_string()))
        }
        Lookup::NotFound => Err(format!("Nothing on the network answers to {host}")),
        Lookup::Unavailable(reason) => {
            log::debug!("Video: Avahi could not be asked for {host}: {reason}");
            Ok(source.clone())
        }
    }
}

/// What Avahi said about a name.
#[derive(Debug, PartialEq)]
enum Lookup {
    Found(Ipv4Addr),
    /// No device answered to it.
    NotFound,
    /// Avahi could not be asked, or gave an answer that is no use.
    Unavailable(String),
}

/// `org.freedesktop.Avahi.Server.ResolveHostName`, for an IPv4 address.
fn lookup(connection: &Connection, host: &str) -> Lookup {
    /// Avahi's `AVAHI_IF_UNSPEC` and `AVAHI_PROTO_UNSPEC`: ask on every
    /// interface, over IPv4 and IPv6 alike.
    const UNSPECIFIED: i32 = -1;
    /// Avahi's `AVAHI_PROTO_INET`: the address wanted is IPv4.
    const IPV4: i32 = 0;

    let reply = connection.call_method(
        Some("org.freedesktop.Avahi"),
        "/",
        Some("org.freedesktop.Avahi.Server"),
        "ResolveHostName",
        &(UNSPECIFIED, UNSPECIFIED, host, IPV4, 0_u32),
    );

    let message = match reply {
        Ok(message) => message,
        // Avahi gives up on a name nothing answers to after a few seconds.
        Err(zbus::Error::MethodError(name, _, _))
            if matches!(
                name.as_str(),
                "org.freedesktop.Avahi.TimeoutError" | "org.freedesktop.Avahi.NotFoundError"
            ) =>
        {
            return Lookup::NotFound;
        }
        Err(error) => return Lookup::Unavailable(error.to_string()),
    };

    // Interface, protocol, name, address protocol, address, flags.
    match message
        .body()
        .deserialize::<(i32, i32, String, i32, String, u32)>()
    {
        Ok((_, _, _, _, address, _)) => match address.parse() {
            Ok(address) => Lookup::Found(address),
            Err(_) => Lookup::Unavailable(format!("{address} is not an IPv4 address")),
        },
        Err(error) => Lookup::Unavailable(error.to_string()),
    }
}

/// Whether `host` is a multicast DNS name.
fn is_mdns(host: &str) -> bool {
    host.trim_end_matches('.')
        .to_ascii_lowercase()
        .ends_with(".local")
}

/// The host a source connects to: an RTSP server, or a TCP stream's
/// sender. A UDP source names an address to listen on, which is local.
fn host(source: &Source) -> Option<&str> {
    match source {
        Source::Rtsp { location } => Some(Authority::of(location)?.host),
        Source::MpegTsTcp { host, .. } => Some(host),
        Source::Rtp { .. } | Source::MpegTsUdp { .. } | Source::TestPattern => None,
    }
}

/// `source` with its host replaced by `address`.
fn with_host(source: &Source, address: &str) -> Source {
    match source {
        Source::Rtsp { location } => match Authority::of(location) {
            Some(authority) => Source::Rtsp {
                location: authority.with_host(address),
            },
            None => source.clone(),
        },
        Source::MpegTsTcp { port, .. } => Source::MpegTsTcp {
            host: address.to_string(),
            port: *port,
        },
        Source::Rtp { .. } | Source::MpegTsUdp { .. } | Source::TestPattern => source.clone(),
    }
}

/// An RTSP URI split around its host:
/// `scheme://[user[:password]@]host[:port][/path]`.
struct Authority<'a> {
    /// `rtsp://`, and the credentials with their `@`, if any.
    before: String,
    host: &'a str,
    /// The port with its `:`, and the path, query and fragment.
    after: String,
}

impl<'a> Authority<'a> {
    /// `None` for an IPv6 literal (`[…]`), which is an address already.
    fn of(location: &'a str) -> Option<Self> {
        let (scheme, rest) = location.split_once("://")?;
        let (authority, path) = match rest.find(['/', '?', '#']) {
            Some(at) => rest.split_at(at),
            None => (rest, ""),
        };
        let (credentials, host_and_port) = match authority.rsplit_once('@') {
            Some((credentials, host_and_port)) => (Some(credentials), host_and_port),
            None => (None, authority),
        };
        if host_and_port.starts_with('[') {
            return None;
        }
        let (host, port) = match host_and_port.split_once(':') {
            Some((host, port)) => (host, Some(port)),
            None => (host_and_port, None),
        };

        Some(Self {
            before: match credentials {
                Some(credentials) => format!("{scheme}://{credentials}@"),
                None => format!("{scheme}://"),
            },
            host,
            after: match port {
                Some(port) => format!(":{port}{path}"),
                None => path.to_string(),
            },
        })
    }

    fn with_host(&self, host: &str) -> String {
        format!("{}{host}{}", self.before, self.after)
    }
}

#[cfg(test)]
mod tests {
    //! Avahi is a mock served over a socket pair: no bus, so nothing asks
    //! the network.

    use std::{
        os::unix::net::UnixStream,
        sync::{Arc, Mutex},
        time::Duration,
    };

    use zbus::{Guid, blocking::connection::Builder};

    use super::*;

    fn rtsp(location: &str) -> Source {
        Source::Rtsp {
            location: location.to_string(),
        }
    }

    #[test]
    fn only_local_names_are_multicast() {
        assert!(is_mdns("camera.local"));
        assert!(is_mdns("Camera.LOCAL."));
        assert!(!is_mdns("camera"));
        assert!(!is_mdns("camera.localdomain"));
        assert!(!is_mdns("192.168.144.25"));
    }

    #[test]
    fn the_host_of_an_rtsp_uri_is_replaced_and_nothing_else() {
        for (uri, host_of, resolved) in [
            (
                "rtsp://camera.local:8554/main.264",
                Some("camera.local"),
                "rtsp://10.0.0.7:8554/main.264",
            ),
            (
                "rtsp://admin:secret@camera.local/stream?channel=1",
                Some("camera.local"),
                "rtsp://admin:secret@10.0.0.7/stream?channel=1",
            ),
            (
                "rtspt://camera.local",
                Some("camera.local"),
                "rtspt://10.0.0.7",
            ),
            (
                "rtsp://[fe80::1]:554/main",
                None,
                "rtsp://[fe80::1]:554/main",
            ),
        ] {
            let source = rtsp(uri);
            assert_eq!(host(&source), host_of, "{uri}");
            assert_eq!(with_host(&source, "10.0.0.7"), rtsp(resolved), "{uri}");
        }
    }

    #[test]
    fn a_tcp_stream_has_a_host_and_udp_ones_do_not() {
        let tcp = Source::MpegTsTcp {
            host: "companion.local".to_string(),
            port: 5000,
        };
        assert_eq!(host(&tcp), Some("companion.local"));
        assert_eq!(
            with_host(&tcp, "10.0.0.7"),
            Source::MpegTsTcp {
                host: "10.0.0.7".to_string(),
                port: 5000
            }
        );

        assert_eq!(host(&Source::parse("udp://:5600").unwrap()), None);
        assert_eq!(host(&Source::TestPattern), None);
    }

    /// What the mock Avahi answers.
    #[derive(Clone, Copy)]
    enum Answer {
        Address(&'static str),
        Timeout,
    }

    #[derive(Debug, zbus::DBusError)]
    #[zbus(prefix = "org.freedesktop.Avahi")]
    enum AvahiError {
        #[zbus(error)]
        ZBus(zbus::Error),
        TimeoutError(String),
    }

    struct Avahi {
        answer: Answer,
        asked: Arc<Mutex<Vec<String>>>,
    }

    #[zbus::interface(name = "org.freedesktop.Avahi.Server")]
    impl Avahi {
        fn resolve_host_name(
            &self,
            interface: i32,
            protocol: i32,
            name: &str,
            aprotocol: i32,
            flags: u32,
        ) -> Result<(i32, i32, String, i32, String, u32), AvahiError> {
            self.asked
                .lock()
                .unwrap()
                .push(format!("{interface} {protocol} {name} {aprotocol} {flags}"));
            match self.answer {
                Answer::Address(address) => Ok((2, 0, name.to_string(), 0, address.to_string(), 0)),
                Answer::Timeout => Err(AvahiError::TimeoutError("Timeout reached".to_string())),
            }
        }
    }

    /// An interface Avahi does not have, so the mock answers a call for
    /// it with an error, as a bus without Avahi does.
    struct Nothing;

    #[zbus::interface(name = "org.groundhog.Nothing")]
    impl Nothing {}

    /// A connection to a mock Avahi answering `answer` (or to none), the
    /// server's end (which must stay open), and what Avahi was asked.
    fn avahi(answer: Option<Answer>) -> (Connection, Connection, Arc<Mutex<Vec<String>>>) {
        let asked = Arc::new(Mutex::new(Vec::new()));
        let (ours, theirs) = UnixStream::pair().unwrap();

        let recorded = asked.clone();
        let server = std::thread::spawn(move || {
            let builder = Builder::async_io_unix_stream(theirs)
                .server(Guid::generate())
                .unwrap()
                .p2p();
            let builder = match answer {
                Some(answer) => builder.serve_at(
                    "/",
                    Avahi {
                        answer,
                        asked: recorded,
                    },
                ),
                None => builder.serve_at("/", Nothing),
            };
            builder.unwrap().build().unwrap()
        });

        // A mistake in the mock then fails the test rather than hanging it.
        let client = Builder::async_io_unix_stream(ours)
            .p2p()
            .method_timeout(Duration::from_secs(5))
            .build()
            .unwrap();
        (client, server.join().unwrap(), asked)
    }

    #[test]
    fn a_local_name_is_resolved_to_its_ipv4_address() {
        let (client, _server, asked) = avahi(Some(Answer::Address("192.168.144.25")));

        assert_eq!(
            resolve_on(&client, &rtsp("rtsp://camera.local:8554/main.264")),
            Ok(rtsp("rtsp://192.168.144.25:8554/main.264"))
        );
        // Every interface and protocol, for an IPv4 address.
        assert_eq!(*asked.lock().unwrap(), ["-1 -1 camera.local 0 0"]);
    }

    #[test]
    fn a_name_nothing_answers_to_says_so() {
        let (client, _server, _) = avahi(Some(Answer::Timeout));

        assert_eq!(
            resolve_on(&client, &rtsp("rtsp://camera.local/main")),
            Err("Nothing on the network answers to camera.local".to_string())
        );
    }

    #[test]
    fn without_avahi_the_name_is_left_to_gstreamer() {
        let (client, _server, _) = avahi(None);
        let source = rtsp("rtsp://camera.local/main");

        assert_eq!(resolve_on(&client, &source), Ok(source));
    }

    #[test]
    fn an_answer_that_is_not_ipv4_leaves_the_name() {
        let (client, _server, _) = avahi(Some(Answer::Address("fe80::1")));
        let source = rtsp("rtsp://camera.local/main");

        assert_eq!(resolve_on(&client, &source), Ok(source));
    }

    #[test]
    fn other_names_are_not_looked_up() {
        let (client, _server, asked) = avahi(Some(Answer::Address("192.168.144.25")));

        for uri in ["rtsp://camera/main", "rtsp://192.168.144.25/main"] {
            assert_eq!(resolve_on(&client, &rtsp(uri)), Ok(rtsp(uri)));
        }
        assert!(asked.lock().unwrap().is_empty());
    }
}
