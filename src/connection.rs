use std::{
    net::{Ipv4Addr, SocketAddrV4},
    str::FromStr,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU8, AtomicUsize, Ordering},
    },
    task::Poll,
};

use iced::{
    Task,
    futures::{Stream, StreamExt},
};
pub use mav_tokio::{DynAsyncReader, DynAsyncWriter, new_async_receiver, new_async_sender};
use mavio::{Frame, prelude::Versionless, protocol::FrameBuilder};
use tokio::sync::mpsc::{self, Receiver, Sender};
use tokio_util::sync::CancellationToken;

use crate::{ConnMessage, connection::mav_tokio::AsyncReceiver, parameters::MavlinkId};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct LinkId(usize);

impl LinkId {
    pub fn new_unique() -> Self {
        static LINK_ID: AtomicUsize = AtomicUsize::new(0);
        LinkId(LINK_ID.fetch_add(1, Ordering::Relaxed))
    }
}

pub const BAUDRATES: &[u32] = &[
    2_400, 4_800, 9_600, 19_200, 38_400, 57_600, 115_200, 230_400, 460_800, 500_000, 576_000,
    921_600, 1_000_000,
];

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub enum LinkConfig {
    Tcp { sock_addr: SocketAddrV4 },
    Udp { sock_addr: SocketAddrV4 },
    Serial { port: String, baud: u32 },
}

impl LinkConfig {
    pub fn to_builder(&self) -> LinkBuilder {
        match self {
            LinkConfig::Tcp { sock_addr } => LinkBuilder::Tcp {
                addr: sock_addr.ip().to_string(),
                port: sock_addr.port().to_string(),
            },
            LinkConfig::Udp { sock_addr } => LinkBuilder::Udp {
                addr: sock_addr.ip().to_string(),
                port: sock_addr.port().to_string(),
            },
            LinkConfig::Serial { port, baud } => LinkBuilder::Serial {
                port: Some(port.clone()),
                available_ports: Vec::new(),
                baud: *baud,
            },
        }
    }

