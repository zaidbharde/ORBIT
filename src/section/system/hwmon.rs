//! Hardware sensor monitoring via Linux hwmon sysfs.
//!
//! P5.7 reads hardware sensor data from `/sys/class/hwmon/hwmon*` which
//! exposes fan speeds, temperatures, voltages and currents through a
//! standard Linux kernel interface. All reads are strictly read-only sysfs
//! lookups — no processes are spawned, no external commands are executed,
//! and no files are modified.
//!
//! On non-Linux platforms every reader returns an empty list so the
//! project compiles. Unavailable data renders as "Unavailable".

use std::collections::VecDeque;

/// Sysfs base directory for hwmon devices.
const HWMON_BASE: &str = "/sys/class/hwmon";

/// Maximum number of fans per device to probe (most systems have 1–4).
const MAX_FANS: usize = 16;

/// Maximum number of temperature sensors per device to probe.
const MAX_TEMPS: usize = 32;

/// Maximum number of voltage inputs per device to probe.
const MAX_VOLTAGES: usize = 16;

/// Maximum number of current inputs per device to probe.
const MAX_CURRENTS: usize = 16;

// ---------------------------------------------------------------------------
// Data models
// ---------------------------------------------------------------------------

/// A single fan sensor reading.
#[derive(Clone, Debug, PartialEq)]
pub struct FanSensor {
    /// 1-based fan index within this hwmon device.
    pub index: usize,
    /// Human-readable label from `fan*_label`, if present.
    pub label: Option<String>,
    /// Current speed in RPM, or `None` if unreadable.
    pub rpm: Option<u32>,
}

/// A single temperature sensor reading from hwmon.
#[derive(Clone, Debug, PartialEq)]
pub struct TempSensor {
    /// 1-based temp index within this hwmon device.
    pub index: usize,
    /// Human-readable label from `temp*_label`, if present.
    pub label: Option<String>,
    /// Temperature in millidegrees Celsius, or `None` if unreadable.
    pub temp_milli: Option<i64>,
}

impl TempSensor {
    /// Temperature in whole degrees Celsius, or `None`.
    pub fn temp_celsius(&self) -> Option<f32> {
        self.temp_milli.map(|t| t as f32 / 1000.0)
    }
}

/// A single voltage sensor reading from hwmon (`in*_input`).
#[derive(Clone, Debug, PartialEq)]
pub struct VoltageSensor {
    /// 1-based input index within this hwmon device.
    pub index: usize,
    /// Human-readable label from `in*_label`, if present.
    pub label: Option<String>,
    /// Voltage in millivolts, or `None` if unreadable.
    pub millivolt: Option<u32>,
}

impl VoltageSensor {
    /// Voltage in volts, or `None`.
    pub fn volts(&self) -> Option<f32> {
        self.millivolt.map(|mv| mv as f32 / 1000.0)
    }
}

/// A single current sensor reading from hwmon (`curr*_input`).
#[derive(Clone, Debug, PartialEq)]
pub struct CurrentSensor {
    /// 1-based input index within this hwmon device.
    pub index: usize,
    /// Human-readable label from `curr*_label`, if present.
    pub label: Option<String>,
    /// Current in milliamps, or `None` if unreadable.
    pub milliamp: Option<u32>,
}

impl CurrentSensor {
    /// Current in amps, or `None`.
    pub fn amps(&self) -> Option<f32> {
        self.milliamp.map(|ma| ma as f32 / 1000.0)
    }
}

/// Snapshot of one hwmon device with all its sensors.
#[derive(Clone, Debug, PartialEq)]
pub struct HwmonDevice {
    /// Sysfs identifier (e.g. "hwmon4").
    pub id: String,
    /// Device name from the `name` file (e.g. "msi_wmi_platform").
    pub name: String,
    /// Fan sensors with available readings.
    pub fans: Vec<FanSensor>,
    /// Temperature sensors.
    pub temps: Vec<TempSensor>,
    /// Voltage sensors.
    pub voltages: Vec<VoltageSensor>,
    /// Current sensors.
    pub currents: Vec<CurrentSensor>,
}

/// Cached hwmon state owned by the System section. Rendering only reads
/// cached values; [`HwmonMonitor::poll`] is called at most once per second
/// from the section's update loop.
pub struct HwmonMonitor {
    devices: Vec<HwmonDevice>,
    fan_history: VecDeque<f32>,
}

impl HwmonMonitor {
    pub fn new() -> Self {
        Self {
            devices: Vec::new(),
            fan_history: VecDeque::with_capacity(60),
        }
    }

