use std::{future::IntoFuture, sync::Arc, time::Duration};

use futures::future::BoxFuture;
use mavio::{error::SpecError, mavspec::rust::spec::MessageSpecStatic, protocol::Payload};
use tokio::sync::mpsc;

use super::{
    error::{MavError, Result},
    matches_target,
    router::{InboundMessage, MavlinkRouter},
};
use crate::foundation::MavlinkId;

const RECEIVE_TIMEOUT: Duration = Duration::from_secs(3);

/// A MAVLink message type which can be decoded directly from a frame.
pub trait MavlinkMessage:
    MessageSpecStatic + for<'a> TryFrom<&'a Payload, Error = SpecError> + Send + 'static
{
}

impl<T> MavlinkMessage for T where
    T: MessageSpecStatic + for<'a> TryFrom<&'a Payload, Error = SpecError> + Send + 'static
{
}

impl MavlinkRouter {
    /// Start listening for messages of type `M` from `source`.
    ///
    /// Listening starts immediately, so create the [`MessageReceiver`] _before_
    /// sending whatever request should trigger the message.
    pub fn receive_message<M: MavlinkMessage>(&self, source: MavlinkId) -> MessageReceiver<M> {
        MessageReceiver::new(self, source)
    }

    /// Start receiving every inbound message matching `filter`.
    pub fn receive_filtered(
        &self,
        filter: impl Fn(&InboundMessage) -> bool + Send + 'static,
    ) -> FilteredReceiver {
        FilteredReceiver::new(self, filter)
    }
}

/// Receives every inbound message matching a filter. Created by [`MavlinkRouter::watch`].
///
/// The filter is registered when the [`Watch`] is created, not when it is first
/// polled, so no message can be missed between creating it and sending a request.
/// It is deregistered when dropped.
pub struct FilteredReceiver {
    id: u64,
    router: MavlinkRouter,
    rx: mpsc::UnboundedReceiver<Arc<InboundMessage>>,
}

impl FilteredReceiver {
    fn new(
        router: &MavlinkRouter,
        filter: impl Fn(&InboundMessage) -> bool + Send + 'static,
    ) -> Self {
        let (tx, rx) = mpsc::unbounded_channel();
        let id = router.insert_waiter(filter, tx);
        FilteredReceiver {
            id,
            router: router.clone(),
            rx,
        }
    }

    pub async fn next(&mut self) -> Option<Arc<InboundMessage>> {
        self.rx.recv().await
    }
}

impl Drop for FilteredReceiver {
    fn drop(&mut self) {
        self.router.remove_waiter(self.id);
    }
}

/// A predicate used to select which received messages are accepted.
type MessageFilter<M> = Box<dyn FnMut(&M) -> bool + Send>;

/// Receives messages of type `M` from a specific source. Created by [`MavlinkRouter::recv`].
///
/// Await it directly to get the next matching message (with a timeout), or call
/// [`MessageReceiver::next`] repeatedly to process a sequence of messages.
pub struct MessageReceiver<M> {
    receiver: FilteredReceiver,
    filter: Option<MessageFilter<M>>,
    timeout: Duration,
}

impl<M: MavlinkMessage> MessageReceiver<M> {
    pub fn new(router: &MavlinkRouter, source: MavlinkId) -> Self {
        let message_id = M::message_id();
        let receiver = router.receive_filtered(move |inbound| {
            inbound.frame.message_id() == message_id && matches_target(source, inbound.source_mav)
        });

        MessageReceiver {
            receiver,
            filter: None,
            timeout: RECEIVE_TIMEOUT,
        }
    }

    /// Only accept messages for which `filter` returns true.
    pub fn filter(mut self, filter: impl FnMut(&M) -> bool + Send + 'static) -> Self {
        self.filter = Some(Box::new(filter));
        self
    }

    /// Timeout used when awaiting this [`MessageReceiver`] directly.
    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Wait for the next matching message, without any timeout.
    pub async fn next(&mut self) -> Option<M> {
        loop {
            let inbound = self.receiver.next().await?;
            let Ok(message) = inbound.frame.decode_message::<M>() else {
                continue;
            };

            if self.filter.as_mut().is_none_or(|filter| filter(&message)) {
                return Some(message);
            }
        }
    }
}

impl<M: MavlinkMessage> IntoFuture for MessageReceiver<M> {
    type Output = Result<M>;
    type IntoFuture = BoxFuture<'static, Result<M>>;

    fn into_future(mut self) -> Self::IntoFuture {
        Box::pin(async move {
            tokio::time::timeout(self.timeout, self.next())
                .await
                .ok()
                .flatten()
                .ok_or(MavError::Timeout)
        })
    }
}
