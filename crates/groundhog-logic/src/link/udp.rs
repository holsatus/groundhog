//! UDP transport as a byte stream for the MAVLink parser.
//!
//! Two modes:
//! - **Peer mode**: the socket sends to one fixed peer from the start, so it
//!   transmits before the peer has spoken (typical for a GCS dialing out to a
//!   companion computer or mavlink-router), and hears only that peer. The
//!   socket is deliberately not `connect`ed: connecting pins its local
//!   address to the interface the route used at that moment, so a link
//!   opened before joining the vehicle's network, or kept across an address
//!   change, would send from an address no longer reachable and never hear
//!   back, without any error to reconnect on. Unconnected, the kernel picks
//!   the source address per datagram.
//! - **Listen mode**: the socket accepts datagrams from anyone and keeps a
//!   table of the peers it has heard recently. Outbound frames go to every
//!   live peer, exactly like QGroundControl and mavlink-router do, so several
//!   vehicles (or SITL instances) can share one listening port, and a peer that
//!   reconnects from a new source port is picked up immediately. MAVLink
//!   frames are addressed by target system, so a vehicle ignores frames meant
//!   for another. Anyone can send to a listening port, so the table is
//!   capped at [`MAX_PEERS`]: a flood of senders evicts the oldest instead of
//!   growing it, and the cost of every write, without bound.

use std::io::Error;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::{Arc, Mutex, PoisonError};
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::UdpSocket;

/// A listen-mode peer silent for this long stops receiving our frames.
const PEER_TIMEOUT: Duration = Duration::from_secs(10);

/// The most listen-mode peers served at once: more vehicles than share a
/// port in practice.
const MAX_PEERS: usize = 16;

/// The largest datagram accepted. MAVLink 2 frames are at most 280 bytes, but
/// routers may pack several frames into one datagram.
const DATAGRAM_CAPACITY: usize = 2048;

#[derive(Debug)]
struct Peer {
    addr: SocketAddr,
    last_heard: Instant,
}

/// Whom a socket talks to.
enum Mode {
    /// One fixed peer.
    Peer(SocketAddr),
    /// Every peer heard recently.
    Listen(Mutex<Vec<Peer>>),
}

/// The socket shared by the reader and writer halves, and its mode.
pub(crate) struct UdpShared {
    socket: UdpSocket,
    mode: Mode,
}

impl UdpShared {
    /// A socket that talks to one fixed peer.
    pub(crate) fn peer(socket: UdpSocket, peer: SocketAddr) -> Arc<Self> {
        Arc::new(Self {
            socket,
            mode: Mode::Peer(peer),
        })
    }

    /// A listening socket that serves every peer it hears from.
    pub(crate) fn listening(socket: UdpSocket) -> Arc<Self> {
        Arc::new(Self {
            socket,
            mode: Mode::Listen(Mutex::default()),
        })
    }

    /// Whether a datagram from `addr` is for this link, noting a listen-mode
    /// peer as heard.
    fn accept(&self, addr: SocketAddr) -> bool {
        let peers = match &self.mode {
            Mode::Peer(peer) => return addr == *peer,
            Mode::Listen(peers) => peers,
        };

        let mut peers = peers.lock().unwrap_or_else(PoisonError::into_inner);
        let now = Instant::now();

        match peers.iter_mut().find(|peer| peer.addr == addr) {
            Some(peer) => peer.last_heard = now,
            None => {
                if peers.len() >= MAX_PEERS
                    && let Some(oldest) = peers
                        .iter()
                        .enumerate()
                        .min_by_key(|(_, peer)| peer.last_heard)
                        .map(|(index, _)| index)
                {
                    let evicted = peers.swap_remove(oldest);
                    log::info!("UDP peer {} evicted for {addr}", evicted.addr);
                }
                log::info!("UDP peer {addr} joined");
                peers.push(Peer {
                    addr,
                    last_heard: now,
                });
            }
        }
        true
    }

    /// The listen-mode peers heard within [`PEER_TIMEOUT`], forgetting the
    /// rest.
    fn live_peers(peers: &Mutex<Vec<Peer>>) -> Vec<SocketAddr> {
        let mut peers = peers.lock().unwrap_or_else(PoisonError::into_inner);
        let now = Instant::now();

        peers.retain(|peer| {
            let live = now.duration_since(peer.last_heard) < PEER_TIMEOUT;
            if !live {
                log::info!("UDP peer {} timed out", peer.addr);
            }
            live
        });

        peers.iter().map(|peer| peer.addr).collect()
    }
}

