//! Opening transports and attaching them to the MAVLink router, plus the
//! type-erased async I/O adapters the router works with.

use std::sync::{Arc, atomic::AtomicBool};

use crate::{
    foundation::{ArcError, LinkId},
    mavlink,
};

use super::config::LinkBuild;

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
            let stream = tokio::net::TcpStream::connect(sock_addr)
                .await
                .map_err(arc)?;
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
