//! Translation between a plan and its wire form for a firmware's layout.
//!
//! Firmwares that reserve sequence 0 for home ([`MissionLayout::home_slot`])
//! shift every plan item up by one on the wire, and with it every reference to
//! an item (jump targets). The plan itself never contains the home item; it is
//! carried separately so the editor's item numbers match what the operator
//! planned, whatever the firmware.

use mavio::default_dialect::{enums::MavMissionType, messages::MissionItemInt};

use super::{MissionItem, OnboardMission};
use crate::domain::firmware::MissionLayout;

/// Encode `items` for upload under `layout`. `home` fills the home slot when
/// the layout has one; without a known home, a zero position is sent, which
/// such firmwares overwrite with their actual home.
pub fn encode(
    items: &[MissionItem],
    home: Option<&MissionItem>,
    layout: MissionLayout,
    mission_type: MavMissionType,
) -> Vec<MissionItemInt> {
    let mut wire = Vec::with_capacity(items.len() + 1);

    if layout.home_slot {
        let home = home.cloned().unwrap_or_else(|| {
            MissionItem::home(super::Position {
                lat: 0.0,
                lon: 0.0,
                alt_m: 0.0,
            })
        });
        wire.push(home.to_wire(0, mission_type));
    }

    for (index, item) in items.iter().enumerate() {
        let mut item = item.clone();
        shift_targets(&mut item, i64::from(layout.offset()));
        wire.push(item.to_wire(layout.wire_seq(index), mission_type));
    }

    wire
}

/// Decode a downloaded mission under `layout`. Items are ordered by their
/// sequence number; the home slot (if the layout has one) becomes
/// [`OnboardMission::home`].
pub fn decode(wire: &[MissionItemInt], opaque_id: u32, layout: MissionLayout) -> OnboardMission {
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

    #[test]
    fn the_home_slot_shifts_items_and_jump_targets() {
        let home = MissionItem::home(Position {
            lat: 47.1,
            lon: 8.1,
            alt_m: 400.0,
        });
        let wire = encode(&plan(), Some(&home), ARDUPILOT, MavMissionType::Mission);

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
        let wire = encode(&plan(), None, PX4, MavMissionType::Mission);
        assert_eq!(wire.len(), 2);
        assert_eq!(wire[1].param1, 0.0);

        let onboard = decode(&wire, 0, PX4);
        assert_eq!(onboard.items, plan());
        assert_eq!(onboard.home, None);
    }

    #[test]
    fn decoding_orders_by_sequence() {
        let mut wire = encode(&plan(), None, PX4, MavMissionType::Mission);
        wire.reverse();
        assert_eq!(decode(&wire, 0, PX4).items, plan());
    }
}
