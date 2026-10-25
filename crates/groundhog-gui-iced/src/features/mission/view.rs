//! The mission panel: how the plan stands against the vehicle, the
//! vehicle's actions, transfer progress, the item list and the selected
//! item's editor.

use std::fmt;

use groundhog_logic::{
    SystemId,
    domain::{
        fleet::FleetSnapshot,
        mission::{
            Field, FieldKind, FieldView, ItemId, MissionItem,
            command::{SPECS, spec_of},
        },
    },
};
use iced::{
    Alignment, Element, Length, Widget,
    widget::{Column, Space, column, container, row, scrollable},
};
use mavio::default_dialect::enums::MavCmd;

use super::{Draft, Message, MessageKind, MissionEditor, Operation, PlanKey, SyncStatus};
use crate::{
    ui::{self, Icon, TextExt, Tone, style::ButtonKind},
    widget::virtualized::virtual_list,
};

/// A command in the command picker.
#[derive(Debug, Clone, Copy, PartialEq)]
struct CommandChoice(MavCmd);

impl fmt::Display for CommandChoice {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match spec_of(self.0) {
            Some(spec) => f.write_str(spec.name),
            None => write!(f, "{:?}", self.0),
        }
    }
}

/// One option of a choice field.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Choice(f32, &'static str);

impl fmt::Display for Choice {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.1)
    }
}

