//! Translation between a plan and its wire form for a firmware's layout.
//!
//! Firmwares that reserve sequence 0 for home ([`MissionLayout::home_slot`])
//! shift every plan item up by one on the wire, and with it every reference to
//! an item (jump targets). The plan itself never contains the home item; it is
//! carried separately so the editor's item numbers match what the operator
//! planned, whatever the firmware.

use mavio::default_dialect::enums::MavMissionType;

use super::item::item_ref;
use super::{MissionItem, OnboardMission, PlanTooLong, Position};
use crate::domain::firmware::MissionLayout;
use crate::wire::mission_item::RawMissionItem;

/// Encode `items` for upload under `layout`. An empty plan encodes to nothing:
/// a lone home row is not sent, because a firmware that reserves the home slot
/// appends it to an empty mission as a waypoint. The zero home placeholder is
/// therefore sent only beside items, where it is harmless: ArduPilot, the
/// firmware with a home slot, ignores what is written there and reads it back
/// as its own home (`AP_Mission::replace_cmd` and `read_cmd_from_storage`).
///
/// # Errors
///
/// [`PlanTooLong`] when `MISSION_COUNT.count` cannot number `items`.
pub(crate) fn encode(
    items: &[MissionItem],
    home: Option<&MissionItem>,
    layout: MissionLayout,
    mission_type: MavMissionType,
) -> Result<Vec<RawMissionItem>, PlanTooLong> {
    if items.is_empty() {
        return Ok(Vec::new());
    }

    if items.len() > layout.max_items() {
        return Err(PlanTooLong);
    }

    let mut wire = Vec::with_capacity(items.len() + usize::from(layout.home_slot));

    if let Some(home) = home_row(home, layout, mission_type) {
        wire.push(home);
    }

    for (index, item) in items.iter().enumerate() {
        let mut item = item.clone();
        shift_targets(&mut item, i64::from(layout.offset()));
        let seq = layout.wire_seq(index).ok_or(PlanTooLong)?;
        wire.push(item.to_wire(seq, mission_type));
    }

    Ok(wire)
}

/// The home slot's wire row for `layout`, or `None` when the layout has no
/// home slot. Without a known `home`, the row holds a zero position.
pub(crate) fn home_row(
    home: Option<&MissionItem>,
    layout: MissionLayout,
    mission_type: MavMissionType,
) -> Option<RawMissionItem> {
    layout.home_slot.then(|| {
        home.cloned()
            .unwrap_or_else(|| {
                MissionItem::home(Position {
                    lat: 0.0,
                    lon: 0.0,
                    alt_m: 0.0,
                })
            })
            .to_wire(0, mission_type)
    })
}

/// Decode a downloaded mission under `layout`. Items are ordered by their
/// sequence number; the home slot (if the layout has one) becomes
/// [`OnboardMission::home`].
pub(crate) fn decode(
    wire: &[RawMissionItem],
    opaque_id: u32,
    layout: MissionLayout,
) -> OnboardMission {
    let mut wire: Vec<&RawMissionItem> = wire.iter().collect();
    wire.sort_by_key(|item| item.seq);

    let mut home = None;
    let mut items = Vec::with_capacity(wire.len());

    for message in wire {
        let mut item = MissionItem::from_wire(message);

        if layout.plan_index(message.seq).is_none() {
            home = Some(item);
            continue;
        }

        shift_targets(&mut item, -i64::from(layout.offset()));
        items.push(item);
    }

    OnboardMission {
        items,
        home,
        opaque_id,
    }
}

