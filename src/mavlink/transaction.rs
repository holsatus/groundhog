use std::{future::IntoFuture, time::Duration};

use futures::future::BoxFuture;
use mavio::default_dialect::{
    enums::{MavCmd, MavResult},
    messages::{CommandAck, CommandCancel, CommandInt, CommandLong},
};
use tokio::sync::mpsc;

use super::{
    Targeted, commands,
    error::{MavError, Result},
    locks::LockKey,
    recv::MavlinkMessage,
    router::MavlinkRouter,
};
use crate::foundation::MavlinkId;

const RESPONSE_TIMEOUT: Duration = Duration::from_secs(1);
const PROGRESS_TIMEOUT: Duration = Duration::from_secs(5);
const RETRY_ATTEMPTS: u8 = 3;

/// How long to wait for a requested message after the request was accepted.
const REPLY_TIMEOUT: Duration = Duration::from_secs(2);

impl MavlinkRouter {
    /// Create a command transaction. Nothing is sent until it is awaited.
    pub fn command(&self, target: MavlinkId, command: impl Into<Command>) -> CommandRequest {
        CommandRequest {
            router: self.clone(),
            target,
            command: command.into(),
            response_timeout: RESPONSE_TIMEOUT,
            progress_timeout: PROGRESS_TIMEOUT,
            on_busy: Busy::default(),
            retries: RETRY_ATTEMPTS,
            locks: true,
        }
    }

    /// Request a single message using `MAV_CMD_REQUEST_MESSAGE` and wait for it.
    ///
    /// Concurrent requests for the same message queue behind each other via
    /// the command lock, so each request holds the pending-ACK registration
    /// (and its own reply filter) for the whole operation.
    pub async fn request_message<M: MavlinkMessage>(&self, target: MavlinkId) -> Result<M> {
        let command = commands::request_message::<M>();
        let _lock = self
            .lock_key(LockKey::command(target, command.kind()))
            .await;

        // The inner transaction must not re-acquire the same lock (it would
        // deadlock on itself); the outer key already serializes it.
        let mut reply = self.receive_message::<M>(target);
        let command = self.command(target, command).without_lock();

        // Some systems send the message before (or without) the ACK
        tokio::select! {
            Some(message) = reply.next() => return Ok(message),
            ack = command.into_future() => { ack?; }
        }

        reply.timeout(REPLY_TIMEOUT).await
    }
}

/// Either kind of MAVLink command message.
#[derive(Debug, Clone)]
pub enum Command {
    Long(CommandLong),
    Int(CommandInt),
}

impl From<CommandLong> for Command {
    fn from(command: CommandLong) -> Self {
        Command::Long(command)
    }
}

impl From<CommandInt> for Command {
    fn from(command: CommandInt) -> Self {
        Command::Int(command)
    }
}

impl Command {
    pub fn kind(&self) -> MavCmd {
        match self {
            Command::Long(command) => command.command,
            Command::Int(command) => command.command,
        }
    }

    fn prepare(&mut self, target: MavlinkId, attempt: u8) {
        match self {
            Command::Long(command) => {
                command.set_target(target);
                command.confirmation = attempt;
            }
            Command::Int(command) => command.set_target(target),
        }
    }

    fn send(&self, router: &MavlinkRouter, target: MavlinkId) -> Result<()> {
        match self {
            Command::Long(command) => router.route(target, command),
            Command::Int(command) => router.route(target, command),
        }
    }
}

/// What to do if a command of the same kind is already in flight to the target.
#[derive(Debug, Clone, Copy, Default)]
#[allow(dead_code)] // `Fail` is not used by the UI yet
pub enum Busy {
    /// Wait for the prior command(s) to finish
    #[default]
    Queue,
    /// Fail immediately with [`MavError::Busy`]
    Fail,
}