impl MissionEditor {
    /// The panel, in the order a mission is worked: how the plan stands
    /// against the vehicle (and its files), the vehicle's and the plan's
    /// actions, a transfer under way, then the items and the one selected.
    pub fn view<'a>(
        &'a self,
        fleet: &'a FleetSnapshot,
        selected: Option<SystemId>,
    ) -> Element<'a, Message> {
        let plan: PlanKey = selected.filter(|system| fleet.contains(*system));
        let Some(draft) = self.displayed(plan) else {
            return Space::new().boxed();
        };

        let message = move |kind| Message { plan, kind };
        let status = self.status(plan, fleet);
        let transferring = plan.is_some_and(|system| self.transferring(system));
        let editable = !transferring;
        let on_vehicle = |kind: MessageKind| (!transferring).then(|| message(kind));
        let on_plan = |kind: MessageKind| editable.then(|| message(kind));

        let (state, tone) = match status {
            SyncStatus::Offline => ("Offline plan", Tone::Muted),
            SyncStatus::Unknown => ("Not downloaded", Tone::Muted),
            SyncStatus::Synced => ("Synced", Tone::Success),
            SyncStatus::Modified => ("Not uploaded", Tone::Warning),
            SyncStatus::VehicleChanged => ("Changed on the vehicle", Tone::Danger),
        };
        let count = match draft.plan().len() {
            1 => "1 item".to_string(),
            count => format!("{count} items"),
        };
        let header = ui::header(
            row![ui::badge(state, tone), ui::caption(count).tone(Tone::Muted)]
                .spacing(ui::space::S)
                .align_y(Alignment::Center),
            [],
            [
                vec![
                    ui::tool(Icon::Add, "Add a waypoint after the last")
                        .on_press_maybe(on_plan(MessageKind::AddWaypoint)),
                    ui::tool(
                        Icon::Place,
                        if draft.placing {
                            "Stop placing waypoints on the map"
                        } else {
                            "Place waypoints on the map"
                        },
                    )
                    .chosen(draft.placing)
                    .on_press_maybe(on_plan(MessageKind::TogglePlacing)),
                ],
                vec![
                    ui::tool(Icon::Open, "Load a plan from a file")
                        .on_press_maybe(on_plan(MessageKind::LoadDialog)),
                    ui::tool(Icon::Save, "Save the plan to a file")
                        .on_press_maybe(on_plan(MessageKind::SaveDialog)),
                    ui::tool(Icon::Delete, "Discard the plan").on_press_maybe(
                        on_plan(MessageKind::Discard).filter(|_| !draft.plan().is_empty()),
                    ),
                ],
            ],
        );

        // The vehicle's actions only exist with a vehicle; the offline plan
        // is edited and saved, nothing else.
        let vehicle = plan.map(|_| {
            vec![
                ui::button("Download")
                    .icon(Icon::Download)
                    .hint("Replace the plan with the vehicle's mission")
                    .on_press_maybe(on_vehicle(MessageKind::Download { confirmed: false }))
                    .boxed(),
                ui::button("Upload")
                    .icon(Icon::Upload)
                    .kind(if status == SyncStatus::Modified {
                        ButtonKind::Primary
                    } else {
                        ButtonKind::Subtle
                    })
                    .hint("Replace the vehicle's mission with the plan")
                    .on_press_maybe(on_vehicle(MessageKind::Upload { confirmed: false }))
                    .boxed(),
                ui::icon_button(Icon::Erase)
                    .hint("Clear the vehicle's mission")
                    .on_press_maybe(on_vehicle(MessageKind::Clear { confirmed: false }))
                    .boxed(),
            ]
        });
        let flight = plan.map(|_| {
            vec![
                ui::button("Start")
                    .icon(Icon::Play)
                    .hint("Fly the mission from its first item")
                    .on_press_maybe(on_vehicle(MessageKind::Start { confirmed: false }))
                    .boxed(),
                ui::icon_button(Icon::Pause)
                    .hint("Pause the mission")
                    .on_press_maybe(on_vehicle(MessageKind::Pause))
                    .boxed(),
                ui::button("Resume")
                    .hint("Resume the paused mission")
                    .on_press_maybe(on_vehicle(MessageKind::Resume { confirmed: false }))
                    .boxed(),
            ]
        });
        let actions = plan.map(|_| ui::actions([vehicle, flight].into_iter().flatten()));

        let changed_elsewhere = (status == SyncStatus::VehicleChanged).then(|| {
            ui::banner(
                "The vehicle's mission was changed elsewhere. Download it before uploading.",
                Tone::Danger,
            )
        });

        let current = plan
            .and_then(|system| fleet.vehicle(system))
            .filter(|_| status == SyncStatus::Synced)
            .and_then(|vehicle| vehicle.current_mission_index());

        let list = self.view_list(plan, draft, current);

        // With an item selected, the list and its editor share the height,
        // each scrolling on its own.
        let body = match draft
            .selected
            .and_then(|id| draft.plan().get(id).map(|item| (id, item)))
        {
            Some((id, item)) => column![
                container(list).height(Length::FillPortion(1)),
                scrollable(view_editor(plan, draft, id, item, status, editable))
                    .height(Length::FillPortion(1)),
            ],
            None => column![
                list,
                (!draft.plan().is_empty())
                    .then(|| ui::caption("Select an item to edit it.").tone(Tone::Muted)),
            ],
        };

        ui::panel_with(
            header,
            column![
                actions,
                changed_elsewhere,
                plan.and_then(|system| self.view_progress(system, message)),
                body.spacing(ui::space::M),
            ]
            .spacing(ui::space::M),
        )
    }

    /// The running transfer's progress and a cancel button.
    fn view_progress<'a>(
        &'a self,
        system: SystemId,
        message: impl Fn(MessageKind) -> Message + 'a,
    ) -> Option<Element<'a, Message>> {
        let operation = [Operation::Download, Operation::Upload, Operation::Clear]
            .into_iter()
            .find(|operation| self.operations.is_running(&(system, *operation)))?;

        let progress = self.operations.progress(&(system, operation));
        let label = match progress {
            Some(progress) => format!(
                "{}: {} of {}",
                operation.describe(),
                progress.done,
                progress.total
            ),
            None => format!("{}…", operation.describe()),
        };

        Some(ui::activity(
            label,
            progress.map(|progress| (f32::from(progress.done), f32::from(progress.total))),
            Some(message(MessageKind::Cancel(operation))),
        ))
    }

    fn view_list<'a>(
        &'a self,
        plan: PlanKey,
        draft: &'a Draft,
        current: Option<usize>,
    ) -> Element<'a, Message> {
        if draft.plan().is_empty() {
            return container(
                ui::text("No items yet. Add a waypoint, or place them on the map.")
                    .tone(Tone::Muted),
            )
            .center(Length::Fill)
            .boxed();
        }

        let selected = draft.selected;
        // A row is a button around one line, with a little room around it.
        let row_height = ui::size::control() + 2.0 * ui::space::XXS;

        let list = virtual_list(draft.plan().items().iter().enumerate(), move |entry| {
            let &(index, planned) = entry;
            let id = planned.id();
            let item = &planned.item;

            let content = row![
                ui::text(format!("{}", index + 1))
                    .tone(Tone::Muted)
                    .width(Length::Fixed(28.0)),
                ui::text(item.name())
                    .single_line()
                    .width(Length::Fixed(120.0)),
                ui::caption(summary(item)).single_line().width(Length::Fill),
                (current == Some(index)).then(|| ui::badge("Current", Tone::Success)),
            ]
            .spacing(ui::space::M)
            .align_y(Alignment::Center);

            ui::button_with(content)
                .chosen(selected == Some(id))
                .width(Length::Fill)
                .on_press(Message {
                    plan,
                    kind: MessageKind::Select(Some(id)),
                })
                .boxed()
        })
        .item_height(move |_| row_height)
        .key(|(_, planned)| planned.id());

        scrollable(list)
            .height(Length::Fill)
            .width(Length::Fill)
            .boxed()
    }
}

