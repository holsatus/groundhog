use std::{collections::BTreeSet, time::Duration};

use mav_param::{Ident, Value};
use mavio::default_dialect::{
    enums::MavProtocolCapability,
    messages::{AutopilotVersion, ParamRequestList, ParamRequestRead, ParamSet, ParamValue},
};
use tokio::time::timeout;

use super::{
    error::{MavError, Result},
    locks::LockKey,
    router::MavlinkRouter,
};
use crate::foundation::MavlinkId;

const PARAM_SET_TIMEOUT: Duration = Duration::from_secs(1);
const PARAM_SET_RETRIES: u8 = 3;

const PARAM_LIST_IDLE_TIMEOUT: Duration = Duration::from_secs(1);
const PARAM_LIST_RETRIES: u8 = 3;

impl MavlinkRouter {
    /// Set a parameter on `target` and wait for it to echo back its new value.
    ///
    /// Returns the value reported by the target, which may differ from `value`
    /// if the target rejected or clamped it. Only one set per parameter is in
    /// flight at once, and the total number of sets in flight is limited.
    pub async fn param_set(
        &self,
        target: MavlinkId,
        ident: Ident,
        value: Value,
        cap: MavProtocolCapability,
    ) -> Result<Value> {
        let (param_value, param_type) =
            crate::wire::param::mavio_from_value(cap, value).ok_or(MavError::ParamEncoding)?;

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

            if let Ok(Some(reply)) = timeout(PARAM_SET_TIMEOUT, recv_value.next()).await {
                return crate::wire::param::mavio_into_value(
                    cap,
                    reply.param_value,
                    reply.param_type,
                )
                .ok_or(MavError::ParamEncoding);
            }
        }

        Err(MavError::Timeout)
    }

    /// Download the full parameter list of `target`, re-requesting any missing parameters.
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
        let _lock = self.lock_key(LockKey::ParamList(target)).await;

        if request_capabilities {
            // Not all systems respect this part of the standard. If all else
            // fails, parameters are assumed to use bytewise encoding.
            if let Err(error) = self.request_message::<AutopilotVersion>(target).await {
                log::warn!("No AUTOPILOT_VERSION from {target:?}: {error}");
            }
        }

        let mut recv_values = self.receive_message::<ParamValue>(target);
        let mut received = BTreeSet::new();
        let mut expected = None;
        let mut retries = 0;

        self.send_targeted(target, ParamRequestList::default())?;

        loop {
            if let Some(count) = expected
                && received.len() >= count as usize
            {
                break;
            }

            match timeout(PARAM_LIST_IDLE_TIMEOUT, recv_values.next()).await {
                Ok(Some(value)) => {
                    retries = 0;
                    if value.param_index < value.param_count {
                        expected = Some(value.param_count);
                        received.insert(value.param_index);
                    }
                }
                Ok(None) => return Err(MavError::Timeout),
                Err(_) if retries >= PARAM_LIST_RETRIES => break,
                Err(_) => {
                    retries += 1;
                    match expected {
                        // We do not know how many we are missing, so request all again
                        None => self.send_targeted(target, ParamRequestList::default())?,
                        Some(count) => {
                            for index in 0..count {
                                if !received.contains(&index) {
                                    self.send_targeted(
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
        }

        let expected = expected.ok_or(MavError::Timeout)?;
        Ok((received.len(), expected))
    }
}