/// A command transaction, created by [`MavlinkRouter::command`]. Await it to run it.
///
/// Resolves to the final `COMMAND_ACK` if the command was accepted, and
/// [`MavError::Rejected`] otherwise. The command is resent if no ACK arrives in
/// time, and while the target reports `IN_PROGRESS` a longer timeout applies.
/// Dropping the future while the command is in progress sends `COMMAND_CANCEL`.
pub struct CommandRequest {
    router: MavlinkRouter,
    target: MavlinkId,
    command: Command,
    response_timeout: Duration,
    progress_timeout: Duration,
    on_busy: Busy,
    retries: u8,
    /// Whether `run` acquires the command lock itself; `false` only for a
    /// caller that already holds the command lock (it would deadlock on
    /// the same key otherwise).
    locks: bool,
}

#[allow(dead_code)] // Builder options not used by the UI yet
impl CommandRequest {
    /// How long to wait for an ACK before resending.
    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.response_timeout = timeout;
        self
    }

    /// How many times to resend the command if no ACK is received.
    pub fn retries(mut self, retries: u8) -> Self {
        self.retries = retries;
        self
    }

    /// How long to wait between `IN_PROGRESS` updates.
    pub fn progress_timeout(mut self, timeout: Duration) -> Self {
        self.progress_timeout = timeout;
        self
    }

    pub fn on_busy(mut self, on_busy: Busy) -> Self {
        self.on_busy = on_busy;
        self
    }

    /// Skip acquiring the command lock in `run`: a caller that already holds
    /// the command lock must not deadlock on itself.
    fn without_lock(mut self) -> Self {
        self.locks = false;
        self
    }

    async fn run(self) -> Result<CommandAck> {
        let kind = self.command.kind();
        let key = LockKey::command(self.target, kind);
        let _lock = if !self.locks {
            None
        } else {
            Some(match self.on_busy {
                Busy::Queue => self.router.lock_key(key).await,
                Busy::Fail => self
                    .router
                    .try_lock_key(key.clone())
                    .ok_or(MavError::Busy {
                        target: self.target,
                    })?,
            })
        };

        let mut pending = Pending::register(self.router.clone(), self.target, kind);
        let mut command = self.command;

        for attempt in 0..=self.retries {
            if attempt > 0 {
                log::debug!("Resending {kind:?} to {:?} ({attempt})", self.target);
            }

            command.prepare(self.target, attempt);
            command.send(&self.router, self.target)?;

            let mut timeout = self.response_timeout;
            while let Ok(ack) = tokio::time::timeout(timeout, pending.acks.recv()).await {
                let Some(ack) = ack else {
                    return Err(MavError::Timeout);
                };

                match ack.result {
                    MavResult::InProgress => {
                        log::debug!("{kind:?} in progress: {}%", ack.progress);
                        pending.in_progress = true;
                        timeout = self.progress_timeout;
                    }
                    MavResult::Accepted => {
                        pending.in_progress = false;
                        return Ok(ack);
                    }
                    result => {
                        pending.in_progress = false;
                        return Err(MavError::Rejected(result));
                    }
                }
            }

            // Do not resend while the target is working on it
            if pending.in_progress {
                break;
            }
        }

        Err(MavError::Timeout)
    }
}

impl IntoFuture for CommandRequest {
    type Output = Result<CommandAck>;
    type IntoFuture = BoxFuture<'static, Result<CommandAck>>;

    fn into_future(self) -> Self::IntoFuture {
        Box::pin(self.run())
    }
}

/// Registration in the pending command table. Removed on drop.
struct Pending {
    router: MavlinkRouter,
    target: MavlinkId,
    kind: MavCmd,
    acks: mpsc::UnboundedReceiver<CommandAck>,
    in_progress: bool,
}

impl Pending {
    fn register(router: MavlinkRouter, target: MavlinkId, kind: MavCmd) -> Self {
        let (tx, acks) = mpsc::unbounded_channel();
        router.insert_pending(target, kind, tx);
        Pending {
            router,
            target,
            kind,
            acks,
            in_progress: false,
        }
    }
}

impl Drop for Pending {
    fn drop(&mut self) {
        self.router.remove_pending(self.target, self.kind);

        if self.in_progress {
            log::debug!("Cancelling {:?} to {:?}", self.kind, self.target);
            let cancel = CommandCancel {
                target_system: self.target.system,
                target_component: self.target.component,
                command: self.kind,
            };
            _ = self.router.route(self.target, &cancel);
        }
    }
}