fn view_editor<'a>(
    plan: PlanKey,
    draft: &'a Draft,
    id: ItemId,
    item: &'a MissionItem,
    status: SyncStatus,
    editable: bool,
) -> Element<'a, Message> {
    let message = move |kind| Message { plan, kind };
    let index = draft.plan().index_of(id).unwrap_or_default();

    let mut choices: Vec<CommandChoice> = SPECS
        .iter()
        .map(|spec| CommandChoice(spec.command))
        .collect();
    if item.spec().is_none() {
        choices.insert(0, CommandChoice(item.command));
    }

    let command = ui::pick_list(
        Some(CommandChoice(item.command)),
        choices,
        CommandChoice::to_string,
    )
    .on_select(move |choice: CommandChoice| message(MessageKind::ChangeCommand(id, choice.0)));

    let fields = item.fields().into_iter().map(|view| {
        ui::field(
            view.label,
            row![
                field_widget(view, draft.edits.get(&view.field), id, plan),
                ui::caption(view.units).width(Length::Fixed(32.0)),
            ]
            .spacing(ui::space::S)
            .align_y(Alignment::Center),
        )
    });

    let on_edit = |kind: MessageKind| editable.then(|| message(kind));
    let first = index == 0;
    let last = index + 1 == draft.plan().len();

    // The item's actions head the card, so they stay in view however far
    // its fields scroll; where they would squeeze the command, they fold
    // into a menu.
    let header = ui::header(
        row![ui::title(format!("Item {}", index + 1)), command]
            .spacing(ui::space::S)
            .align_y(Alignment::Center),
        [],
        [
            vec![
                ui::tool(Icon::MoveUp, "Move up")
                    .on_press_maybe(on_edit(MessageKind::MoveUp(id)).filter(|_| !first)),
                ui::tool(Icon::MoveDown, "Move down")
                    .on_press_maybe(on_edit(MessageKind::MoveDown(id)).filter(|_| !last)),
                ui::tool(
                    Icon::Skip,
                    if plan.is_some() && status == SyncStatus::Synced {
                        "Continue the mission from here"
                    } else {
                        "Continue from here (once synced)"
                    },
                )
                .on_press_maybe(
                    on_edit(MessageKind::SetCurrent {
                        item: id,
                        confirmed: false,
                    })
                    .filter(|_| plan.is_some() && status == SyncStatus::Synced),
                ),
                ui::tool(Icon::Delete, "Remove the item")
                    .on_press_maybe(on_edit(MessageKind::Remove(id))),
            ],
            vec![ui::tool(Icon::Close, "Deselect").on_press(message(MessageKind::Select(None)))],
        ],
    );

    ui::card(
        column![header, Column::with_children(fields).spacing(ui::space::S)].spacing(ui::space::M),
    )
}