    /// Polls all hwmon devices. Called once per second from the update loop.
    pub fn poll(&mut self) {
        self.devices = collect_hwmon_devices();
        if let Some(rpm) = self.primary_fan_rpm() {
            if self.fan_history.len() == 60 {
                self.fan_history.pop_front();
            }
            self.fan_history.push_back(rpm as f32);
        }
    }

    /// All fan sensors across all devices.
    pub fn all_fans(&self) -> Vec<(&HwmonDevice, &FanSensor)> {
        self.devices
            .iter()
            .flat_map(|d| d.fans.iter().map(move |f| (d, f)))
            .collect()
    }

    /// RPM of the first available spinning fan, or `None`.
    pub fn primary_fan_rpm(&self) -> Option<u32> {
        self.devices
            .iter()
            .flat_map(|d| &d.fans)
            .find(|f| f.rpm.map_or(false, |r| r > 0))
            .and_then(|f| f.rpm)
    }

    /// All hwmon temperature sensors across all devices.
    pub fn all_temps(&self) -> Vec<(&HwmonDevice, &TempSensor)> {
        self.devices
            .iter()
            .flat_map(|d| d.temps.iter().map(move |t| (d, t)))
            .collect()
    }

    /// All voltage sensors across all devices.
    pub fn all_voltages(&self) -> Vec<(&HwmonDevice, &VoltageSensor)> {
        self.devices
            .iter()
            .flat_map(|d| d.voltages.iter().map(move |v| (d, v)))
            .collect()
    }

    /// All current sensors across all devices.
    pub fn all_currents(&self) -> Vec<(&HwmonDevice, &CurrentSensor)> {
        self.devices
            .iter()
            .flat_map(|d| d.currents.iter().map(move |c| (d, c)))
            .collect()
    }
}

/// Collects all hwmon devices from sysfs. On non-Linux platforms returns
/// an empty list.
pub fn collect_hwmon_devices() -> Vec<HwmonDevice> {
    imp::collect_hwmon_devices()
}

// ---------------------------------------------------------------------------
// Parsing helpers (public for unit testing)
// ---------------------------------------------------------------------------

/// Parses a fan label file content.
pub fn parse_fan_label(content: &str) -> String {
    content.trim().to_owned()
}

/// Parses a fan RPM value (fan*_input is in RPM).
pub fn parse_fan_rpm(content: &str) -> Option<u32> {
    let trimmed = content.trim();
    if trimmed.is_empty() {
        return None;
    }
    trimmed.parse().ok()
}

/// Parses a temperature value (temp*_input is in millidegrees Celsius).
pub fn parse_temp_milli(content: &str) -> Option<i64> {
    let trimmed = content.trim();
    if trimmed.is_empty() {
        return None;
    }
    trimmed.parse().ok()
}

/// Parses a voltage value (in*_input is in millivolts).
pub fn parse_voltage_millivolt(content: &str) -> Option<u32> {
    let trimmed = content.trim();
    if trimmed.is_empty() {
        return None;
    }
    trimmed.parse().ok()
}

/// Parses a current value (curr*_input is in milliamps).
pub fn parse_current_milliamp(content: &str) -> Option<u32> {
    let trimmed = content.trim();
    if trimmed.is_empty() {
        return None;
    }
    trimmed.parse().ok()
}

/// Parses a hwmon device name file.
pub fn parse_device_name(content: &str) -> String {
    content.trim().to_owned()
}

// ---------------------------------------------------------------------------
// Platform-specific implementation
// ---------------------------------------------------------------------------

#[cfg(target_os = "linux")]
mod imp {
    use super::*;

    fn read_file(path: &str) -> Option<String> {
        std::fs::read_to_string(path).ok()
    }