pub(crate) struct UdpReader {
    shared: Arc<UdpShared>,
    buffer: Box<[u8; DATAGRAM_CAPACITY]>,
    head: usize,
    tail: usize,
}

impl UdpReader {
    pub(crate) fn new(shared: Arc<UdpShared>) -> Self {
        Self {
            shared,
            buffer: Box::new([0u8; DATAGRAM_CAPACITY]),
            head: 0,
            tail: 0,
        }
    }

    /// Copy buffered datagram bytes into `buf`.
    fn drain_into(&mut self, buf: &mut ReadBuf<'_>) {
        let to_copy = std::cmp::min(buf.remaining(), self.head - self.tail);
        buf.put_slice(&self.buffer[self.tail..self.tail + to_copy]);
        self.tail += to_copy;
    }
}

impl AsyncRead for UdpReader {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        if self.tail < self.head {
            self.drain_into(buf);
            return Poll::Ready(Ok(()));
        }

        let shared = self.shared.clone();
        let this = &mut *self;

        loop {
            let mut read_buf = ReadBuf::new(this.buffer.as_mut());
            match shared.socket.poll_recv_from(cx, &mut read_buf) {
                Poll::Ready(Ok(from)) => {
                    // A peer-mode link hears only its peer; anything else
                    // is dropped, and the next datagram awaited.
                    if !shared.accept(from) {
                        log::trace!("UDP datagram from {from} ignored");
                        continue;
                    }
                    this.head = read_buf.filled().len();
                    this.tail = 0;
                    this.drain_into(buf);
                    return Poll::Ready(Ok(()));
                }
                // Some systems (Windows, for one) report an ICMP "port
                // unreachable" from an earlier send as a read error. The peer
                // may simply not be up yet; treat it as an empty read rather
                // than a failing transport.
                Poll::Ready(Err(error)) if is_unreachable_peer(&error) => {
                    cx.waker().wake_by_ref();
                    return Poll::Pending;
                }
                Poll::Ready(Err(error)) => return Poll::Ready(Err(error)),
                Poll::Pending => return Poll::Pending,
            }
        }
    }
}

/// Whether a read error only reports that an earlier datagram found nobody
/// listening.
fn is_unreachable_peer(error: &std::io::Error) -> bool {
    matches!(
        error.kind(),
        std::io::ErrorKind::ConnectionRefused | std::io::ErrorKind::ConnectionReset
    )
}

#[derive(Clone)]
pub(crate) struct UdpWriter {
    shared: Arc<UdpShared>,
}

impl UdpWriter {
    pub(crate) fn new(shared: Arc<UdpShared>) -> Self {
        Self { shared }
    }
}

impl AsyncWrite for UdpWriter {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<Result<usize, Error>> {
        let peers = match &self.shared.mode {
            Mode::Peer(peer) => {
                return match self.shared.socket.poll_send_to(cx, buf, *peer) {
                    // A datagram that cannot leave (no route to the peer yet,
                    // or the peer not listening) is lost, as on a radio out of
                    // range. Nothing is wrong with the socket, so the link
                    // stays: the next frame goes out once there is a way.
                    Poll::Ready(Err(error)) => {
                        log::debug!("UDP send to {peer} failed: {error}");
                        Poll::Ready(Ok(buf.len()))
                    }
                    other => other,
                };
            }
            Mode::Listen(peers) => peers,
        };

        // Listen mode: one datagram per live peer. A peer whose send would
        // block loses this frame, as an overflowing radio would; it must not
        // stall the others.
        for addr in UdpShared::live_peers(peers) {
            match self.shared.socket.try_send_to(buf, addr) {
                Ok(_) => {}
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    log::trace!("UDP send to {addr} would block; frame dropped");
                }
                Err(error) => log::debug!("UDP send to {addr} failed: {error}"),
            }
        }

