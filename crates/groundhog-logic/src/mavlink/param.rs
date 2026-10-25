//! Parameter transactions: setting one parameter, and downloading the list.
//!
//! A set waits for the target to echo the parameter's new value, which may
//! differ from the value sent. The echo carries nothing that ties it to the
//! set: no transaction id, and an index that both ArduPilot and PX4 also use
//! for other `PARAM_VALUE`s of the same parameter (a list download's, a later
//! read's). So a reply reporting the requested value is taken as the echo at
//! once, and one reporting another value only if nothing better arrives
//! before the attempt's deadline. A list download requests every parameter,
//! then re-requests the missing ones by index, in small batches, while the
//! target keeps answering. The values themselves reach vehicle state through
//! the inbound message stream, like any other message.
//!
//! A download and a set cannot share the reply channel: the download would
//! answer the set with an old value. A download holds
//! [`LockKey::ParamChannel`] exclusively and a set holds it shared, so a set
//! waits for a running download while sets of different parameters overlap.

use std::{collections::BTreeSet, time::Duration};

use mav_param::{Ident, Value};
use mavio::default_dialect::messages::{
    AutopilotVersion, ComponentInformationBasic, ParamRequestList, ParamRequestRead, ParamSet,
    ParamValue,
};
use tokio::time::{Instant, timeout_at};

use super::{
    error::{MavError, Result},
    locks::LockKey,
    router::MavlinkRouter,
};
use crate::{MavlinkId, wire::param::ParamEncoding};

const PARAM_SET_TIMEOUT: Duration = Duration::from_secs(1);
const PARAM_SET_RETRIES: u8 = 3;

pub(super) const PARAM_LIST_IDLE_TIMEOUT: Duration = Duration::from_secs(1);
pub(super) const PARAM_LIST_RETRIES: u8 = 3;

/// The most missing parameters re-requested per idle round. Re-requesting
/// hundreds at once floods a slow radio and mostly produces more losses; the
/// round repeats while the vehicle keeps answering.
const PARAM_REREQUEST_BATCH: usize = 32;

impl MavlinkRouter {
    /// Set a parameter on `target` and wait for it to echo back its new value.
    ///
    /// Returns the value reported by the target, which may differ from `value`
    /// if the target rejected or clamped it. Only one set per parameter is in
    /// flight at once, and the total number of sets in flight is limited.
    ///
    /// A reply reporting another value than `value` may be the echo of a
    /// clamped write, or a value someone else asked for (a list download, a
    /// late echo of an earlier write). It is returned only if no reply
    /// reporting `value` arrives before the attempt's deadline, so a clamped
    /// write reports up to [`PARAM_SET_TIMEOUT`] later than it arrived. Once
    /// such a reply is in hand, the set is not sent again: a clamping target
    /// would clamp again.
    pub async fn param_set(
        &self,
        target: MavlinkId,
        ident: Ident,
        value: Value,
        encoding: ParamEncoding,
    ) -> Result<Value> {
        let (param_value, param_type) = encoding.encode(value);

        // Share the target's reply channel with other sets, but keep list
        // downloads out: their values would otherwise answer this set.
        let _channel = self.lock_channel_shared(target).await;

        // Ensure we are the only ones setting *this* parameter
        let _lock = self.lock_key(LockKey::Param(target, ident.clone())).await;

        // Ensure we limit the total number in-flight ParamSet
        let _permit = self.param_permit().await?;

        let param_id = *ident.as_raw();
        let mut recv_value = self
            .receive_message::<ParamValue>(target)
            .filter(move |reply| reply.param_id == param_id);

        for attempt in 0..=PARAM_SET_RETRIES {
            if attempt > 0 {
                log::debug!("Resending PARAM_SET for '{}' ({attempt})", ident.as_str());
            }

            self.send_targeted(
                target,
                ParamSet {
                    param_id,
                    param_value,
                    param_type,
                    ..Default::default()
                },
            )?;

            let deadline = Instant::now() + PARAM_SET_TIMEOUT;
            let mut other = None;
            while let Ok(Some(reply)) = timeout_at(deadline, recv_value.next()).await {
                let reported = encoding
                    .decode(reply.param_value, reply.param_type)
                    .ok_or(MavError::ParamEncoding)?;
                if reported == value {
                    return Ok(reported);
                }
                // The newest: a list's stale value tends to precede the echo.
                other = Some(reported);
            }

            if let Some(reported) = other {
                return Ok(reported);
            }
        }

        Err(MavError::Timeout)
    }

