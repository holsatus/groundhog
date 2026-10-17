//! mavio adapter plumbing for the type-erased async I/O defined in
//! [`crate::link::transport`]: the boxed reader/writer are bridged to
//! `mavio`'s async traits and wrapped in mavio's framed receiver/sender.

use crate::link::transport::{BoxedReader, BoxedWriter, DynAsyncReader, DynAsyncWriter};

impl mavio::io::AsyncRead<std::io::Error> for BoxedReader {
    async fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        self.0.read(buf).await
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