        Poll::Ready(Ok(buf.len()))
    }

    fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Result<(), Error>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Result<(), Error>> {
        Poll::Ready(Ok(()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    async fn bound() -> (UdpSocket, SocketAddr) {
        let socket = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let addr = socket.local_addr().unwrap();
        (socket, addr)
    }

    /// Two vehicles share one listening port: both are heard, and both
    /// receive our frames.
    #[tokio::test]
    async fn listen_mode_serves_every_peer() {
        let (gcs, gcs_addr) = bound().await;
        let shared = UdpShared::listening(gcs);
        let mut reader = UdpReader::new(shared.clone());
        let mut writer = UdpWriter::new(shared);

        let (vehicle_a, _) = bound().await;
        let (vehicle_b, _) = bound().await;
        vehicle_a.send_to(b"from-a", gcs_addr).await.unwrap();
        vehicle_b.send_to(b"from-b", gcs_addr).await.unwrap();

        let mut buffer = [0u8; 6];
        for _ in 0..2 {
            reader.read_exact(&mut buffer).await.unwrap();
        }

        writer.write_all(b"hello").await.unwrap();

        let mut received = [0u8; 16];
        for vehicle in [&vehicle_a, &vehicle_b] {
            let n = tokio::time::timeout(Duration::from_secs(1), vehicle.recv(&mut received))
                .await
                .expect("each peer receives the frame")
                .unwrap();
            assert_eq!(&received[..n], b"hello");
        }
    }

    /// A flood of senders does not grow the peer table: past [`MAX_PEERS`],
    /// the peer heard longest ago makes room.
    #[tokio::test]
    async fn listen_mode_serves_the_newest_peers_up_to_the_cap() {
        let (gcs, gcs_addr) = bound().await;
        let shared = UdpShared::listening(gcs);
        let mut reader = UdpReader::new(shared.clone());
        let mut writer = UdpWriter::new(shared);

        let mut senders = Vec::new();
        for _ in 0..MAX_PEERS + 4 {
            let (sender, _) = bound().await;
            sender.send_to(b"hi", gcs_addr).await.unwrap();
            let mut buffer = [0u8; 2];
            reader.read_exact(&mut buffer).await.unwrap();
            senders.push(sender);
            // Distinct arrival times, so "oldest" is well defined.
            tokio::time::sleep(Duration::from_millis(1)).await;
        }

        writer.write_all(b"hello").await.unwrap();

        // Every send happened within the write, so what the served peers
        // got, the evicted ones would have too.
        let mut received = [0u8; 16];
        let (evicted, served) = senders.split_at(4);
        for sender in served {
            tokio::time::timeout(Duration::from_secs(1), sender.recv(&mut received))
                .await
                .expect("a served peer receives the frame")
                .unwrap();
        }
        for sender in evicted {
            assert!(sender.try_recv(&mut received).is_err(), "evicted");
        }
    }

    /// A peer-mode link hears its peer at any of its own addresses: the
    /// address a reply comes to is not pinned to the interface the route
    /// used at open, so the link survives a change of network. On Linux,
    /// every 127.0.0.0/8 address is local, which stands in for a second
    /// interface.
    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn peer_mode_hears_its_peer_at_another_local_address() {
        let (vehicle, vehicle_addr) = bound().await;
        let gcs = UdpSocket::bind("0.0.0.0:0").await.unwrap();
        let port = gcs.local_addr().unwrap().port();
        let shared = UdpShared::peer(gcs, vehicle_addr);
        let mut reader = UdpReader::new(shared.clone());
        let mut writer = UdpWriter::new(shared);

        // The link speaks first, over the loopback route, as at open.
        writer.write_all(b"hello").await.unwrap();
        let mut received = [0u8; 16];
        let n = vehicle.recv(&mut received).await.unwrap();
        assert_eq!(&received[..n], b"hello");

        // The vehicle answers at another of the GCS's addresses.
        vehicle
            .send_to(b"answer", ("127.0.0.2", port))
            .await
            .unwrap();
        let mut buffer = [0u8; 6];
        tokio::time::timeout(Duration::from_secs(1), reader.read_exact(&mut buffer))
            .await
            .expect("the answer arrives")
            .unwrap();
        assert_eq!(&buffer, b"answer");
    }

    /// A peer-mode link hears only its peer.
    #[tokio::test]
    async fn peer_mode_ignores_other_senders() {
        let (vehicle, vehicle_addr) = bound().await;
        let (gcs, gcs_addr) = bound().await;
        let mut reader = UdpReader::new(UdpShared::peer(gcs, vehicle_addr));

        let (stranger, _) = bound().await;
        stranger.send_to(b"stranger", gcs_addr).await.unwrap();
        vehicle.send_to(b"vehicle", gcs_addr).await.unwrap();

        let mut buffer = [0u8; 7];
        tokio::time::timeout(Duration::from_secs(1), reader.read_exact(&mut buffer))
            .await
            .expect("the peer's datagram arrives")
            .unwrap();
        assert_eq!(&buffer, b"vehicle");
    }

    /// Before anyone has spoken, listen mode has nobody to send to: frames are
    /// dropped without failing the writer.
    #[tokio::test]
    async fn listen_mode_without_peers_drops_frames() {
        let (gcs, _) = bound().await;
        let mut writer = UdpWriter::new(UdpShared::listening(gcs));
        writer.write_all(b"nobody").await.unwrap();
    }
}
