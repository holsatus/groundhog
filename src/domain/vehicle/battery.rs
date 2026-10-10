//! Vehicle battery state.

use std::time::Instant;

use mavio::default_dialect::messages;

#[derive(Debug, Clone, Default)]
pub struct Battery {
    pub state: Option<BatteryState>,
}

#[derive(Clone, Debug)]
pub struct BatteryState {
    pub at: Instant,
    pub voltage: f32,
    pub current: f32,
    pub charge: f32,
}

impl Battery {
    pub fn observe_sys_status(&mut self, message: &messages::SysStatus, time: Instant) {
        self.state = Some(BatteryState {
            at: time,
            voltage: message.voltage_battery as f32 * 1e-3,
            current: message.current_battery as f32 * 1e-2,
            charge: message.battery_remaining as f32,
        });
    }

    pub fn observe_battery_status(&mut self, message: &messages::BatteryStatus, time: Instant) {
        self.state = Some(BatteryState {
            at: time,
            voltage: message
                .voltages
                .iter()
                .cloned()
                .filter_map(|v| (v != u16::MAX).then_some(v as f32))
                .sum::<f32>()
                * 1e-3,
            current: message.current_battery as f32 * 1e-2,
            charge: message.battery_remaining as f32,
        });
    }
}
