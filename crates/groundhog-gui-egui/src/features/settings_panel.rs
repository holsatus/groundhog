//! The application settings panel.
//!
//! A direct editor over [`Settings`], which the application owns and
//! persists. Each control mutates the settings in place; the return value
//! tells the application whether to persist.

use crate::config::Settings;

/// What changed, so the application can react to toggles beyond persisting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SettingsChanges {
    pub any: bool,
    /// The telemetry-recording switch moved (start/stop handling).
    pub record_telemetry: bool,
}

/// Draw the panel, applying changes to `settings` in place.
pub fn view(
    ui: &mut egui::Ui,
    settings: &mut Settings,
    recording: Option<&std::path::Path>,
) -> SettingsChanges {
    let mut changes = SettingsChanges {
        any: false,
        record_telemetry: false,
    };

    ui.heading("Application settings");

    ui.horizontal(|ui| {
        ui.label(egui::RichText::new("Theme").weak());
        if ui.selectable_label(settings.dark, "Dark").clicked() && !settings.dark {
            settings.dark = true;
            changes.any = true;
        }
        if ui.selectable_label(!settings.dark, "Light").clicked() && settings.dark {
            settings.dark = false;
            changes.any = true;
        }
    });

    let mut scale = settings.scale_factor;
    ui.horizontal(|ui| {
        ui.label("Scale");
        if ui
            .add(egui::Slider::new(&mut scale, 0.5..=2.0).fixed_decimals(1))
            .changed()
        {
            settings.scale_factor = scale;
            changes.any = true;
        }
    });

    ui.separator();
    ui.heading("Planning");
    let mut altitude = settings.default_waypoint_altitude_m;
    ui.horizontal(|ui| {
        ui.label(format!("Default waypoint altitude: {altitude:.0} m"));
        if ui
            .add(
                egui::Slider::new(&mut altitude, 5.0..=500.0)
                    .step_by(5.0)
                    .suffix(" m"),
            )
            .changed()
        {
            settings.default_waypoint_altitude_m = altitude;
            changes.any = true;
        }
    });

    ui.separator();
    ui.heading("Recording");
    let mut record = settings.record_telemetry;
    if ui
        .checkbox(&mut record, "Record telemetry logs (.tlog)")
        .changed()
    {
        settings.record_telemetry = record;
        changes.any = true;
        changes.record_telemetry = true;
    }
    ui.label(
        egui::RichText::new(match recording {
            Some(path) => format!("Recording to {}", path.display()),
            None => "Not recording".to_owned(),
        })
        .small()
        .weak(),
    );

    changes
}
