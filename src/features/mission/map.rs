//! The mission's map layers: the route under the vehicles, and draggable item
//! markers above them.

use std::hash::{DefaultHasher, Hash, Hasher};

use iced::widget::canvas::{LineCap, LineDash, Stroke, Style};
use mavio::default_dialect::enums::MavCmd;

use super::{Draft, Message, MessageKind, MissionEditor, PlanKey};
use crate::{
    domain::{
        geo::GeoPosition,
        mission::{ItemId, MissionItem},
    },
    features::MapContext,
    map::{
        Handle, HandleId, Interaction, LayerEvent, LayerId, LayerSet, MapFrame, MapLayer,
        MapPlugin, MapView,
    },
    style,
};

/// Screen-space hit radius of an item marker.
const MARKER_HIT_RADIUS: f32 = 12.0;

impl<'a> MapPlugin<'a, MapContext<'a>> for MissionEditor {
    type Message = Message;

    fn layers(&'a self, context: MapContext<'a>, layers: &mut LayerSet<'a, Message>) {
        let plan = context.selected;
        let Some(draft) = self.displayed(plan) else {
            return;
        };

        let current = plan
            .and_then(|system| context.fleet.vehicle(system))
            .filter(|_| self.status(plan, context.fleet) == super::SyncStatus::Synced)
            .and_then(|vehicle| vehicle.current_mission_index());

        layers.push(Route { draft });
        layers.push(Markers {
            plan,
            draft,
            current,
            editable: plan.is_none_or(|system| !self.transferring(system)),
        });
    }
}

fn handle_of(item: ItemId) -> HandleId {
    HandleId(item.raw().into())
}

fn item_of(handle: HandleId) -> ItemId {
    ItemId::from_raw(handle.0 as u32)
}

/// The loiter radius of an item, in metres, if it loiters.
fn loiter_radius(item: &MissionItem) -> Option<f32> {
    let slot = match item.command {
        MavCmd::NavLoiterUnlim | MavCmd::NavLoiterTurns | MavCmd::NavLoiterTime => 2,
        MavCmd::NavLoiterToAlt => 1,
        _ => return None,
    };
    let radius = item.params[slot].abs();
    (radius.is_finite() && radius > 0.0).then_some(radius)
}

/// The dashed line through the positioned items.
struct Route<'a> {
    draft: &'a Draft,
}

impl MapLayer<Message> for Route<'_> {
    fn id(&self) -> LayerId {
        LayerId::new("mission.route")
    }

    fn z_index(&self) -> i32 {
        10
    }

    fn revision(&self) -> Option<u64> {
        Some(self.draft.plan.revision())
    }

    fn draw(&self, frame: &mut MapFrame<'_>, _interaction: Interaction) {
        let positions = self
            .draft
            .plan
            .route()
            .map(|(_, _, position)| GeoPosition::new(position.lat, position.lon));

        frame.polyline(
            positions,
            Stroke {
                style: Style::Solid(style::map_route()),
                width: 2.0,
                line_cap: LineCap::Butt,
                line_dash: LineDash {
                    segments: &[15.0, 15.0],
                    offset: 0,
                },
                ..Stroke::default()
            },
        );
    }
}

/// A numbered marker per positioned item. Dragging one moves the item;
/// clicking one selects it; clicking the map while placing adds a waypoint.
struct Markers<'a> {
    plan: PlanKey,
    draft: &'a Draft,
    /// The plan index the vehicle executes, when the plan is synced.
    current: Option<usize>,
    editable: bool,
}

impl Markers<'_> {
    fn message(&self, kind: MessageKind) -> Message {
        Message {
            plan: self.plan,
            kind,
        }
    }
}