    pub fn collect_hwmon_devices() -> Vec<HwmonDevice> {
        let mut devices = Vec::new();
        let base = std::path::Path::new(HWMON_BASE);

        let entries = match std::fs::read_dir(base) {
            Ok(entries) => entries,
            Err(_) => return devices,
        };

        for entry in entries.flatten() {
            let name = entry.file_name();
            let name_str = name.to_string_lossy();
            if !name_str.starts_with("hwmon") {
                continue;
            }

            let hwmon_dir = entry.path();
            let name_path = hwmon_dir.join("name");

            let device_name = read_file(name_path.to_str().unwrap_or(""))
                .map(|c| parse_device_name(&c))
                .unwrap_or_default();

            if device_name.is_empty() {
                continue;
            }

            let mut fans = Vec::new();
            for i in 1..=MAX_FANS {
                let fan_path = hwmon_dir.join(format!("fan{i}_input"));
                let content = match read_file(fan_path.to_str().unwrap_or("")) {
                    Some(c) => c,
                    None => continue,
                };
                let rpm = parse_fan_rpm(&content);
                let label = read_file(
                    hwmon_dir
                        .join(format!("fan{i}_label"))
                        .to_str()
                        .unwrap_or(""),
                )
                .map(|c| parse_fan_label(&c));
                fans.push(FanSensor {
                    index: i,
                    label,
                    rpm,
                });
            }

            let mut temps = Vec::new();
            for i in 1..=MAX_TEMPS {
                let temp_path = hwmon_dir.join(format!("temp{i}_input"));
                let content = match read_file(temp_path.to_str().unwrap_or("")) {
                    Some(c) => c,
                    None => continue,
                };
                let temp_milli = parse_temp_milli(&content);
                let label = read_file(
                    hwmon_dir
                        .join(format!("temp{i}_label"))
                        .to_str()
                        .unwrap_or(""),
                )
                .map(|c| parse_fan_label(&c));
                temps.push(TempSensor {
                    index: i,
                    label,
                    temp_milli,
                });
            }

            let mut voltages = Vec::new();
            for i in 1..=MAX_VOLTAGES {
                let in_path = hwmon_dir.join(format!("in{i}_input"));
                let content = match read_file(in_path.to_str().unwrap_or("")) {
                    Some(c) => c,
                    None => continue,
                };
                let millivolt = parse_voltage_millivolt(&content);
                let label = read_file(
                    hwmon_dir
                        .join(format!("in{i}_label"))
                        .to_str()
                        .unwrap_or(""),
                )
                .map(|c| parse_fan_label(&c));
                voltages.push(VoltageSensor {
                    index: i,
                    label,
                    millivolt,
                });
            }

            let mut currents = Vec::new();
            for i in 1..=MAX_CURRENTS {
                let curr_path = hwmon_dir.join(format!("curr{i}_input"));
                let content = match read_file(curr_path.to_str().unwrap_or("")) {
                    Some(c) => c,
                    None => continue,
                };
                let milliamp = parse_current_milliamp(&content);
                let label = read_file(
                    hwmon_dir
                        .join(format!("curr{i}_label"))
                        .to_str()
                        .unwrap_or(""),
                )
                .map(|c| parse_fan_label(&c));
                currents.push(CurrentSensor {
                    index: i,
                    label,
                    milliamp,
                });
            }

            devices.push(HwmonDevice {
                id: name_str.into_owned(),
                name: device_name,
                fans,
                temps,
                voltages,
                currents,
            });
        }

        devices.sort_by(|a, b| a.id.cmp(&b.id));
        devices
    }
}

#[cfg(not(target_os = "linux"))]
mod imp {
    use super::HwmonDevice;

