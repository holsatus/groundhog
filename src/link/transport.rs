//! Opening transports and attaching them to the MAVLink router, plus the
//! type-erased async I/O adapters the router works with.

use std::{
    sync::{Arc, atomic::AtomicBool},
    time::Duration,
};

use crate::{
    foundation::{ArcError, LinkId},
    mavlink,
};

use super::config::LinkBuild;

/// TCP connect timeout for a transport open: a dead address must fail the
/// open in bounded time, not hang the pane's connect task.
const TCP_CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

/// TCP keepalive tuning applied best effort to connected transports: a silent
/// TCP death is otherwise undetectable on the reader half of a link.
const TCP_KEEPALIVE_TIME: Duration = Duration::from_secs(15);
const TCP_KEEPALIVE_INTERVAL: Duration = Duration::from_secs(5);
const TCP_KEEPALIVE_RETRIES: u32 = 3;

/// The result of opening a link transport.
///
/// Named for what it is, not when it fires: this value is the outcome of a
/// `Connect`, not a stream event like [`mavlink::router::Event`].
#[derive(Debug, Clone)]
pub enum OpenResult {
    Connected(LinkId, mavlink::LinkGuard),
    Failed(ArcError),
}

/// Erase an error into the shared error type.
fn arc<E: std::error::Error + Send + Sync + 'static>(error: E) -> ArcError {
    Arc::new(error)
}

/// Open the transport described by `build`, returning the reader and writer to
/// attach to a router. Awaiting this performs any blocking work off-thread.
pub async fn open(build: LinkBuild) -> Result<(BoxedReader, BoxedWriter), ArcError> {
    match build {
        LinkBuild::Tcp { sock_addr } => {
            let stream = tokio::time::timeout(
                TCP_CONNECT_TIMEOUT,
                tokio::net::TcpStream::connect(sock_addr),
            )
            .await
            .map_err(|_| {
                arc(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    "TCP connect timed out",
                ))
            })?
            .map_err(arc)?;

            let stream = tune_tcp(stream).map_err(arc)?;

            let (reader, writer) = stream.into_split();
            Ok((BoxedReader::new(reader), BoxedWriter::new(writer)))
        }
        LinkBuild::Udp { sock_addr } => {
            let socket = tokio::net::UdpSocket::bind(sock_addr).await.map_err(arc)?;
            let socket = Arc::new((socket, AtomicBool::new(false)));
            let reader = super::udp::UdpReader::new(socket.clone());
            let writer = super::udp::UdpWriter::new(socket);
            Ok((BoxedReader::new(reader), BoxedWriter::new(writer)))
        }
        LinkBuild::Serial { port, baud } => {
            // `open` and `try_clone` both block and register with the tokio
            // reactor; `spawn_blocking` enters the runtime context for us.
            let (reader, writer) = tokio::task::spawn_blocking(move || {
                let writer = serial2_tokio::SerialPort::open(&port, baud)?;
                let reader = writer.try_clone()?;
                Ok::<_, std::io::Error>((reader, writer))
            })
            .await
            .map_err(arc)?
            .map_err(arc)?;

            Ok((BoxedReader::new(reader), BoxedWriter::new(writer)))
        }
    }
}

/// Tune an established TCP transport for robustness: TCP keepalive (so a
/// silently dead network becomes detectable on the reader) and `TCP_NODELAY`
/// (so control exchanges stay fresh).
///
/// Tuning is best effort: an option the socket refuses is logged and ignored
/// and the open still succeeds, returning the — possibly untuned — halves in
/// the usual way.
fn tune_tcp(stream: tokio::net::TcpStream) -> Result<tokio::net::TcpStream, ArcError> {
    // Round-trip through the std socket for the socket2 options; tokio's
    // `into_std` deregisters from the reactor and `from_std` re-registers.
    let socket = socket2::Socket::from(stream.into_std().map_err(arc)?);

    if let Err(error) = socket.set_tcp_keepalive(&tcp_keepalive()) {
        log::warn!("Could not set TCP keepalive on transport: {error}");
    }

    if let Err(error) = socket.set_tcp_nodelay(true) {
        log::warn!("Could not set TCP_NODELAY on transport: {error}");
    }

    tokio::net::TcpStream::from_std(socket.into()).map_err(arc)
}

