//! The pending-ACK table.
//!
//! Command transactions register a sender here before they send, keyed by the
//! target and command kind. Inbound `COMMAND_ACK`s are matched back to the
//! waiting transaction; an unmatched ACK is logged as unsolicited.

use std::{collections::BTreeMap, sync::Mutex};

use mavio::default_dialect::{enums::MavCmd, messages::CommandAck};
use tokio::sync::mpsc;

use super::lock;
use crate::foundation::MavlinkId;

/// The senders awaiting a command ACK, keyed by `(target, command kind)`.
#[derive(Default)]
pub(super) struct PendingAckMap {
    map: Mutex<BTreeMap<(MavlinkId, u16), mpsc::UnboundedSender<CommandAck>>>,
}

impl PendingAckMap {
    /// Forward the ACKs for `kind` commands from `target` to `tx`, until removed.
    ///
    /// Calls are serialized per `(target, kind)` by their transactions; this
    /// replaces only a stale sender.
    pub(super) fn insert(
        &self,
        target: MavlinkId,
        kind: MavCmd,
        tx: mpsc::UnboundedSender<CommandAck>,
    ) {
        lock(&self.map).insert((target, kind as u16), tx);
    }

    pub(super) fn remove(&self, target: MavlinkId, kind: MavCmd) {
        lock(&self.map).remove(&(target, kind as u16));
    }

    /// Route `ack`, received from `source`, to the transaction waiting for it.
    ///
    /// Some systems leave the target fields empty, so those are accepted too.
    pub(super) fn route_ack(&self, gcs_id: MavlinkId, source: MavlinkId, ack: &CommandAck) {
        // Some systems leave the target fields empty, so accept those as well
        let for_us = (ack.target_system == 0 || ack.target_system == gcs_id.system)
            && (ack.target_component == 0 || ack.target_component == gcs_id.component);
        if !for_us {
            return;
        }

        let command = ack.command as u16;
        let system_wide = MavlinkId {
            system: source.system,
            component: 0,
        };

        let pending = lock(&self.map);
        let sender = pending
            .get(&(source, command))
            .or_else(|| pending.get(&(system_wide, command)));

        match sender {
            Some(sender) => _ = sender.send(ack.clone()),
            None => log::debug!("Unsolicited ACK from {source:?}: {ack:?}"),
        }
    }
}
