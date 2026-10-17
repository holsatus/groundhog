//! The mission panel: toolbar, transfer progress, the item list and the
//! selected item's editor.

use std::fmt;

use iced::{
    Alignment, Element, Length, Theme, Widget,
    alignment::Vertical,
    widget::{
        Button, Column, Space, button, checkbox, column, container, pick_list, progress_bar, row,
        scrollable, text, text::Ellipsis, text::Wrapping, text_input,
    },
};
use mavio::default_dialect::enums::MavCmd;

use super::{Draft, Message, MessageKind, MissionEditor, Operation, PlanKey, SyncStatus};
use crate::{
    domain::{
        SystemId,
        fleet::FleetSnapshot,
        mission::{
            Field, FieldKind, FieldView, ItemId, MissionItem,
            command::{SPECS, spec_of},
        },
    },
    style,
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
        theme: &Theme,
    ) -> Element<'a, Message> {
        let plan: PlanKey = selected.filter(|system| fleet.contains(*system));
        let Some(draft) = self.displayed(plan) else {
            return Space::new().boxed();
        };

        let message = move |kind| Message { plan, kind };
        let status = self.status(plan, fleet);
        let transferring = plan.is_some_and(|system| self.transferring(system));
        let editable = !transferring;

        let vehicle_action = |label: &'static str, kind: Option<MessageKind>| {
            Button::new(text(label))
                .style(style::button::transparent)
                .on_press_maybe(
                    kind.filter(|_| plan.is_some() && !transferring)
                        .map(message),
                )
        };

        let toolbar = row![
            vehicle_action("Download", Some(MessageKind::Download { confirmed: false })),
            vehicle_action("Upload", Some(MessageKind::Upload { confirmed: false })),
            vehicle_action(
                "Clear vehicle",
                Some(MessageKind::Clear { confirmed: false })
            ),
            vehicle_action("Start", Some(MessageKind::Start { confirmed: false })),
            vehicle_action("Pause", Some(MessageKind::Pause)),
            vehicle_action("Resume", Some(MessageKind::Resume)),
        ]
        .spacing(6.0)
        .wrap();

        let edit_action = |label: &'static str, kind: MessageKind| {
            Button::new(text(label))
                .style(style::button::transparent)
                .on_press_maybe(editable.then(|| message(kind)))
        };

        let placing = draft.placing;
        let editing = row![
            edit_action("Add waypoint", MessageKind::AddWaypoint),
            Button::new(text(if placing {
                "Placing… (click map)"
            } else {
                "Place on map"
            }))
            .style(if placing {
                button::primary
            } else {
                style::button::transparent
            })
            .on_press_maybe(editable.then(|| message(MessageKind::TogglePlacing))),
            edit_action("Discard plan", MessageKind::Discard),
            edit_action("Save…", MessageKind::SaveDialog),
            edit_action("Load…", MessageKind::LoadDialog),
        ]
        .spacing(6.0)
        .wrap();

        let palette = theme.palette();
        let (status_text, status_color) = match status {
            SyncStatus::Offline => ("Offline plan".to_string(), palette.secondary.base.color),
            SyncStatus::Unknown => (
                "Vehicle mission not downloaded".to_string(),
                palette.secondary.base.color,
            ),
            SyncStatus::Synced => (
                "Synced with the vehicle".to_string(),
                palette.success.base.color,
            ),
            SyncStatus::Modified => (
                "Modified (not uploaded)".to_string(),
                palette.warning.base.color,
            ),
            SyncStatus::VehicleChanged => (
                "The vehicle's mission was changed elsewhere; download it".to_string(),
                palette.danger.base.color,
            ),
        };

        let count = match draft.plan.len() {
            1 => "1 item".to_string(),
            count => format!("{count} items"),
        };
        let mut header = column![
            toolbar,
            editing,
            text(format!("{status_text} · {count}")).color(status_color)
        ]
        .spacing(6.0);

        if let Some(system) = plan
            && let Some(progress) = self.view_progress(system, message)
        {
            header = header.push(progress);
        }

        let current = plan
            .and_then(|system| fleet.vehicle(system))
            .filter(|_| status == SyncStatus::Synced)
            .and_then(|vehicle| vehicle.current_mission_index());

        let list = self.view_list(plan, draft, current, theme);

        // With an item selected, the list and its editor share the height,
        // each scrolling on its own.
        let body: Element<'_, Message> = match draft
            .selected
            .and_then(|id| draft.plan.get(id).map(|item| (id, item)))
        {
            Some((id, item)) => column![
                container(list).height(Length::FillPortion(1)),
                scrollable(view_editor(plan, draft, id, item, status, editable))
                    .height(Length::FillPortion(1)),
            ]
            .spacing(8.0)
            .boxed(),
            None => column![list, text("Select an item to edit it.").size(12)]
                .spacing(8.0)
                .boxed(),
        };

        container(column![header, body].spacing(8.0))
            .padding(8.0)
            .width(Length::Fill)
            .height(Length::Fill)
            .boxed()
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
        let (done, total) = progress.map_or((0.0, 1.0), |progress| {
            (f32::from(progress.done), f32::from(progress.total.max(1)))
        });

        let label = match progress {
            Some(progress) => format!(
                "{}: {}/{}",
                operation.describe(),
                progress.done,
                progress.total
            ),
            None => format!("{}…", operation.describe()),
        };

        Some(
            row![
                column![
                    text(label).size(12),
                    progress_bar(0.0..=total, done).girth(6.0)
                ]
                .spacing(2)
                .width(Length::Fill),
                button(text("Cancel"))
                    .style(button::danger)
                    .on_press(message(MessageKind::Cancel(operation))),
            ]
            .spacing(8.0)
            .align_y(Alignment::Center)
            .boxed(),
        )
    }

    fn view_list<'a>(
        &'a self,
        plan: PlanKey,
        draft: &'a Draft,
        current: Option<usize>,
        theme: &Theme,
    ) -> Element<'a, Message> {
        if draft.plan.is_empty() {
            return container(text("The plan is empty.").size(12))
                .height(Length::Fill)
                .boxed();
        }

        let current_color = theme.palette().success.base.color;
        let selected = draft.selected;

        let list = virtual_list(draft.plan.items().iter().enumerate(), move |entry| {
            let &(index, planned) = entry;
            let id = planned.id();
            let item = &planned.item;

            let content = row![
                text(format!("{}", index + 1)).width(Length::Fixed(32.0)),
                text(item.name())
                    .wrapping(Wrapping::None)
                    .ellipsis(Ellipsis::End)
                    .width(Length::Fixed(130.0)),
                text(summary(item))
                    .wrapping(Wrapping::None)
                    .ellipsis(Ellipsis::End)
                    .width(Length::Fill),
                text(if current == Some(index) {
                    "▶ current"
                } else {
                    ""
                })
                .color(current_color)
                .width(Length::Fixed(72.0)),
            ]
            .spacing(8.0)
            .align_y(Vertical::Center);

            let style: fn(&Theme, button::Status) -> button::Style = if selected == Some(id) {
                button::primary
            } else {
                style::button::transparent
            };

            button(content)
                .style(style)
                .padding(4)
                .width(Length::Fill)
                .on_press(Message {
                    plan,
                    kind: MessageKind::Select(Some(id)),
                })
                .boxed()
        })
        .item_height(|_| ROW_HEIGHT)
        .key(|(_, planned)| planned.id())
        .spacing(0.0);

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
    let index = draft.plan.index_of(id).unwrap_or_default();

    let mut choices: Vec<CommandChoice> = SPECS
        .iter()
        .map(|spec| CommandChoice(spec.command))
        .collect();
    if item.spec().is_none() {
        choices.insert(0, CommandChoice(item.command));
    }

    let command = pick_list(
        Some(CommandChoice(item.command)),
        choices,
        CommandChoice::to_string,
    )
    .on_select(move |choice: CommandChoice| message(MessageKind::ChangeCommand(id, choice.0)))
    .style(style::pick_list::default)
    .width(Length::Fill);

    let mut fields = Column::<Element<'a, Message>>::new().spacing(4.0);
    for view in item.fields() {
        fields = fields.push(
            row![
                text(view.label).width(Length::Fixed(150.0)),
                field_widget(view, draft.edits.get(&view.field), id, plan),
                text(view.units).width(Length::Fixed(40.0)),
            ]
            .spacing(6.0)
            .align_y(Vertical::Center)
            .boxed(),
        );
    }

    let action = |label: &'static str, kind: Option<MessageKind>| {
        Button::new(text(label))
            .style(style::button::transparent)
            .on_press_maybe(kind.filter(|_| editable).map(message))
    };

    let actions = row![
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
    ]
    .spacing(6.0)
    .wrap();

    container(
        column![
            row![text(format!("Item {}", index + 1)).size(16), command]
                .spacing(8.0)
                .align_y(Alignment::Center),
            fields,
            actions,
        ]
        .spacing(6.0),
    )
    .padding(8.0)
    .style(style::inner_container)
    .boxed()
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
        FieldKind::Bool => checkbox(view.value != 0.0)
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
                Some(selected) => pick_list(Some(selected), options, Choice::to_string)
                    .on_select(move |choice: Choice| edit(choice.0.to_string()))
                    .style(style::pick_list::default)
                    .width(Length::Fill)
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

    text_input(placeholder, value)
        .on_input(edit)
        .style(style::text_input::default)
        .width(Length::Fill)
        .boxed()
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
