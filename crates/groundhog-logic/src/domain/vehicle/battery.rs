//! Vehicle battery state.
//!
//! MAVLink marks unknown values with sentinels (`UINT16_MAX` volts, `-1`
//! current or charge); those become `None` here rather than nonsense like
//! "-1 %". `BATTERY_STATUS` (per battery, by id) takes precedence over the
//! single summary in `SYS_STATUS` once a vehicle sends it, for each value it
//! actually reports.

use std::collections::BTreeMap;

use mavio::default_dialect::messages;

#[derive(Debug, Clone, Default)]
pub struct Battery {
    /// The summary from `SYS_STATUS`.
    summary: Option<BatteryState>,
    /// Individual batteries from `BATTERY_STATUS`, by id.
    batteries: BTreeMap<u8, BatteryState>,
}

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct BatteryState {
    pub voltage: Option<f32>,
    pub current: Option<f32>,
    /// Remaining charge in percent.
    pub remaining: Option<f32>,
}

impl Battery {
    /// The main battery: battery 0 if reported individually, else the
    /// lowest-numbered battery reported individually, else the `SYS_STATUS`
    /// summary. Field by field: what the chosen battery does not report (a
    /// monitor sending only sentinels) comes from the summary.
    pub fn primary(&self) -> Option<BatteryState> {
        let individual = self
            .batteries
            .get(&0)
            .or_else(|| self.batteries.values().next());

        match (individual, self.summary) {
            (Some(battery), Some(summary)) => Some(BatteryState {
                voltage: battery.voltage.or(summary.voltage),
                current: battery.current.or(summary.current),
                remaining: battery.remaining.or(summary.remaining),
            }),
            (Some(battery), None) => Some(*battery),
            (None, summary) => summary,
        }
    }

    pub(crate) fn observe_sys_status(&mut self, message: &messages::SysStatus) {
        self.summary = Some(BatteryState {
            voltage: (message.voltage_battery != u16::MAX)
                .then(|| message.voltage_battery as f32 / 1000.0),
            current: (message.current_battery >= 0).then(|| message.current_battery as f32 / 100.0),
            remaining: (message.battery_remaining >= 0).then_some(message.battery_remaining as f32),
        });
    }

    pub(crate) fn observe_battery_status(&mut self, message: &messages::BatteryStatus) {
        let cells: Vec<f32> = message
            .voltages
            .iter()
            .chain(message.voltages_ext.iter())
            .filter(|millivolts| **millivolts != u16::MAX && **millivolts != 0)
            .map(|millivolts| *millivolts as f32 / 1000.0)
            .collect();

        self.batteries.insert(
            message.id,
            BatteryState {
                voltage: (!cells.is_empty()).then_some(cells.iter().sum()),
                current: (message.current_battery >= 0)
                    .then(|| message.current_battery as f32 / 100.0),
                remaining: (message.battery_remaining >= 0)
                    .then_some(message.battery_remaining as f32),
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sentinels_mean_unknown() {
        let mut battery = Battery::default();
        battery.observe_sys_status(&messages::SysStatus {
            voltage_battery: u16::MAX,
            current_battery: -1,
            battery_remaining: -1,
            ..Default::default()
        });

        let state = battery.primary().unwrap();
        assert_eq!(state.voltage, None);
        assert_eq!(state.current, None);
        assert_eq!(state.remaining, None);
    }

    #[test]
    fn an_empty_battery_report_does_not_hide_the_summary() {
        let mut battery = Battery::default();
        battery.observe_sys_status(&messages::SysStatus {
            voltage_battery: 12_000,
            current_battery: -1,
            battery_remaining: 50,
            ..Default::default()
        });
        battery.observe_battery_status(&messages::BatteryStatus {
            id: 0,
            voltages: [u16::MAX; 10],
            current_battery: 250,
            battery_remaining: -1,
            ..Default::default()
        });

        let state = battery.primary().unwrap();
        assert_eq!(state.voltage, Some(12.0), "from the summary");
        assert_eq!(state.current, Some(2.5), "from the battery");
        assert_eq!(state.remaining, Some(50.0), "from the summary");
    }

    #[test]
    fn battery_status_takes_precedence() {
        let mut battery = Battery::default();
        battery.observe_sys_status(&messages::SysStatus {
            voltage_battery: 12_000,
            current_battery: 100,
            battery_remaining: 50,
            ..Default::default()
        });

        let mut voltages = [u16::MAX; 10];
        voltages[..3].copy_from_slice(&[4_000, 4_000, 4_000]);
        battery.observe_battery_status(&messages::BatteryStatus {
            id: 0,
            voltages,
            current_battery: 250,
            battery_remaining: 80,
            ..Default::default()
        });

        let state = battery.primary().unwrap();
        assert!((state.voltage.unwrap() - 12.0).abs() < 1e-6);
        assert_eq!(state.current, Some(2.5));
        assert_eq!(state.remaining, Some(80.0));
    }
}
