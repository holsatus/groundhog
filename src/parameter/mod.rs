use std::{path::PathBuf, time::Duration};

use iced::Task;
use mav_param::{Ident, Value};
use mavio::default_dialect::{
    enums::{MavCmd, MavProtocolCapability},
    messages::{AutopilotVersion, CommandInt, ParamRequestList, ParamSet},
};

use crate::parameter::base::MavlinkId;

pub mod base;
use base::*;

#[derive(Debug, Clone)]
pub enum Message {
    /// Filter for only certain parameters
    FilterBuf(String),

    /// Initiate a paramter list request to the target
    ListReload(MavlinkId),

    /// Upload all modified parameters to the target
    UploadAll(MavlinkId),

    /// Reset a local parameter to its target-default
    ValueReset(MavlinkId, Ident),

    /// Upload a local parameter to the target
    ValueUpload(MavlinkId, Ident, Value),

    /// Upload a local parameter to the target
    ValueUploadTimeout(MavlinkId, Ident),

    /// Edit the text-value buffer of the local parameter
    BufferEdit(MavlinkId, Ident, String),

    /// Open a file-picker dialog to choose a save location
    SaveDialog(base::Parameters),

    /// Save the parameters to the chosen path
    SaveToFile(PathBuf, base::Parameters),

    /// Open a file-picker dialog to choose file to load
    LoadDialog(MavlinkId),

    /// Load parameters from the chosen path
    LoadFromFile(PathBuf, MavlinkId),
}