/// The widget for one field: text for numbers, a checkbox for flags, a
/// picker for choices.
fn field_widget<'a>(
    view: FieldView,
    buffer: Option<&'a String>,
    id: ItemId,
    plan: PlanKey,
) -> Element<'a, Message> {
    let edit = move |text: String| Message {
        plan,
        kind: MessageKind::EditField(id, view.field, text),
    };

    match view.kind {
        FieldKind::Bool => ui::unlabelled_checkbox(view.value != 0.0)
            .on_toggle(move |on| edit(if on { "1" } else { "0" }.to_string()))
            .boxed(),
        FieldKind::Choice(options) => {
            let options: Vec<Choice> = options
                .iter()
                .map(|(value, label)| Choice(*value, label))
                .collect();
            let selected = options
                .iter()
                .copied()
                .find(|choice| f64::from(choice.0) == view.value);

            match selected {
                Some(selected) => ui::pick_list(Some(selected), options, Choice::to_string)
                    .on_select(move |choice: Choice| edit(choice.0.to_string()))
                    .boxed(),
                // A value outside the known choices (from another planner):
                // edit it as a number rather than hide it.
                None => number_input(view, buffer, edit),
            }
        }
        FieldKind::Number
        | FieldKind::Integer
        | FieldKind::OptionalNumber
        | FieldKind::ItemNumber => number_input(view, buffer, edit),
    }
}

fn number_input<'a>(
    view: FieldView,
    buffer: Option<&'a String>,
    edit: impl Fn(String) -> Message + 'a,
) -> Element<'a, Message> {
    let value = match buffer {
        Some(buffer) => buffer.clone(),
        None => format_value(view),
    };

    let placeholder = if view.kind == FieldKind::OptionalNumber {
        "vehicle default"
    } else {
        ""
    };

    ui::text_input(placeholder, value).on_input(edit).boxed()
}

fn format_value(view: FieldView) -> String {
    if view.value.is_nan() {
        return String::new();
    }

    match (view.field, view.kind) {
        (Field::Latitude | Field::Longitude, _) => format!("{:.7}", view.value),
        (_, FieldKind::Integer | FieldKind::ItemNumber) => format!("{}", view.value.round()),
        // Every other field is an `f32` on the wire, widened: narrowing it
        // back is exact, and prints the shortest text that reads back as the
        // same value (12.3, not 12.300000190734863).
        _ => (view.value as f32).to_string(),
    }
}

/// A one-line summary of an item for the list.
fn summary(item: &MissionItem) -> String {
    if let Some(position) = item.position() {
        return format!(
            "{:.6}, {:.6}, {:.1} m",
            position.lat, position.lon, position.alt_m
        );
    }

    item.fields()
        .iter()
        .filter(|view| !view.value.is_nan())
        .take(3)
        .map(|view| format!("{} {}", view.label, format_value(*view)))
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use groundhog_logic::domain::mission::{MissionItem, Position};

    use super::*;

    #[test]
    fn values_print_without_widening_noise() {
        let waypoint = MissionItem::waypoint(Position {
            lat: 47.0,
            lon: 8.0,
            alt_m: 12.3,
        });
        let altitude = waypoint
            .fields()
            .into_iter()
            .find(|view| view.field == Field::Altitude)
            .expect("a waypoint has an altitude");

        assert_eq!(format_value(altitude), "12.3");
    }
}