impl MapLayer<Message> for Markers<'_> {
    fn id(&self) -> LayerId {
        LayerId::new("mission.markers")
    }

    fn z_index(&self) -> i32 {
        30
    }

    fn revision(&self) -> Option<u64> {
        let mut hasher = DefaultHasher::new();
        (
            self.draft.plan.revision(),
            self.draft.selected,
            self.current,
        )
            .hash(&mut hasher);
        Some(hasher.finish())
    }

    fn draw(&self, frame: &mut MapFrame<'_>, interaction: Interaction) {
        // Loiter circles, to scale: they show the airspace the vehicle uses.
        for planned in self.draft.plan.items() {
            if let (Some(position), Some(radius_m)) =
                (planned.item.position(), loiter_radius(&planned.item))
            {
                let radius = radius_m / frame.view().metres_per_pixel(position.lat) as f32;
                if radius > 3.0 {
                    frame.ring(
                        GeoPosition::new(position.lat, position.lon),
                        radius,
                        Stroke {
                            style: Style::Solid(style::map_route().scale_alpha(0.6)),
                            width: 1.5,
                            ..Stroke::default()
                        },
                    );
                }
            }
        }

        for (index, id, position) in self.draft.plan.route() {
            let at = GeoPosition::new(position.lat, position.lon);
            let handle = handle_of(id);

            let (radius, color) = if interaction.dragged == Some(handle) {
                (12.0, style::marker_dragged())
            } else if self.current == Some(index) {
                (10.0, style::marker_current())
            } else if self.draft.selected == Some(id) || interaction.hovered == Some(handle) {
                (10.0, style::marker_selected())
            } else {
                (8.0, style::marker_default())
            };

            frame.circle(at, radius, color);
            frame.label(at, (index + 1).to_string(), 12.0, style::marker_label());
        }
    }

    fn handles(&self, _view: &MapView) -> Vec<Handle> {
        self.draft
            .plan
            .route()
            .map(|(_, id, position)| {
                let handle = Handle::at(
                    handle_of(id),
                    GeoPosition::new(position.lat, position.lon),
                    MARKER_HIT_RADIUS,
                );
                if self.editable {
                    handle.draggable()
                } else {
                    handle
                }
            })
            .collect()
    }

    fn on_event(&self, event: &LayerEvent) -> Option<Message> {
        let kind = match *event {
            LayerEvent::Click {
                handle: Some(handle),
                ..
            } => MessageKind::Select(Some(item_of(handle))),
            LayerEvent::Click { handle: None, at } if self.draft.placing && self.editable => {
                MessageKind::PlaceAt(at)
            }
            LayerEvent::Click { handle: None, .. } => return None,
            LayerEvent::Drag { handle, at } => MessageKind::Drag(item_of(handle), at),
            LayerEvent::DragEnd { handle, at } => MessageKind::Drop(item_of(handle), at),
            LayerEvent::DragCancel { handle, origin } => MessageKind::Drop(item_of(handle), origin),
        };

        Some(self.message(kind))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::mission::{MissionPlan, Position};

    fn draft(placing: bool) -> Draft {
        let mut plan = MissionPlan::default();
        plan.push(MissionItem::waypoint(Position {
            lat: 47.0,
            lon: 8.0,
            alt_m: 50.0,
        }));
        Draft {
            plan,
            placing,
            ..Draft::default()
        }
    }

    fn markers(draft: &Draft, editable: bool) -> Markers<'_> {
        Markers {
            plan: None,
            draft,
            current: None,
            editable,
        }
    }

    fn kind(message: Option<Message>) -> Option<MessageKind> {
        message.map(|message| message.kind)
    }

    #[test]
    fn gestures_become_mission_edits() {
        let draft = draft(false);
        let layer = markers(&draft, true);
        let item = draft.plan.items()[0].id();
        let handle = handle_of(item);
        let at = GeoPosition::new(1.0, 2.0);

        assert!(matches!(
            kind(layer.on_event(&LayerEvent::Click { handle: Some(handle), at })),
            Some(MessageKind::Select(Some(selected))) if selected == item
        ));
        assert!(matches!(
            kind(layer.on_event(&LayerEvent::Drag { handle, at })),
            Some(MessageKind::Drag(dragged, _)) if dragged == item
        ));
        assert!(matches!(
            kind(layer.on_event(&LayerEvent::DragCancel { handle, origin: at })),
            Some(MessageKind::Drop(dropped, origin)) if dropped == item && origin == at
        ));
        assert!(
            layer
                .on_event(&LayerEvent::Click { handle: None, at })
                .is_none(),
            "empty clicks fall through when not placing"
        );
    }

    #[test]
    fn empty_clicks_place_waypoints_while_placing() {
        let draft = draft(true);
        let at = GeoPosition::new(1.0, 2.0);

        let editable = markers(&draft, true);
        assert!(matches!(
            kind(editable.on_event(&LayerEvent::Click { handle: None, at })),
            Some(MessageKind::PlaceAt(placed)) if placed == at
        ));

        let locked = markers(&draft, false);
        assert!(
            locked
                .on_event(&LayerEvent::Click { handle: None, at })
                .is_none()
        );
        assert!(
            locked
                .handles(&crate::map::MapView::for_tests())
                .iter()
                .all(|handle| !handle.draggable),
            "nothing drags during a transfer"
        );
    }

    #[test]
    fn only_loiters_have_a_radius() {
        let at = Some(Position {
            lat: 1.0,
            lon: 1.0,
            alt_m: 10.0,
        });
        let mut loiter = MissionItem::new(MavCmd::NavLoiterUnlim, at);
        loiter.params[2] = -60.0; // counter-clockwise
        assert_eq!(loiter_radius(&loiter), Some(60.0));
        assert_eq!(
            loiter_radius(&MissionItem::new(MavCmd::NavWaypoint, at)),
            None
        );
    }
}
