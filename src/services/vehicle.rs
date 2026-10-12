//! Intent-level operations on a single vehicle.

use mav_param::{Ident, Value};
use mavio::default_dialect::{
    enums::{MavMissionType, MavProtocolCapability},
    messages::ComponentInformationBasic,
};

use crate::{
    domain::mission::{MissionItem, MissionPlan},
    foundation::MavlinkId,
    mavlink::{self, Command, MavlinkMessage, MavlinkRouter, commands},
};

use super::{Error, Result};

/// A summary of a parameter-list download: how many parameters arrived and how
/// many the target advertised.
#[derive(Debug, Clone, Copy)]
#[allow(dead_code)] // Only logged by the application for now
pub struct ParamListSummary {
    pub received: usize,
    pub expected: u16,
}

/// A plan downloaded from a vehicle, together with the opaque id the vehicle
/// reports for it. What the vehicle handed over is what the vehicle has, so the
/// fleet records it as synced under `vehicle_id`.
#[derive(Debug, Clone)]
pub struct DownloadedMission {
    pub plan: MissionPlan,
    pub vehicle_id: u32,
}

/// An intent-level handle for operating a single vehicle through the router.
///
/// Cheap to clone: it holds the router (an `Arc`) and the target id, so callers
/// never thread the target through every call.
///
/// Every public operation returns [`services::Result`](super::Result): the
/// middleware's error is mapped at this boundary, so no `mavio` or middleware
/// type appears in the surface.
#[derive(Clone)]
pub struct VehicleClient {
    router: MavlinkRouter,
    id: MavlinkId,
}

impl VehicleClient {
    /// Wrap `router` as a handle for the vehicle identified by `id`.
    pub fn new(router: &MavlinkRouter, id: MavlinkId) -> Self {
        Self {
            router: router.clone(),
            id,
        }
    }

    /// The target vehicle's MAVLink id.
    #[allow(dead_code)] // Escape hatch: unused yet
    pub fn id(&self) -> MavlinkId {
        self.id
    }

    /// Download the vehicle's parameter list, optionally requesting capabilities first.
    pub async fn fetch_parameters(&self, request_capabilities: bool) -> Result<ParamListSummary> {
        let (received, expected) = self
            .router
            .param_list(self.id, request_capabilities)
            .await?;
        Ok(ParamListSummary { received, expected })
    }

    /// Set one parameter, returning the value the vehicle accepted.
    pub async fn set_parameter(
        &self,
        ident: Ident,
        value: Value,
        capabilities: MavProtocolCapability,
    ) -> Result<Value> {
        self.router
            .param_set(self.id, ident, value, capabilities)
            .await
            .map_err(Error::from)
    }

    /// Request the vehicle's `COMPONENT_INFORMATION_BASIC`.
    ///
    /// The value itself is not needed by the caller (the vehicle folds the
    /// message into its own state), so only success is reported.
    pub async fn component_information(&self) -> Result<()> {
        self.request::<ComponentInformationBasic>()
            .await
            .map(|_| ())
            .map_err(Error::from)
    }

    /// Escape hatch: request and await one message of type `M`.
    pub(crate) async fn request<M: MavlinkMessage>(&self) -> mavlink::Result<M> {
        self.router.request_message::<M>(self.id).await
    }

    /// Run a command and await its final ACK, discarding the ACK itself: only
    /// whether it succeeded matters to callers.
    pub async fn run(&self, command: impl Into<Command>) -> Result<()> {
        self.router
            .command(self.id, command)
            .await
            .map(|_| ())
            .map_err(Error::from)
    }

    /// Download the vehicle's flight plan.
    pub async fn download_mission(&self) -> Result<DownloadedMission> {
        let downloaded = self
            .router
            .mission_download(self.id, MavMissionType::Mission)
            .await
            .map_err(Error::from)?;

        let items = downloaded
            .items
            .iter()
            .map(MissionItem::from_wire)
            .collect::<Vec<_>>();

        let mut plan = MissionPlan::default();
        plan.set_items(items);

        Ok(DownloadedMission {
            plan,
            vehicle_id: downloaded.vehicle_id,
        })
    }

    /// Upload `plan` to the vehicle; returns the vehicle's opaque plan id.
    pub async fn upload_mission(&self, plan: &MissionPlan) -> Result<u32> {
        let wire = plan
            .items()
            .iter()
            .enumerate()
            .map(|(seq, item)| item.to_wire(seq as u16, MavMissionType::Mission))
            .collect::<Vec<_>>();

        self.router
            .mission_upload(self.id, MavMissionType::Mission, &wire)
            .await
            .map_err(Error::from)
    }

    /// Clear the flight plan on the vehicle.
    pub async fn clear_mission(&self) -> Result<()> {
        self.router
            .mission_clear(self.id, MavMissionType::Mission)
            .await
            .map_err(Error::from)
    }

    /// Ask the vehicle to make item `seq` the current execution point.
    pub async fn set_current_mission_item(&self, seq: u16) -> Result<()> {
        self.router
            .mission_set_current(self.id, seq)
            .await
            .map_err(Error::from)
    }

    /// Start executing the stored mission.
    pub async fn start_mission(&self) -> Result<()> {
        self.run(commands::mission_start()).await
    }

    /// Pause (`true`) or resume (`false`) the running mission.
    pub async fn pause_mission(&self, pause: bool) -> Result<()> {
        self.run(commands::pause_continue(pause)).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mavlink::DEFAULT_GCS_ID;

    const VEHICLE: MavlinkId = MavlinkId {
        system: 1,
        component: 1,
    };

    #[test]
    fn id_is_returned() {
        let (router, _events) = MavlinkRouter::new(DEFAULT_GCS_ID);
        let client = VehicleClient::new(&router, VEHICLE);
        assert_eq!(client.id(), VEHICLE);
    }

    #[tokio::test]
    async fn operations_without_a_link_error() {
        let (router, _events) = MavlinkRouter::new(DEFAULT_GCS_ID);
        let client = VehicleClient::new(&router, VEHICLE);

        assert!(client.fetch_parameters(false).await.is_err());
        assert!(client.component_information().await.is_err());
        assert!(
            client
                .set_parameter(
                    Ident::from_str_truncated("PARAM_ONE"),
                    Value::F32(1.0),
                    MavProtocolCapability::default(),
                )
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn mission_operations_without_a_link_error() {
        let (router, _events) = MavlinkRouter::new(DEFAULT_GCS_ID);
        let client = VehicleClient::new(&router, VEHICLE);

        assert!(client.download_mission().await.is_err());
        assert!(
            client
                .upload_mission(&MissionPlan::default())
                .await
                .is_err()
        );
        assert!(client.clear_mission().await.is_err());
        assert!(client.set_current_mission_item(0).await.is_err());
        assert!(client.start_mission().await.is_err());
        assert!(client.pause_mission(true).await.is_err());
    }
}
