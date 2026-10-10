//! Opening transports and attaching them to the MAVLink router, plus the
//! type-erased async I/O adapters the router works with.

use std::sync::{Arc, atomic::AtomicBool};

use iced::Task;

use crate::{
    foundation::{ArcError, LinkId},
    mav,
};

use super::config::LinkBuild;

/// The result of opening a link transport.
#[derive(Debug, Clone)]
pub enum Event {
    Connected(LinkId, mav::LinkGuard),
    Failed(ArcError),
}

impl LinkBuild {
    /// Open the transport and attach it to the provided `router`.
    pub fn connect(&self, id: LinkId, router: mav::MavlinkRouter) -> Task<Event> {
        fn attached(id: &LinkId, handle: mav::LinkGuard) -> Task<Event> {
            Task::done(Event::Connected(id.clone(), handle))
        }

        fn error_map<E: std::error::Error + Send + Sync + 'static>(error: E) -> Task<Event> {
            Task::done(Event::Failed(Arc::new(error)))
        }

        match self.clone() {
            LinkBuild::Tcp { sock_addr } => Task::future(tokio::net::TcpStream::connect(sock_addr))
                .then(move |result| match result {
                    Ok(stream) => {
                        let (reader, writer) = stream.into_split();
                        attached(&id, router.attach_reader_writer(id.clone(), reader, writer))
                    }
                    Err(error) => error_map(error),
                }),
            LinkBuild::Udp { sock_addr } => Task::future(tokio::net::UdpSocket::bind(sock_addr))
                .then(move |result| match result {
                    Ok(socket) => {
                        let connected = AtomicBool::new(false);
                        let socket = Arc::new((socket, connected));
                        let writer = super::udp::UdpWriter::new(socket.clone());
                        let reader = super::udp::UdpReader::new(socket);
                        attached(&id, router.attach_reader_writer(id.clone(), reader, writer))
                    }
                    Err(error) => error_map(error),
                }),
            LinkBuild::Serial { port, baud } => {
                let writer = match serial2_tokio::SerialPort::open(&port, baud) {
                    Ok(port) => port,
                    Err(error) => return error_map(error),
                };

                // Not sure why this would ever fail?
                let reader = match writer.try_clone() {
                    Ok(rcv) => rcv,
                    Err(error) => return error_map(error),
                };

                attached(&id, router.attach_reader_writer(id.clone(), reader, writer))
            }
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

impl mavio::io::AsyncRead<std::io::Error> for BoxedReader {
    async fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        self.0.read(buf).await
    }
}

pub struct BoxedWriter(pub Box<dyn DynAsyncWriter>);

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