    pub fn connect(&self) -> Task<crate::Message> {
        match self.clone() {
            LinkConfig::Tcp { sock_addr } => Task::future(tokio::net::TcpStream::connect(
                sock_addr,
            ))
            .then(|result| match result {
                Ok(stream) => {
                    let (rcv, snd) = stream.into_split();
                    ConnectionSendRunner::spawn(rcv, snd)
                }
                Err(error) => Task::done(ConnMessage::ConnectFailed(Arc::new(error)).into()),
            }),
            LinkConfig::Udp { sock_addr } => Task::future(tokio::net::UdpSocket::bind(sock_addr))
            .then(|result| match result {
                Ok(socket) => {
                    let connected = AtomicBool::new(false);
                    let socket = std::sync::Arc::new((socket, connected));
                    let writer = udp_wrap::UdpWriter::new(socket.clone());
                    let reader = udp_wrap::UdpReader::new(socket);
                    ConnectionSendRunner::spawn(reader, writer)
                }
                Err(error) => Task::done(ConnMessage::ConnectFailed(Arc::new(error)).into()),
            }),
            LinkConfig::Serial { port, baud } => {
                let port = match serial2_tokio::SerialPort::open(&port, baud) {
                    Ok(port) => port,
                    Err(error) => {
                        return Task::done(ConnMessage::ConnectFailed(Arc::new(error)).into());
                    }
                };

                let rcv = match port.try_clone() {
                    Ok(rcv) => rcv,
                    Err(error) => {
                        return Task::done(ConnMessage::ConnectFailed(Arc::new(error)).into());
                    }
                };

                let snd = port;
                ConnectionSendRunner::spawn(rcv, snd)
            }
        }
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

    pub fn try_build(&self) -> Option<LinkConfig> {
        let config = match self {
            LinkBuilder::Tcp { addr, port } => {
                let addr = Ipv4Addr::from_str(addr).ok()?;
                let port = u16::from_str(port).ok()?;
                let sock = SocketAddrV4::new(addr, port);
                LinkConfig::Tcp { sock_addr: sock }
            }
            LinkBuilder::Udp { addr, port } => {
                let addr = Ipv4Addr::from_str(addr).ok()?;
                let port = u16::from_str(port).ok()?;
                let sock = SocketAddrV4::new(addr, port);
                LinkConfig::Udp { sock_addr: sock }
            }
            LinkBuilder::Serial { port, baud, .. } => {
                let port = port.to_owned()?;
                LinkConfig::Serial { port, baud: *baud }
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

/// A handle to interact with an established MAVLink connection.
///
/// Dropping this handle will release the associated system resources.
#[derive(Debug)]
struct ConnectionHandleState {
    sequence: AtomicU8,
    send_frame: Sender<Frame<Versionless>>,
    cancellation_token: CancellationToken,
}

impl Drop for ConnectionHandleState {
    fn drop(&mut self) {
        self.cancellation_token.cancel();
    }
}

struct ConnectionSendRunner {
    mav_sender: mav_tokio::AsyncSender,
    recv_frame: Receiver<Frame<Versionless>>,
}

#[derive(Debug, Clone)]
pub struct WeakConnectionHandle {
    mav_id: MavlinkId,
    send_state: std::sync::Weak<ConnectionHandleState>,
}

impl WeakConnectionHandle {
    pub fn upgrade(&self) -> Option<ConnectionHandle> {
        self.send_state
            .upgrade()
            .map(|send_state| ConnectionHandle {
                mav_id: self.mav_id.clone(),
                send_state,
            })
    }

    pub async fn send_message<M: mavio::Message>(&self, message: M) -> bool {
        if let Some(handle) = self.upgrade() {
            handle.send_message(message).await
        } else {
            false
        }
    }

    /// Send the message on this connection if it is still live.
    pub fn spawn_send_message<M: mavio::Message + Send + 'static>(&self, message: M) {
        let connection = self.clone();
        tokio::spawn(async move {
            connection.send_message(message).await;
        });
    }
}

#[derive(Debug, Clone)]
pub struct ConnectionHandle {
    mav_id: MavlinkId,
    send_state: Arc<ConnectionHandleState>,
}

impl ConnectionHandle {
    pub fn downgrade(&self) -> WeakConnectionHandle {
        WeakConnectionHandle {
            mav_id: self.mav_id.clone(),
            send_state: Arc::downgrade(&self.send_state),
        }
    }

    pub fn new(
        mav_id: MavlinkId,
        sender: Sender<Frame<Versionless>>,
        cancellation_token: CancellationToken,
    ) -> Self {
        Self {
            mav_id,
            send_state: Arc::new(ConnectionHandleState {
                sequence: AtomicU8::new(0),
                send_frame: sender,
                cancellation_token,
            }),
        }
    }

    async fn send_message<M: mavio::Message>(&self, message: M) -> bool {
        let sequence = self.send_state.sequence.fetch_add(1, Ordering::AcqRel);

        let frame = FrameBuilder::new()
            .system_id(self.mav_id.system)
            .component_id(self.mav_id.component)
            .version(mavio::prelude::V2)
            .sequence(sequence)
            .message(&message)
            .unwrap()
            .build()
            .into_versionless();

        if self.send_state.send_frame.send(frame).await.is_err() {
            log::error!("Failed to send MAVLink frame because connection queue closed");
            false
        } else {
            true
        }
    }

    pub fn close(self) {
        self.send_state.cancellation_token.cancel();
    }
}

impl ConnectionSendRunner {
    fn spawn<R: DynAsyncReader + 'static, W: DynAsyncWriter + 'static>(
        rcv: R,
        snd: W,
    ) -> Task<crate::Message> {
        let mav_receiver = new_async_receiver(rcv);
        let mav_sender = new_async_sender(snd);

        let (send_frame, recv_frame) = mpsc::channel::<Frame<Versionless>>(32);
        let cancellation_token = CancellationToken::new();

        let link_id = LinkId::new_unique();

        struct ReceiverStream {
            receiver: AsyncReceiver,
            cancellation_token: CancellationToken,
        }

        impl Stream for ReceiverStream {
            type Item = Result<Frame<Versionless>, mavio::Error>;

            fn poll_next(
                mut self: std::pin::Pin<&mut Self>,
                cx: &mut std::task::Context<'_>,
            ) -> Poll<Option<Self::Item>> {
                if self.cancellation_token.is_cancelled() {
                    return Poll::Ready(None);
                }

                {
                    let cancelled = core::pin::pin!(self.cancellation_token.cancelled());
                    if cancelled.poll(cx).is_ready() {
                        return Poll::Ready(None);
                    }
                }

                {
                    let pinned = core::pin::pin!(self.receiver.recv());
                    match pinned.poll(cx) {
                        Poll::Ready(Err(mavio::Error::Io(_))) => Poll::Ready(None),
                        Poll::Ready(result) => Poll::Ready(Some(result)),
                        Poll::Pending => Poll::Pending,
                    }
                }
            }
        }

        let recv_task = Task::stream(
            ReceiverStream {
                receiver: mav_receiver,
                cancellation_token: cancellation_token.clone(),
            }
            .map(move |result| {
                match result {
                    Ok(frame) => ConnMessage::RecvFrame(frame, link_id),
                    Err(error) => ConnMessage::RecvError(error, link_id),
                }
                .into()
            }),
        );

        let connection = ConnectionSendRunner {
            mav_sender,
            recv_frame,
        };

        tokio::spawn(connection.run());

        // TODO: Make configurable
        let mav_id = MavlinkId {
            system: 255,
            component: 1,
        };

        let connection_handle =
            ConnectionHandle::new(mav_id, send_frame, cancellation_token.clone());

        Task::done(crate::Message::Conn(ConnMessage::ConnectSuccess(
            connection_handle,
        )))
        .chain(recv_task)
    }

    async fn run(mut self) {
        while let Some(frame_out) = self.recv_frame.recv().await {
            if let Err(err) = self.mav_sender.send(&frame_out).await {
                log::error!("Failed to send MAVLink frame to transport: {err}");
            }
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

    // ----- mavio wrapper of the above trait -----

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

mod udp_wrap {
    use std::io::Error;
    use std::pin::Pin;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::task::{Context, Poll};

    use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
    use tokio::net::UdpSocket;

    pub struct UdpReader {
        socket: Arc<(UdpSocket, AtomicBool)>,
        buffer: Box<[u8; 2048]>,
        head: usize,
        tail: usize,
    }

    impl UdpReader {
        pub fn new(socket: Arc<(UdpSocket, AtomicBool)>) -> Self {
            Self {
                socket,
                buffer: Box::new([0u8; 2048]),
                head: 0,
                tail: 0,
            }
        }
    }

    impl AsyncRead for UdpReader {
        fn poll_read(
            mut self: Pin<&mut Self>,
            cx: &mut Context<'_>,
            buf: &mut ReadBuf<'_>,
        ) -> Poll<std::io::Result<()>> {
            if self.tail < self.head {
                // Fill the ReadBuf from the internal buffer storage
                let to_copy = std::cmp::min(buf.remaining(), self.head - self.tail);
                buf.put_slice(&self.buffer[self.tail..self.tail + to_copy]);
                self.tail += to_copy;
                return Poll::Ready(Ok(()));
            }

            let socket = self.socket.clone();

            let mut read_buf = ReadBuf::new(self.buffer.as_mut());
            
            match socket.0.poll_recv_from(cx, &mut read_buf) {
                Poll::Ready(Ok(from)) => {

                    // Try to connect to source if we arent already connected
                    if !socket.1.load(Ordering::Relaxed) {
                        let pinned = core::pin::pin!(socket.0.connect(from));
                        if let Poll::Ready(Ok(())) = pinned.poll(cx) {
                            socket.1.store(true, Ordering::Relaxed);
                        }
                    }

                    let filled = read_buf.filled().len();

                    drop(read_buf);

                    self.tail = 0;
                    self.head = filled;

                    // Fill the ReadBuf from the internal buffer storage
                    let to_copy = std::cmp::min(buf.remaining(), self.head - self.tail);
                    buf.put_slice(&self.buffer[self.tail..self.tail + to_copy]);
                    self.tail += to_copy;
                    Poll::Ready(Ok(()))
                }
                Poll::Ready(Err(e)) => {
                    cx.waker().wake_by_ref();
                    if e.kind() == std::io::ErrorKind::WouldBlock {
                        Poll::Pending
                    } else {
                        Poll::Ready(Err(e))
                    }
                }
                Poll::Pending => Poll::Pending,
            }
        }
    }

    #[derive(Clone)]
    pub struct UdpWriter {
        socket: Arc<(UdpSocket, AtomicBool)>,
    }

    impl UdpWriter {
        pub fn new(socket: Arc<(UdpSocket, AtomicBool)>) -> Self {
            Self { socket }
        }
    }

    impl AsyncWrite for UdpWriter {
        fn poll_write(
            self: Pin<&mut Self>,
            cx: &mut Context<'_>,
            buf: &[u8],
        ) -> Poll<Result<usize, Error>> {

            // Pretend to write successfully if we arent connected yet,
            // since the reader will connect on the first packet received.
            if !self.socket.1.load(Ordering::Relaxed) {
                return Poll::Ready(Ok(buf.len()))
            }

            match self.socket.0.poll_send(cx, buf) {
                Poll::Ready(Ok(bytes_sent)) => Poll::Ready(Ok(bytes_sent)),
                Poll::Ready(Err(e)) => {
                    cx.waker().wake_by_ref();
                    if e.kind() == std::io::ErrorKind::WouldBlock {
                        Poll::Pending
                    } else {
                        Poll::Ready(Err(e))
                    }
                },
                Poll::Pending => Poll::Pending,
            }
        }

        fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Result<(), Error>> {
            Poll::Ready(Ok(()))
        }

        fn poll_shutdown(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Result<(), Error>> {
            Poll::Ready(Ok(()))
        }
    }
}
