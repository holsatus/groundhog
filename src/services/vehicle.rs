//! Intent-level operations on a single vehicle.

use mav_param::{Ident, Value};
use mavio::default_dialect::{
    enums::{MavMissionType, MavProtocolCapability},
    messages::{AutopilotVersion, CommandAck, ComponentInformationBasic},
};

use crate::{
    domain::mission::{Mission, MissionItem},
    foundation::MavlinkId,
    mavlink::{self, Command, MavlinkMessage, MavlinkRouter, ParamListSummary, Targeted, commands},
};

/// An intent-level handle for operating a single vehicle through the router.
///
/// Cheap to clone: it holds the router (an `Arc`) and the target id, so callers
/// never thread the target through every call.
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
    #[allow(dead_code)] // Used by tests and future callers
    pub fn id(&self) -> MavlinkId {
        self.id
    }

    /// Download the vehicle's parameter list, optionally requesting capabilities first.
    pub async fn fetch_parameters(
        &self,
        request_capabilities: bool,
    ) -> mavlink::Result<ParamListSummary> {
        self.router.param_list(self.id, request_capabilities).await
    }

    /// Set one parameter, returning the value the vehicle accepted.
    pub async fn set_parameter(
        &self,
        ident: Ident,
        value: Value,
        capabilities: MavProtocolCapability,
    ) -> mavlink::Result<Value> {
        self.router
            .param_set(self.id, ident, value, capabilities)
            .await
    }

    /// Request the vehicle's `COMPONENT_INFORMATION_BASIC`.
    pub async fn component_information(&self) -> mavlink::Result<ComponentInformationBasic> {
        self.request().await
    }

    /// Request the vehicle's `AUTOPILOT_VERSION`.
    #[allow(dead_code)] // Escape hatch retained for future operations
    pub async fn autopilot_version(&self) -> mavlink::Result<AutopilotVersion> {
        self.request().await
    }

    /// Escape hatch: request and await one message of type `M`.
    pub async fn request<M: MavlinkMessage>(&self) -> mavlink::Result<M> {
        self.router.request_message::<M>(self.id).await
    }

    /// Escape hatch: run a command and await its final ACK.
    #[allow(dead_code)] // Escape hatch retained for future operations
    pub async fn run(&self, command: impl Into<Command>) -> mavlink::Result<CommandAck> {
        self.router.command(self.id, command).await
    }

    /// Escape hatch: send a targeted message without awaiting a response.
    #[allow(dead_code)] // Escape hatch retained for future operations
    pub fn send<M: Targeted>(&self, message: M) -> mavlink::Result<()> {
        self.router.send_targeted(self.id, message)
    }

    /// Download the vehicle's flight plan, replacing the caller's copy.
    pub async fn download_mission(&self) -> mavlink::Result<Mission> {
        let downloaded = self
            .router
            .mission_download(self.id, MavMissionType::Mission)
            .await?;

        let items = downloaded
            .items
            .iter()
            .map(MissionItem::from_wire)
            .collect::<Vec<_>>();

        let mut mission = Mission::default();
        mission.set_items(items);
        mission.mark_synced(Some(downloaded.vehicle_id));
        Ok(mission)
    }

    /// Upload `mission` to the vehicle; returns the vehicle's opaque plan id.
    pub async fn upload_mission(&self, mission: &Mission) -> mavlink::Result<u32> {
        let wire = mission
            .items()
            .iter()
            .enumerate()
            .map(|(seq, item)| item.to_wire(seq as u16, MavMissionType::Mission))
            .collect::<Vec<_>>();

        self.router
            .mission_upload(self.id, MavMissionType::Mission, &wire)
            .await
    }

    /// Clear the flight plan on the vehicle.
    pub async fn clear_mission(&self) -> mavlink::Result<()> {
        self.router
            .mission_clear(self.id, MavMissionType::Mission)
            .await
    }

    /// Ask the vehicle to make item `seq` the current execution point.
    pub async fn set_current_mission_item(&self, seq: u16) -> mavlink::Result<()> {
        self.router.mission_set_current(self.id, seq).await
    }

    /// Start executing the stored mission.
    pub async fn start_mission(&self) -> mavlink::Result<CommandAck> {
        self.run(commands::mission_start()).await
    }

    /// Pause (`true`) or resume (`false`) the running mission.
    pub async fn pause_mission(&self, pause: bool) -> mavlink::Result<CommandAck> {
        self.run(commands::pause_continue(pause)).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::foundation::DEFAULT_GCS_ID;

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
        assert!(client.upload_mission(&Mission::default()).await.is_err());
        assert!(client.clear_mission().await.is_err());
        assert!(client.set_current_mission_item(0).await.is_err());
        assert!(client.start_mission().await.is_err());
        assert!(client.pause_mission(true).await.is_err());
    }
}