    /// Download the full parameter list of `target`, re-requesting any missing parameters.
    ///
    /// Ends when every parameter arrived, or after [`PARAM_LIST_RETRIES`]
    /// rounds of [`PARAM_LIST_IDLE_TIMEOUT`] in a row in which no parameter
    /// arrived that was not received before.
    ///
    /// The received values are delivered to the application through the normal
    /// inbound message stream; this only drives the protocol to completion.
    /// If `request_capabilities` is set, `AUTOPILOT_VERSION` is requested first,
    /// since the capabilities determine how parameter values are encoded.
    ///
    /// Returns `(received, expected)`.
    pub async fn param_list(
        &self,
        target: MavlinkId,
        request_capabilities: bool,
    ) -> Result<(usize, u16)> {
        let _lock = self.lock_channel(target).await;

        if request_capabilities {
            // Both messages contain the capabilities flag, but AUTOPILOT_VERSION
            // is more likely to give a response, so try that first.
            if let Err(error) = self.request_message::<AutopilotVersion>(target).await {
                log::warn!("Failed to get AUTOPILOT_VERSION for {target:?}: {error}");
                if let Err(error) = self
                    .request_message::<ComponentInformationBasic>(target)
                    .await
                {
                    log::warn!("Failed to get COMPONENT_INFORMATION_BASIC for {target:?}: {error}");
                }
            }
        }

        let mut recv_values = self.receive_message::<ParamValue>(target);
        let mut received = BTreeSet::new();
        let mut expected: Option<u16> = None;

        // The download ends when every index arrived, or after
        // PARAM_LIST_RETRIES rounds in a row without progress, a round being
        // PARAM_LIST_IDLE_TIMEOUT without a parameter not received before.
        // Values that bring nothing new (a repeat, a set's echo at index
        // UINT16_MAX, another GCS's download) do not extend a round.
        let mut idle_rounds = 0;
        let mut round_ends = Instant::now() + PARAM_LIST_IDLE_TIMEOUT;

        self.send_targeted(target, ParamRequestList::default())?;

        while expected.is_none_or(|count| received.len() < usize::from(count)) {
            match timeout_at(round_ends, recv_values.next()).await {
                Ok(None) => return Err(MavError::Timeout),
                Ok(Some(value)) => {
                    // The first count stands; a value counting otherwise is
                    // from another list.
                    let count = *expected.get_or_insert(value.param_count);
                    if value.param_count == count
                        && value.param_index < count
                        && received.insert(value.param_index)
                    {
                        idle_rounds = 0;
                        round_ends = Instant::now() + PARAM_LIST_IDLE_TIMEOUT;
                    }
                }
                Err(_) => {
                    idle_rounds += 1;
                    if idle_rounds > PARAM_LIST_RETRIES {
                        break;
                    }
                    round_ends = Instant::now() + PARAM_LIST_IDLE_TIMEOUT;

                    match expected {
                        // We do not know how many we are missing, so request all again
                        None => self.send_targeted(target, ParamRequestList::default())?,
                        Some(count) => {
                            let missing = (0..count)
                                .filter(|index| !received.contains(index))
                                .take(PARAM_REREQUEST_BATCH);

                            for index in missing {
                                self.send_targeted_bulk(
                                    target,
                                    ParamRequestRead {
                                        param_index: index as i16,
                                        ..Default::default()
                                    },
                                )?;
                            }
                        }
                    }
                }
            }
        }

        let expected = expected.ok_or(MavError::Timeout)?;
        Ok((received.len(), expected))
    }
}
