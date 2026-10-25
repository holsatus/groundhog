//! Translation between a plan and its wire form for a firmware's layout.
//!
//! Firmwares that reserve sequence 0 for home ([`MissionLayout::home_slot`])
//! shift every plan item up by one on the wire, and with it every reference to
//! an item (jump targets). The plan itself never contains the home item; it is
//! carried separately so the editor's item numbers match what the operator
//! planned, whatever the firmware.

use mavio::default_dialect::{enums::MavMissionType, messages::MissionItemInt};

use super::{MissionItem, OnboardMission, PlanTooLong, Position};
use crate::domain::firmware::MissionLayout;

/// Encode `items` for upload under `layout`. An empty plan encodes to nothing:
/// a lone home row is not sent, because a firmware that reserves the home slot
/// appends it to an empty mission as a waypoint. The zero home placeholder is
/// therefore sent only beside items, where it is harmless: ArduPilot, the
/// firmware with a home slot, ignores what is written there and reads it back
/// as its own home (`AP_Mission::replace_cmd` and `read_cmd_from_storage`).
///
/// # Errors
///
/// [`PlanTooLong`] when `items` cannot be numbered with 16-bit wire sequences.
pub(crate) fn encode(
    items: &[MissionItem],
    home: Option<&MissionItem>,
    layout: MissionLayout,
    mission_type: MavMissionType,
) -> Result<Vec<MissionItemInt>, PlanTooLong> {
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
) -> Option<MissionItemInt> {
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
    wire: &[MissionItemInt],
    opaque_id: u32,
    layout: MissionLayout,
) -> OnboardMission {
    let mut wire: Vec<&MissionItemInt> = wire.iter().collect();
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

/// Move every item reference of `item` by `by`, saturating at zero.
fn shift_targets(item: &mut MissionItem, by: i64) {
    let slots: Vec<usize> = item.item_index_slots().collect();
    for slot in slots {
        let target = item.params[slot];
        if target.is_finite() && target >= 0.0 {
            item.params[slot] = (target as i64 + by).max(0) as f32;
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
    ) -> Vec<MissionItemInt> {
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
        assert_eq!(wire[2].command, MavCmd::DoJump);
        assert_eq!(wire[2].param1, 1.0, "the jump targets wire seq 1");

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
        assert_eq!(wire[1].param1, 0.0);

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
