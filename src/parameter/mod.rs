use std::path::PathBuf;

use iced::Task;
use mav_param::{Ident, Value};

use crate::{mav, parameter::base::MavlinkId};

pub mod base;
use base::*;

#[derive(Default, PartialEq)]
pub struct ParameterFilter {
    pub ident_pattern: String,
    pub only_changed: bool,
}

#[derive(Debug, Clone)]
pub enum Message {
    /// Filter for only certain parameters
    FilterIdentPattern(String),

    /// Whether to only show changed parameters
    FilterOnlyChanged(bool),

    /// Initiate a paramter list request to the target
    ListReload(MavlinkId),

    /// The parameter list request to the target has finished
    ListReloadResult(MavlinkId, mav::Result<mav::ParamListSummary>),

    /// Upload all modified parameters to the target
    UploadAll(MavlinkId),

    /// Reset a local parameter to its target-default
    ValueReset(MavlinkId, Ident),

    /// Upload a local parameter to the target
    ValueUpload(MavlinkId, Ident, Value),

    /// The result of uploading a local parameter to the target
    ValueUploadResult(MavlinkId, Ident, Value, mav::Result<Value>),

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
    pub fn refresh_filtered_parameter(&mut self) -> Option<&Parameters> {
        // Shortcut if no filter has been applied
        if self.parameter_filter == ParameterFilter::default() {
            self.parameter_filtered = None;
            return None;
        }

        let filter_string_lower = self.parameter_filter.ident_pattern.to_lowercase();

        let mav_id = self.primary_vehicle?;
        let vehicle = self.vehicles.get(&mav_id)?;

        let param_map = vehicle
            .parameters
            .map
            .iter()
            .filter(|(_, param)| {
                !self.parameter_filter.only_changed || !matches!(param.state, ParamState::Unchanged)
            })
            .filter(|(ident, _)| {
                filter_string_lower.is_empty()
                    || ident.as_str().to_lowercase().contains(&filter_string_lower)
            })
            .map(|(ident, param)| (ident.clone(), param.clone()));

        self.parameter_filtered = Some(Parameters {
            map: param_map.collect(),
            loading_state: vehicle.parameters.loading_state.clone(),
        });

        self.parameter_filtered.as_ref()
    }

    pub fn parameter_message_update(&mut self, message: Message) -> Option<Task<crate::Message>> {
        match message {
            Message::FilterIdentPattern(buffer) => {
                self.parameter_filter.ident_pattern = buffer;
            }
            Message::FilterOnlyChanged(changed) => {
                self.parameter_filter.only_changed = changed;
            }
            Message::ListReload(mav_id) => {
                let mav = self.router.clone();
                let vehicle = self.vehicles.get_mut(&mav_id)?;

                let request_capabilities = vehicle.capabilities.is_none();

                vehicle.parameters.loading_state.has_loaded.clear();

                let task = Task::perform(
                    async move { mav.param_list(mav_id, request_capabilities).await },
                    move |result| Message::ListReloadResult(mav_id, result),
                );

                return Some(task.map(crate::Message::Parameter));
            }
            Message::ListReloadResult(mav_id, result) => match result {
                Ok(summary) => log::info!("Parameters from {mav_id:?}: {summary:?}"),
                Err(error) => log::warn!("Parameter download from {mav_id:?} failed: {error}"),
            },
            Message::UploadAll(mav_id) => {
                let vehicle = self.vehicles.get(&mav_id)?;

                // The number of concurrent uploads is limited by the middleware
                let uploads: Vec<_> = vehicle
                    .parameters
                    .map
                    .iter()
                    .filter_map(|(ident, param)| match param.state {
                        ParamState::Changed { value, .. } => Some(Task::done(
                            Message::ValueUpload(mav_id, ident.clone(), value),
                        )),
                        _ => None,
                    })
                    .collect();

                log::info!("Uploading {} changed parameters", uploads.len());
                return Some(Task::batch(uploads).map(crate::Message::Parameter));
            }
            Message::BufferEdit(mav_id, ident, buffer) => {
                let entry = self.vehicles.get_mut(&mav_id)?;
                let param = entry.parameters.map.get_mut(&ident)?;

                param.state = ParamState::Unchanged;

                if let Some(new_value) = base::value_parse_as(param.value, &buffer)
                    && new_value != param.value
                {
                    param.state = ParamState::Changed {
                        value: new_value,
                        status: ChangedStatus::None,
                    };
                }

                param.editing = Some(buffer);
            }
            Message::ValueReset(mav_id, ident) => {
                let entry = self.vehicles.get_mut(&mav_id)?;
                let param = entry.parameters.map.get_mut(&ident)?;

                param.editing = None;
                param.state = ParamState::Unchanged;
            }
            Message::ValueUpload(mav_id, ident, value) => {
                let mav = self.router.clone();
                let vehicle = self.vehicles.get_mut(&mav_id)?;
                let param = vehicle.parameters.map.get_mut(&ident)?;
                let cap = vehicle.capabilities.unwrap_or_default();

                let ident_cloned = ident.clone();
                let (task, handle) = Task::perform(
                    async move { mav.param_set(mav_id, ident_cloned, value, cap).await },
                    move |result| Message::ValueUploadResult(mav_id, ident.clone(), value, result),
                )
                .abortable();

                // Save the value we are currently uploading. Dropping the state
                // (e.g. by editing the value again) aborts the upload.
                param.state = ParamState::Uploading {
                    value,
                    handle: handle.abort_on_drop(),
                };

                return Some(task.map(crate::Message::Parameter));
            }
            Message::ValueUploadResult(mav_id, ident, requested, result) => {
                let vehicle = self.vehicles.get_mut(&mav_id)?;
                let param = vehicle.parameters.map.get_mut(&ident)?;

                if !matches!(param.state, ParamState::Uploading { value, .. } if value == requested)
                {
                    return None;
                }

                match result {
                    Ok(value) if value == requested => {
                        param.value = value;
                        param.state = ParamState::Unchanged;
                    }
                    Ok(value) => {
                        log::warn!(
                            "Parameter '{}' was set to {value:?} instead of {requested:?}",
                            ident.as_str()
                        );
                        param.value = value;
                        param.state = ParamState::Changed {
                            value: requested,
                            status: ChangedStatus::Rejected,
                        };
                    }
                    Err(error) => {
                        log::warn!("Parameter upload for '{}' failed: {error}", ident.as_str());
                        param.state = ParamState::Changed {
                            value: requested,
                            status: ChangedStatus::Failed,
                        };
                    }
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
                    if let Some(old_param) = vehicle.parameters.map.get_mut(ident)
                        && base::value_type_matches(old_param.value, new_param.value)
                        && old_param.value != new_param.value
                    {
                        old_param.state = ParamState::Changed {
                            value: new_param.value,
                            status: ChangedStatus::None,
                        };
                        old_param.editing = Some(base::value_as_string(new_param.value));
                    }
                }

                self.set_file_picker_path_config(load_path.as_path());
            }
        }

        self.refresh_filtered_parameter()?;

        None
    }
}
