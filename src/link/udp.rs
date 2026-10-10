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

                self.head = read_buf.filled().len();
                self.tail = 0;

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
            return Poll::Ready(Ok(buf.len()));
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
            }
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
