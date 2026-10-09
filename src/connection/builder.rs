use std::{
    net::{Ipv4Addr, SocketAddrV4},
    str::FromStr,
    sync::{Arc, atomic::AtomicBool},
};

use iced::Task;
#[cfg(test)]
pub use mav_tokio::{AsyncReceiver, AsyncSender};
pub use mav_tokio::{DynAsyncReader, DynAsyncWriter, new_async_receiver, new_async_sender};

use crate::{ConnectionMessage, mav};

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct LinkId(Arc<str>);

impl LinkId {
    pub fn new(id: &str) -> Self {
        LinkId(Arc::from(id))
    }

    pub fn str(&self) -> &str {
        &self.0
    }
}

pub const BAUDRATES: &[u32] = &[
    2_400, 4_800, 9_600, 19_200, 38_400, 57_600, 115_200, 230_400, 460_800, 500_000, 576_000,
    921_600, 1_000_000,
];

#[derive(Debug, Clone)]
pub struct LinkConfig {
    pub name: String,
    pub auto_connect: bool,
    pub high_latency: bool,
    pub build: LinkBuild,
}

impl LinkConfigurator {
    pub fn parse(&self) -> Option<LinkConfig> {
        self.build.as_ref().map(|build| LinkConfig {
            name: self.name.clone(),
            auto_connect: self.auto_connect,
            high_latency: self.high_latency,
            build: build.clone(),
        })
    }
}

pub struct LinkConfigurator {
    pub name: String,
    pub auto_connect: bool,
    pub high_latency: bool,
    pub builder: LinkBuilder,
    pub build: Option<LinkBuild>,
}

impl LinkConfigurator {
    pub fn new(builder: LinkBuilder, name: String) -> LinkConfigurator {
        LinkConfigurator {
            name,
            auto_connect: false,
            high_latency: false,
            builder: builder.clone(),
            build: builder.try_build(),
        }
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub enum LinkBuild {
    Tcp { sock_addr: SocketAddrV4 },
    Udp { sock_addr: SocketAddrV4 },
    Serial { port: String, baud: u32 },
}

impl LinkBuild {
    pub fn to_builder(&self) -> LinkBuilder {
        match self {
            LinkBuild::Tcp { sock_addr } => LinkBuilder::Tcp {
                addr: sock_addr.ip().to_string(),
                port: sock_addr.port().to_string(),
            },
            LinkBuild::Udp { sock_addr } => LinkBuilder::Udp {
                addr: sock_addr.ip().to_string(),
                port: sock_addr.port().to_string(),
            },
            LinkBuild::Serial { port, baud } => LinkBuilder::Serial {
                port: Some(port.clone()),
                available_ports: Vec::new(),
                baud: *baud,
            },
        }
    }

    /// Open the transport and attach it to the provided `router`.
    pub fn connect(&self, id: LinkId, router: mav::MavlinkRouter) -> Task<crate::Message> {
        fn attached(id: &LinkId, handle: mav::LinkGuard) -> Task<crate::Message> {
            Task::done(ConnectionMessage::ConnectSuccess(id.clone(), handle).into())
        }

        fn error_map<E: std::error::Error + Send + Sync + 'static>(
            error: E,
        ) -> Task<crate::Message> {
            Task::done(ConnectionMessage::ConnectFailed(Arc::new(error)).into())
        }

        let task = match self.clone() {
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
                        let socket = std::sync::Arc::new((socket, connected));
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
        };

        task
    }
}

#[derive(Debug, Clone)]
pub enum LinkBuilder {
    Tcp {
        addr: String,
        port: String,
    },
    Udp {
        addr: String,
        port: String,
    },
    Serial {
        port: Option<String>,
        baud: u32,
        available_ports: Vec<String>,
    },
}

impl LinkBuilder {
    pub fn default_tcp() -> LinkBuilder {
        LinkBuilder::Tcp {
            addr: "0.0.0.0".into(),
            port: "5760".into(),
        }
    }

    pub fn default_udp() -> LinkBuilder {
        LinkBuilder::Udp {
            addr: "0.0.0.0".into(),
            port: "14550".into(),
        }
    }

    pub fn default_serial() -> LinkBuilder {
        LinkBuilder::Serial {
            port: None,
            baud: 115_200,
            available_ports: Vec::new(),
        }
    }

    pub fn to_variant(&self) -> LinkVariant {
        match self {
            LinkBuilder::Tcp { .. } => LinkVariant::Tcp,
            LinkBuilder::Udp { .. } => LinkVariant::Udp,
            LinkBuilder::Serial { .. } => LinkVariant::Serial,
        }
    }

    pub fn try_build(&self) -> Option<LinkBuild> {
        let config = match self {
            LinkBuilder::Tcp { addr, port } => {
                let addr = Ipv4Addr::from_str(addr).ok()?;
                let port = u16::from_str(port).ok()?;
                let sock = SocketAddrV4::new(addr, port);
                LinkBuild::Tcp { sock_addr: sock }
            }
            LinkBuilder::Udp { addr, port } => {
                let addr = Ipv4Addr::from_str(addr).ok()?;
                let port = u16::from_str(port).ok()?;
                let sock = SocketAddrV4::new(addr, port);
                LinkBuild::Udp { sock_addr: sock }
            }
            LinkBuilder::Serial { port, baud, .. } => {
                let port = port.to_owned()?;
                LinkBuild::Serial { port, baud: *baud }
            }
        };

        Some(config)
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum LinkVariant {
    Tcp,
    Udp,
    Serial,
}

impl LinkVariant {
    pub fn to_default_builder(self) -> LinkBuilder {
        match self {
            LinkVariant::Tcp => LinkBuilder::default_tcp(),
            LinkVariant::Udp => LinkBuilder::default_udp(),
            LinkVariant::Serial => LinkBuilder::default_serial(),
        }
    }

    pub fn list() -> &'static [LinkVariant] {
        &[LinkVariant::Tcp, LinkVariant::Udp, LinkVariant::Serial]
    }
}

impl std::fmt::Display for LinkVariant {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LinkVariant::Tcp => f.write_str("TCP"),
            LinkVariant::Udp => f.write_str("UDP"),
            LinkVariant::Serial => f.write_str("Serial"),
        }
    }
}

mod mav_tokio {
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

    // ----- blanket tokio imeplementation -----

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
    pub fn new_async_receiver<W: DynAsyncReader + 'static>(writer: W) -> AsyncReceiver {
        mavio::io::AsyncReceiver::new(BoxedReader(Box::new(writer))).make_stateful()
    }

    /// Alias for the versionless boxed async writer for type erasure
    pub type AsyncSender =
        mavio::io::AsyncSender<std::io::Error, BoxedWriter, mavio::prelude::Versionless>;

    /// Construct a new [`AsyncSender`]
    pub fn new_async_sender<W: DynAsyncWriter + 'static>(writer: W) -> AsyncSender {
        mavio::io::AsyncSender::new(BoxedWriter(Box::new(writer)))
    }
}
