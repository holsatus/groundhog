//! The mission panel: toolbar, transfer progress, the item list and the
//! selected item's editor.

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
    ui::{self, TextExt, Tone},
    widget::virtualized::virtual_list,
};

/// Height of a single mission-list row.
const ROW_HEIGHT: f32 = 32.0;

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

        let vehicle_action = |label: &'static str, kind: MessageKind| {
            ui::button(label)
                .on_press_maybe((plan.is_some() && !transferring).then(|| message(kind)))
                .boxed()
        };
        let edit_action = |label: &'static str, kind: MessageKind| {
            ui::button(label)
                .on_press_maybe(editable.then(|| message(kind)))
                .boxed()
        };

        let transfers = ui::toolbar([
            vehicle_action("Download", MessageKind::Download { confirmed: false }),
            vehicle_action("Upload", MessageKind::Upload { confirmed: false }),
            vehicle_action("Clear vehicle", MessageKind::Clear { confirmed: false }),
            vehicle_action("Start", MessageKind::Start { confirmed: false }),
            vehicle_action("Pause", MessageKind::Pause),
            vehicle_action("Resume", MessageKind::Resume),
        ]);

        let editing = ui::toolbar([
            edit_action("Add waypoint", MessageKind::AddWaypoint),
            ui::button(if draft.placing {
                "Placing… (click map)"
            } else {
                "Place on map"
            })
            .chosen(draft.placing)
            .on_press_maybe(editable.then(|| message(MessageKind::TogglePlacing)))
            .boxed(),
            edit_action("Discard plan", MessageKind::Discard),
            edit_action("Save…", MessageKind::SaveDialog),
            edit_action("Load…", MessageKind::LoadDialog),
        ]);

        let (status_text, tone) = match status {
            SyncStatus::Offline => ("Offline plan", Tone::Muted),
            SyncStatus::Unknown => ("Vehicle mission not downloaded", Tone::Muted),
            SyncStatus::Synced => ("Synced with the vehicle", Tone::Success),
            SyncStatus::Modified => ("Modified (not uploaded)", Tone::Warning),
            SyncStatus::VehicleChanged => (
                "The vehicle's mission was changed elsewhere; download it",
                Tone::Danger,
            ),
        };
        let count = match draft.plan().len() {
            1 => "1 item".to_string(),
            count => format!("{count} items"),
        };

        let header = column![
            transfers,
            editing,
            ui::text(format!("{status_text} · {count}")).tone(tone),
            plan.and_then(|system| self.view_progress(system, message)),
        ]
        .spacing(ui::space::S);

        let current = plan
            .and_then(|system| fleet.vehicle(system))
            .filter(|_| status == SyncStatus::Synced)
            .and_then(|vehicle| vehicle.current_mission_index());

        let list = self.view_list(plan, draft, current);

        // With an item selected, the list and its editor share the height by
        // what each needs, each scrolling on its own when cut short.
        let body = match draft
            .selected
            .and_then(|id| draft.plan().get(id).map(|item| (id, item)))
        {
            Some((id, item)) => ui::split_column(
                list,
                scrollable(view_editor(plan, draft, id, item, status, editable)),
            ),
            None => column![
                container(list).height(Length::Fill),
                ui::caption("Select an item to edit it.").tone(Tone::Muted)
            ]
            .spacing(ui::space::M)
            .boxed(),
        };

        ui::panel(column![header, body].spacing(ui::space::M))
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
                "{}: {}/{}",
                operation.describe(),
                progress.done,
                progress.total
            ),
            None => format!("{}…", operation.describe()),
        };
        let (done, total) = progress.map_or((0.0, 1.0), |progress| {
            (f32::from(progress.done), f32::from(progress.total))
        });

        Some(
            row![
                column![ui::caption(label), ui::progress(done, total)]
                    .spacing(ui::space::XXS)
                    .width(Length::Fill),
                ui::button("Cancel")
                    .danger()
                    .on_press(message(MessageKind::Cancel(operation)))
                    .boxed(),
            ]
            .spacing(ui::space::M)
            .align_y(Alignment::Center)
            .boxed(),
        )
    }

    fn view_list<'a>(
        &'a self,
        plan: PlanKey,
        draft: &'a Draft,
        current: Option<usize>,
    ) -> Element<'a, Message> {
        if draft.plan().is_empty() {
            return ui::caption("The plan is empty.").tone(Tone::Muted).boxed();
        }

        let selected = draft.selected;

        let list = virtual_list(draft.plan().items().iter().enumerate(), move |entry| {
            let &(index, planned) = entry;
            let id = planned.id();
            let item = &planned.item;

            let content = row![
                ui::text(format!("{}", index + 1)).width(Length::Fixed(32.0)),
                ui::text(item.name())
                    .single_line()
                    .width(Length::Fixed(130.0)),
                ui::text(summary(item)).single_line().width(Length::Fill),
                ui::text(if current == Some(index) {
                    "current"
                } else {
                    ""
                })
                .tone(Tone::Success)
                .width(Length::Fixed(72.0)),
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
        .item_height(|_| ROW_HEIGHT)
        .key(|(_, planned)| planned.id());

        // Only as tall as its rows; its parent decides how much of that shows.
        scrollable(list).width(Length::Fill).boxed()
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

    let action = |label: &'static str, kind: Option<MessageKind>| {
        ui::button(label)
            .on_press_maybe(kind.filter(|_| editable).map(message))
            .boxed()
    };

    let actions = ui::toolbar([
        action("Move up", Some(MessageKind::MoveUp(id))),
        action("Move down", Some(MessageKind::MoveDown(id))),
        action("Remove", Some(MessageKind::Remove(id))),
        action(
            "Continue here",
            (plan.is_some() && status == SyncStatus::Synced).then_some(MessageKind::SetCurrent {
                item: id,
                confirmed: false,
            }),
        ),
        action("Deselect", Some(MessageKind::Select(None))),
    ]);

    ui::card(
        column![
            row![ui::title(format!("Item {}", index + 1)), command]
                .spacing(ui::space::M)
                .align_y(Alignment::Center),
            Column::with_children(fields).spacing(ui::space::XS),
            actions,
        ]
        .spacing(ui::space::S),
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
        FieldKind::Bool => iced::widget::checkbox(view.value != 0.0)
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
        _ => view.value.to_string(),
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