    pub fn collect_hwmon_devices() -> Vec<HwmonDevice> {
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_fan_rpm() {
        assert_eq!(parse_fan_rpm("2341"), Some(2341));
        assert_eq!(parse_fan_rpm("  0\n"), Some(0));
        assert_eq!(parse_fan_rpm("0"), Some(0));
        assert_eq!(parse_fan_rpm(""), None);
        assert_eq!(parse_fan_rpm("  \n"), None);
        assert_eq!(parse_fan_rpm("banana"), None);
    }

    #[test]
    fn parses_fan_label() {
        assert_eq!(parse_fan_label("  Fan #1  "), "Fan #1");
        assert_eq!(parse_fan_label("CPU Fan"), "CPU Fan");
        assert_eq!(parse_fan_label("\n"), "");
    }

    #[test]
    fn parses_temp_milli() {
        assert_eq!(parse_temp_milli("53000"), Some(53_000));
        assert_eq!(parse_temp_milli("  20000\n"), Some(20_000));
        assert_eq!(parse_temp_milli("0"), Some(0));
        assert_eq!(parse_temp_milli("-1000"), Some(-1000));
        assert_eq!(parse_temp_milli(""), None);
        assert_eq!(parse_temp_milli("  \n"), None);
        assert_eq!(parse_temp_milli("banana"), None);
    }

    #[test]
    fn parses_voltage_millivolt() {
        assert_eq!(parse_voltage_millivolt("12803"), Some(12803));
        assert_eq!(parse_voltage_millivolt("  3300\n"), Some(3300));
        assert_eq!(parse_voltage_millivolt("0"), Some(0));
        assert_eq!(parse_voltage_millivolt(""), None);
        assert_eq!(parse_voltage_millivolt("banana"), None);
    }

    #[test]
    fn parses_current_milliamp() {
        assert_eq!(parse_current_milliamp("991"), Some(991));
        assert_eq!(parse_current_milliamp("  500\n"), Some(500));
        assert_eq!(parse_current_milliamp("0"), Some(0));
        assert_eq!(parse_current_milliamp(""), None);
        assert_eq!(parse_current_milliamp("banana"), None);
    }

    #[test]
    fn parses_device_name() {
        assert_eq!(
            parse_device_name("  msi_wmi_platform  "),
            "msi_wmi_platform"
        );
        assert_eq!(parse_device_name("coretemp"), "coretemp");
        assert_eq!(parse_device_name("\n"), "");
    }

    #[test]
    fn temp_sensor_celsius_conversion() {
        let sensor = TempSensor {
            index: 1,
            label: Some("Composite".into()),
            temp_milli: Some(39_850),
        };
        assert!((sensor.temp_celsius().unwrap() - 39.85).abs() < 0.01);
    }

    #[test]
    fn temp_sensor_none_temp_yields_none_celsius() {
        let sensor = TempSensor {
            index: 1,
            label: None,
            temp_milli: None,
        };
        assert_eq!(sensor.temp_celsius(), None);
    }

    #[test]
    fn voltage_sensor_volts_conversion() {
        let sensor = VoltageSensor {
            index: 0,
            label: Some("bat0".into()),
            millivolt: Some(12803),
        };
        assert!((sensor.volts().unwrap() - 12.803).abs() < 0.01);
    }

    #[test]
    fn voltage_sensor_none_yields_none() {
        let sensor = VoltageSensor {
            index: 0,
            label: None,
            millivolt: None,
        };
        assert_eq!(sensor.volts(), None);
    }

    #[test]
    fn current_sensor_amps_conversion() {
        let sensor = CurrentSensor {
            index: 1,
            label: Some("bat0".into()),
            milliamp: Some(991),
        };
        assert!((sensor.amps().unwrap() - 0.991).abs() < 0.001);
    }

    #[test]
    fn current_sensor_none_yields_none() {
        let sensor = CurrentSensor {
            index: 1,
            label: None,
            milliamp: None,
        };
        assert_eq!(sensor.amps(), None);
    }

    #[test]
    fn primary_fan_rpm_selects_first_spinning() {
        let monitor = HwmonMonitor {
            devices: vec![HwmonDevice {
                id: "hwmon4".into(),
                name: "msi_wmi_platform".into(),
                fans: vec![
                    FanSensor {
                        index: 1,
                        label: None,
                        rpm: Some(2341),
                    },
                    FanSensor {
                        index: 2,
                        label: None,
                        rpm: Some(0),
                    },
                ],
                temps: vec![],
                voltages: vec![],
                currents: vec![],
            }],
            fan_history: VecDeque::new(),
        };
        assert_eq!(monitor.primary_fan_rpm(), Some(2341));
    }

    #[test]
    fn primary_fan_rpm_none_when_no_fans() {
        let monitor = HwmonMonitor::new();
        assert_eq!(monitor.primary_fan_rpm(), None);
    }

    #[test]
    fn primary_fan_rpm_none_when_all_zero() {
        let monitor = HwmonMonitor {
            devices: vec![HwmonDevice {
                id: "hwmon4".into(),
                name: "msi_wmi_platform".into(),
                fans: vec![FanSensor {
                    index: 1,
                    label: None,
                    rpm: Some(0),
                }],
                temps: vec![],
                voltages: vec![],
                currents: vec![],
            }],
            fan_history: VecDeque::new(),
        };
        assert_eq!(monitor.primary_fan_rpm(), None);
    }

    #[test]
    fn all_fans_collects_across_devices() {
        let monitor = HwmonMonitor {
            devices: vec![
                HwmonDevice {
                    id: "hwmon4".into(),
                    name: "msi_wmi_platform".into(),
                    fans: vec![FanSensor {
                        index: 1,
                        label: None,
                        rpm: Some(2341),
                    }],
                    temps: vec![],
                    voltages: vec![],
                    currents: vec![],
                },
                HwmonDevice {
                    id: "hwmon6".into(),
                    name: "coretemp".into(),
                    fans: vec![FanSensor {
                        index: 1,
                        label: Some("Processor".into()),
                        rpm: Some(1200),
                    }],
                    temps: vec![],
                    voltages: vec![],
                    currents: vec![],
                },
            ],
            fan_history: VecDeque::new(),
        };
        assert_eq!(monitor.all_fans().len(), 2);
    }

    #[test]
    fn fan_history_is_capped_at_60() {
        let mut monitor = HwmonMonitor::new();
        for i in 0..70 {
            monitor.fan_history.push_back(i as f32);
            if monitor.fan_history.len() > 60 {
                monitor.fan_history.pop_front();
            }
        }
        assert_eq!(monitor.fan_history.len(), 60);
    }

    #[test]
    fn hwmon_monitor_starts_empty() {
        let monitor = HwmonMonitor::new();
        assert!(monitor.devices.is_empty());
        assert!(monitor.fan_history.is_empty());
    }
}