/// Move every item reference of `item` by `by`.
///
/// In a plan, `-1` means the home slot: a home-slot layout decodes wire
/// sequence `0` to it and encodes it back to `0`. It is opaque to the
/// item-reference rule, so both directions are spelled out here. Every other
/// target the rule cannot read is left as it is, as is a result that would
/// leave the wire's `u16` range: an invalid target must not silently become
/// another item.
fn shift_targets(item: &mut MissionItem, by: i64) {
    let slots: Vec<usize> = item.item_index_slots().collect();
    for slot in slots {
        let target = item.params[slot];

        if target == -1.0 {
            if by > 0 {
                item.params[slot] = 0.0;
            }
            continue;
        }

        let Some(target) = item_ref(target) else {
            continue;
        };
        let shifted = i64::from(target) + by;
        if shifted == -1 && by < 0 {
            item.params[slot] = -1.0;
        } else if let Ok(shifted) = u16::try_from(shifted) {
            item.params[slot] = f32::from(shifted);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::mission::Position;
    use mavio::default_dialect::enums::MavCmd;

    const ARDUPILOT: MissionLayout = MissionLayout { home_slot: true };
    const PX4: MissionLayout = MissionLayout { home_slot: false };

    fn plan() -> Vec<MissionItem> {
        let waypoint = MissionItem::waypoint(Position {
            lat: 47.0,
            lon: 8.0,
            alt_m: 40.0,
        });
        let mut jump = MissionItem::new(MavCmd::DoJump, None);
        jump.params[0] = 0.0; // back to the first plan item
        vec![waypoint, jump]
    }

    fn encoded(
        items: &[MissionItem],
        home: Option<&MissionItem>,
        layout: MissionLayout,
    ) -> Vec<RawMissionItem> {
        encode(items, home, layout, MavMissionType::Mission).expect("the plan fits on the wire")
    }

    #[test]
    fn the_home_slot_shifts_items_and_jump_targets() {
        let home = MissionItem::home(Position {
            lat: 47.1,
            lon: 8.1,
            alt_m: 400.0,
        });
        let wire = encoded(&plan(), Some(&home), ARDUPILOT);

        assert_eq!(wire.len(), 3);
        assert_eq!(wire[0].seq, 0);
        assert_eq!(MissionItem::from_wire(&wire[0]), home);
        assert_eq!(wire[1].seq, 1);
        assert_eq!(wire[2].command, MavCmd::DoJump as u16);
        assert_eq!(wire[2].params[0], 1.0, "the jump targets wire seq 1");

        let onboard = decode(&wire, 9, ARDUPILOT);
        assert_eq!(
            onboard.items,
            plan(),
            "the round trip restores plan indices"
        );
        assert_eq!(onboard.home, Some(home));
        assert_eq!(onboard.opaque_id, 9);
    }

    #[test]
    fn without_a_home_slot_the_plan_travels_as_is() {
        let wire = encoded(&plan(), None, PX4);
        assert_eq!(wire.len(), 2);
        assert_eq!(wire[1].params[0], 0.0);

        let onboard = decode(&wire, 0, PX4);
        assert_eq!(onboard.items, plan());
        assert_eq!(onboard.home, None);
    }

    #[test]
    fn decoding_orders_by_sequence() {
        let mut wire = encoded(&plan(), None, PX4);
        wire.reverse();
        assert_eq!(decode(&wire, 0, PX4).items, plan());
    }

    /// Wire 0 is the home slot, not the first plan item. A plan says so with
    /// `-1`, which encodes back to 0; nothing shifts it on the way.
    #[test]
    fn a_jump_to_the_home_slot_stays_a_jump_to_home() {
        let home = MissionItem::home(Position {
            lat: 47.0,
            lon: 8.0,
            alt_m: 400.0,
        });
        let mut jump = MissionItem::new(MavCmd::DoJump, None);
        jump.params[0] = 0.0;

        let wire = vec![
            home.to_wire(0, MavMissionType::Mission),
            plan()[0].to_wire(1, MavMissionType::Mission),
            jump.to_wire(2, MavMissionType::Mission),
        ];

        let onboard = decode(&wire, 0, ARDUPILOT);
        assert_eq!(
            onboard.items[1].params[0], -1.0,
            "home, not the first plan item"
        );

        let back = encoded(&onboard.items, onboard.home.as_ref(), ARDUPILOT);
        assert_eq!(back[2].params[0], 0.0, "the home slot again");
    }

    /// Without a home slot there is no home reference to shift.
    #[test]
    fn without_a_home_slot_a_zero_target_stays_item_zero() {
        let wire = encoded(&plan(), None, PX4);
        let onboard = decode(&wire, 0, PX4);

        assert_eq!(onboard.items[1].params[0], 0.0);
        assert_eq!(encoded(&onboard.items, None, PX4)[1].params[0], 0.0);
    }

    /// A target the rule cannot read is opaque and comes through untouched,
    /// whether the layout shifts references or not.
    #[test]
    fn an_opaque_jump_target_is_encoded_unchanged() {
        let mut jump = MissionItem::new(MavCmd::DoJump, None);
        jump.params[0] = 1e30;

        let items = vec![plan()[0].clone(), jump];
        for layout in [ARDUPILOT, PX4] {
            let wire = encoded(&items, None, layout);
            assert_eq!(
                wire.last().expect("the jump is encoded").params[0],
                1e30,
                "{layout:?}"
            );
        }
    }

    #[test]
    fn an_empty_plan_encodes_to_nothing_even_with_a_home_slot() {
        let home = MissionItem::home(Position {
            lat: 47.0,
            lon: 8.0,
            alt_m: 400.0,
        });

        assert_eq!(
            encoded(&[], Some(&home), ARDUPILOT),
            vec![],
            "a lone home row would become a waypoint on an empty ArduPilot mission"
        );
        assert_eq!(encoded(&[], None, PX4), vec![]);
    }

    #[test]
    fn a_plan_that_the_wire_cannot_number_is_refused() {
        let item = MissionItem::waypoint(Position {
            lat: 47.0,
            lon: 8.0,
            alt_m: 40.0,
        });

        assert_eq!(ARDUPILOT.max_items(), 65_534, "home takes one of 65 535");
        assert_eq!(PX4.max_items(), 65_535, "no home row");

        for layout in [ARDUPILOT, PX4] {
            let fits = vec![item.clone(); layout.max_items()];
            assert!(
                encode(&fits, None, layout, MavMissionType::Mission).is_ok(),
                "{layout:?} numbers its largest plan"
            );

            let one_too_many = vec![item.clone(); layout.max_items() + 1];
            assert_eq!(
                encode(&one_too_many, None, layout, MavMissionType::Mission),
                Err(PlanTooLong)
            );
        }
    }
}