impl crate::Application {
    pub fn parameter_message_update(&mut self, message: Message) -> Option<Task<crate::Message>> {
        match message {
            Message::FilterBuf(buffer) => {
                self.parameter_filter = buffer;

                if self.parameter_filter.is_empty() {
                    self.parameter_filtered = None;
                    return None;
                }

                let mav_id = self.primary_vehicle?;
                let vehicle = self.vehicles.get(&mav_id)?;

                let param_map = vehicle.params.map.iter().filter_map(|(ident, param)| {
                    ident
                        .as_str()
                        .to_lowercase()
                        .contains(&self.parameter_filter.to_lowercase())
                        .then_some((ident.clone(), param.clone()))
                });

                self.parameter_filtered = Some(Parameters {
                    map: param_map.collect(),
                    loading_state: vehicle.params.loading_state.clone(),
                });
            }

            Message::ListReload(mav_id) => {
                let connection = self.get_connection_handle()?;
                let vehicle = self.vehicles.get_mut(&mav_id)?;

                let get_capabilities = vehicle.capabilities.is_none();
                vehicle.params.loading_state.has_loaded.clear();

                tokio::spawn(async move {
                    if get_capabilities {
                        log::debug!("Requesting capabilities");
                        connection
                            .send_message(CommandInt {
                                target_system: mav_id.system,
                                target_component: mav_id.component,
                                command: MavCmd::RequestMessage,
                                param1: AutopilotVersion::ID as f32,
                                ..Default::default()
                            })
                            .await;

                        // Give MAV some time to respond in right order
                        tokio::time::sleep(Duration::from_millis(100)).await;
                    }

                    log::debug!("Requesting parameters");
                    connection
                        .send_message(ParamRequestList {
                            target_system: mav_id.system,
                            target_component: mav_id.component,
                        })
                        .await;
                });
            }
            Message::UploadAll(mav_id) => {
                let connection = self.get_connection_handle()?;
                let vehicle = self.vehicles.get_mut(&mav_id)?;

                let mut timeout_tasks = Vec::new();

                // Loop over all changed parameters and do a param set request.
                // Mark the parameter as uploading to keep track of things.
                for (ident, param, value) in
                    vehicle
                        .params
                        .map
                        .iter_mut()
                        .filter_map(|(ident, param)| match param.state {
                            ParamState::Changed(value) => Some((ident, param, value)),
                            _ => None,
                        })
                {
                    let ident_cloned = ident.clone();
                    let (task, handle) = Task::future(async move {
                        tokio::time::sleep(Duration::from_secs(2)).await;
                        Message::ValueUploadTimeout(mav_id, ident_cloned)
                    })
                    .abortable();

                    param.state = ParamState::Uploading(handle, param.value);

                    let cap = vehicle.capabilities?;
                    let (param_value, param_type) = base::from_value(cap, value)?;

                    let param_set = ParamSet {
                        target_system: mav_id.system,
                        target_component: mav_id.component,
                        param_id: *ident.as_raw(),
                        param_value,
                        param_type,
                    };

                    connection.spawn_send_message(param_set);

                    timeout_tasks.push(task);
                }

                return Some(Task::batch(timeout_tasks).map(crate::Message::Parameter));
            }
            Message::BufferEdit(mav_id, ident, buffer) => {
                let entry = self.vehicles.get_mut(&mav_id)?;
                let param = entry.params.map.get_mut(&ident)?;

                param.state = ParamState::Unchanged;

                if let Some(new_value) = base::value_parse_as(param.value, &buffer)
                    && new_value != param.value
                {
                    param.state = ParamState::Changed(new_value);
                }

                param.editing = Some(buffer);
            }
            Message::ValueReset(mav_id, ident) => {
                let entry = self.vehicles.get_mut(&mav_id)?;
                let param = entry.params.map.get_mut(&ident)?;

                param.editing = None;
                param.state = ParamState::Unchanged;
            }
            Message::ValueUpload(mav_id, ident, value) => {
                let connection = self.get_connection_handle()?;
                let vehicle = self.vehicles.get_mut(&mav_id)?;
                let param = vehicle.params.map.get_mut(&ident)?;

                let ident_cloned = ident.clone();
                let (timeout_task, handle) = Task::future(async move {
                    tokio::time::sleep(Duration::from_secs(2)).await;
                    Message::ValueUploadTimeout(mav_id, ident_cloned)
                })
                .abortable();

                param.state = ParamState::Uploading(handle, param.value);

                let cap = vehicle.capabilities?;
                let (param_value, param_type) =
                    if cap.contains(MavProtocolCapability::PARAM_ENCODE_BYTEWISE) {
                        base::value_into_bytewise(value)
                    } else if cap.contains(MavProtocolCapability::PARAM_ENCODE_C_CAST) {
                        base::value_into_c_cast(value)
                    } else {
                        log::error!("Parameter encoding type not known for vehicle");
                        return None;
                    };

                let param_set = ParamSet {
                    target_system: mav_id.system,
                    target_component: mav_id.component,
                    param_id: *ident.as_raw(),
                    param_value,
                    param_type,
                };

                tokio::spawn(async move {
                    connection.send_message(param_set).await;
                });

                return Some(timeout_task.map(crate::Message::Parameter));
            }
            Message::ValueUploadTimeout(mav_id, ident) => {
                let entry = self.vehicles.get_mut(&mav_id)?;
                let param = entry.params.map.get_mut(&ident)?;

                if let ParamState::Uploading(handle, value) = param.state.clone() {
                    log::warn!("Parameter upload for '{}' timed out", ident.as_str());
                    param.state = ParamState::Changed(value);
                    handle.abort();
                }
            }
            Message::SaveDialog(parameters) => {
                let file_dialog = self.new_file_dialog();

                return Some(
                    Task::future(async move {
                        file_dialog.save_file().await.map(|file| (file, parameters))
                    })
                    .and_then(|(file, parameters)| {
                        Task::done(Message::SaveToFile(file.path().to_owned(), parameters))
                    })
                    .map(crate::Message::Parameter),
                );
            }
            Message::SaveToFile(save_path, parameters) => {
                log::info!("Saving parameters to file: {save_path:?}");

                let result = base::save_parameters_to_ini(&save_path, parameters);
                if let Err(error) = result {
                    log::error!("Error saving to file: {}", error);
                }

                self.set_file_picker_path_config(save_path.as_path());
            }
            Message::LoadDialog(mav_id) => {
                let file_dialog = self.new_file_dialog();

                return Some(
                    Task::future(async move {
                        file_dialog.pick_file().await.map(|file| (file, mav_id))
                    })
                    .and_then(|(file, mav_id)| {
                        Task::done(Message::LoadFromFile(file.path().to_owned(), mav_id))
                    })
                    .map(crate::Message::Parameter),
                );
            }
            Message::LoadFromFile(load_path, mav_id) => {
                log::info!("Loading parameters from file: {load_path:?}");

                let vehicle = self.vehicles.get_mut(&mav_id)?;
                let loaded_params = match base::load_parameters_from_ini(&load_path) {
                    Ok(loaded_params) => loaded_params,
                    Err(error) => {
                        log::error!("Could not load parameter file: {}", error);
                        return None;
                    }
                };

                // Set the parameters of the vehicle as modified if that is the case
                for (ident, new_param) in loaded_params.map.iter() {
                    if let Some(old_param) = vehicle.params.map.get_mut(ident)
                        && base::value_type_matches(old_param.value, new_param.value)
                        && old_param.value != new_param.value
                    {
                        old_param.state = ParamState::Changed(new_param.value);
                        old_param.editing = Some(base::value_as_string(new_param.value));
                    }
                }

                self.set_file_picker_path_config(load_path.as_path());
            }
        }

        None
    }
}