/// The keepalive settings this platform supports: the idle time is everywhere,
/// interval and retries exist on fewer systems (socket2 gates them per OS).
fn tcp_keepalive() -> socket2::TcpKeepalive {
    let keepalive = socket2::TcpKeepalive::new().with_time(TCP_KEEPALIVE_TIME);

    // `with_interval` and `with_retries` are not offered on every platform
    // socket2 supports; apply them where the target has them.
    #[cfg(any(
        target_os = "linux",
        target_os = "macos",
        target_os = "ios",
        target_os = "visionos",
        target_os = "tvos",
        target_os = "watchos",
        target_os = "android",
        target_os = "freebsd",
        target_os = "netbsd",
        target_os = "dragonfly",
        target_os = "illumos",
        target_os = "emscripten",
        target_os = "fuchsia",
        target_os = "cygwin",
        target_os = "windows",
        target_os = "nuttx",
    ))]
    let keepalive = keepalive.with_interval(TCP_KEEPALIVE_INTERVAL);

    // `with_retries` additionally needs socket2's `all` feature, enabled for
    // this dependency.
    #[cfg(any(
        target_os = "linux",
        target_os = "macos",
        target_os = "ios",
        target_os = "visionos",
        target_os = "tvos",
        target_os = "watchos",
        target_os = "android",
        target_os = "freebsd",
        target_os = "netbsd",
        target_os = "dragonfly",
        target_os = "illumos",
        target_os = "emscripten",
        target_os = "fuchsia",
        target_os = "cygwin",
        target_os = "windows",
        target_os = "nuttx",
    ))]
    let keepalive = keepalive.with_retries(TCP_KEEPALIVE_RETRIES);

    keepalive
}

// ----- dyn-compatible trait definition -----

#[async_trait::async_trait]
pub trait DynAsyncReader: Send {
    async fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize>;
}

#[async_trait::async_trait]
pub trait DynAsyncWriter: Send {
    async fn write_all(&mut self, buf: &[u8]) -> std::io::Result<()>;
    async fn flush(&mut self) -> std::io::Result<()>;
}

// ----- blanket tokio implementation -----

#[async_trait::async_trait]
impl<R: tokio::io::AsyncRead + Unpin + Send> DynAsyncReader for R {
    async fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        tokio::io::AsyncReadExt::read(self, buf).await
    }
}

#[async_trait::async_trait]
impl<W: tokio::io::AsyncWrite + Unpin + Send> DynAsyncWriter for W {
    async fn write_all(&mut self, buf: &[u8]) -> std::io::Result<()> {
        tokio::io::AsyncWriteExt::write_all(self, buf).await
    }

    async fn flush(&mut self) -> std::io::Result<()> {
        tokio::io::AsyncWriteExt::flush(self).await
    }
}

// ----- mavio wrapper of the above traits -----

pub struct BoxedReader(pub Box<dyn DynAsyncReader>);

impl BoxedReader {
    pub fn new(reader: impl DynAsyncReader + 'static) -> Self {
        Self(Box::new(reader))
    }
}

#[async_trait::async_trait]
impl DynAsyncReader for BoxedReader {
    async fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        self.0.read(buf).await
    }
}

impl mavio::io::AsyncRead<std::io::Error> for BoxedReader {
    async fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        self.0.read(buf).await
    }
}

pub struct BoxedWriter(pub Box<dyn DynAsyncWriter>);

impl BoxedWriter {
    pub fn new(writer: impl DynAsyncWriter + 'static) -> Self {
        Self(Box::new(writer))
    }
}

#[async_trait::async_trait]
impl DynAsyncWriter for BoxedWriter {
    async fn write_all(&mut self, buf: &[u8]) -> std::io::Result<()> {
        self.0.write_all(buf).await
    }

    async fn flush(&mut self) -> std::io::Result<()> {
        self.0.flush().await
    }
}

impl mavio::io::AsyncWrite<std::io::Error> for BoxedWriter {
    async fn write_all(&mut self, buf: &[u8]) -> std::io::Result<()> {
        self.0.write_all(buf).await
    }

    async fn flush(&mut self) -> std::io::Result<()> {
        self.0.flush().await
    }
}

/// Alias for the versionless boxed async reader for type erasure
pub type AsyncReceiver = mavio::io::AsyncReceiver<
    std::io::Error,
    BoxedReader,
    mavio::prelude::Versionless,
    mavio::protocol::FrameParser<mavio::prelude::Versionless>,
>;

/// Construct a new [`AsyncReceiver`]
pub fn new_async_receiver<R: DynAsyncReader + 'static>(reader: R) -> AsyncReceiver {
    mavio::io::AsyncReceiver::new(BoxedReader(Box::new(reader))).make_stateful()
}

/// Alias for the versionless boxed async writer for type erasure
pub type AsyncSender =
    mavio::io::AsyncSender<std::io::Error, BoxedWriter, mavio::prelude::Versionless>;

/// Construct a new [`AsyncSender`]
pub fn new_async_sender<W: DynAsyncWriter + 'static>(writer: W) -> AsyncSender {
    mavio::io::AsyncSender::new(BoxedWriter(Box::new(writer)))
}
