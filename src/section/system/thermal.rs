//! Temperature monitoring via Linux thermal zones.
//!
//! P5.6 reads thermal zone data from `/sys/class/thermal/thermal_zone*`
//! which exposes sensor names, current temperatures (in millidegrees
//! Celsius) and optional trip points. All reads are strictly read-only
//! sysfs lookups — no processes are spawned and no files are modified.
//!
//! On non-Linux platforms every reader returns an empty list so the
//! project compiles. Unavailable data renders as "Unavailable".

use std::time::Instant;

/// Sysfs base directory for thermal zones.
const THERMAL_BASE: &str = "/sys/class/thermal";

/// Invalid trip point sentinel value (millidegrees) that some firmware
/// reports when no real trip point is configured. We filter these out.
const INVALID_TRIP_TEMP: i64 = -274_000;

/// A single trip point within a thermal zone.
#[derive(Clone, Debug, PartialEq)]
pub struct TripPoint {
    /// Trip point index (0-based).
    pub index: usize,
    /// Temperature threshold in millidegrees Celsius.
    pub temp_milli: i64,
    /// Trip point type label (e.g. "critical", "hot", "passive", "active").
    pub point_type: String,
}

impl TripPoint {
    /// Temperature in whole degrees Celsius.
    pub fn temp_celsius(&self) -> f32 {
        self.temp_milli as f32 / 1000.0
    }
}

/// Snapshot of one thermal zone.
#[derive(Clone, Debug, PartialEq)]
pub struct ThermalZone {
    /// Sysfs zone identifier (e.g. "thermal_zone0").
    pub zone_id: String,
    /// Human-readable sensor type (e.g. "acpitz", "TCPU", "x86_pkg_temp").
    pub type_name: String,
    /// Current temperature in millidegrees Celsius, or `None` if unreadable.
    pub temp_milli: Option<i64>,
    /// Trip points with valid temperatures (invalid sentinels filtered out).
    pub trip_points: Vec<TripPoint>,
}

impl ThermalZone {
    /// Current temperature in whole degrees Celsius, or `None`.
    pub fn temp_celsius(&self) -> Option<f32> {
        self.temp_milli.map(|t| t as f32 / 1000.0)
    }

    /// Critical trip point temperature in °C, if present.
    pub fn critical_temp_celsius(&self) -> Option<f32> {
        self.trip_points
            .iter()
            .find(|tp| tp.point_type == "critical")
            .map(|tp| tp.temp_celsius())
    }
}

/// Cached temperature state owned by the System section. Rendering only
/// reads cached values; [`ThermalMonitor::poll`] is called at most once
/// per second from the section's update loop.
pub struct ThermalMonitor {
    zones: Vec<ThermalZone>,
    last_poll: Option<Instant>,
}

impl ThermalMonitor {
    pub fn new() -> Self {
        Self {
            zones: Vec::new(),
            last_poll: None,
        }
    }

    /// Polls all thermal zones. Called once per second from the update loop.
    pub fn poll(&mut self) {
        self.zones = collect_thermal_zones();
        self.last_poll = Some(Instant::now());
    }

    pub fn zones(&self) -> &[ThermalZone] {
        &self.zones
    }

    /// Maximum current temperature across all zones in °C, or `None`.
    pub fn max_temp_celsius(&self) -> Option<f32> {
        self.zones
            .iter()
            .filter_map(|z| z.temp_celsius())
            .reduce(f32::max)
    }
}

/// Collects all thermal zones from sysfs. On non-Linux platforms returns
/// an empty list.
pub fn collect_thermal_zones() -> Vec<ThermalZone> {
    imp::collect_thermal_zones()
}

/// Parses the `type` file content for a thermal zone.
pub fn parse_zone_type(content: &str) -> String {
    content.trim().to_owned()
}

/// Parses the `temp` file content (millidegrees Celsius) into `Option<i64>`.
/// Non-numeric or empty content yields `None`.
pub fn parse_zone_temp(content: &str) -> Option<i64> {
    let trimmed = content.trim();
    if trimmed.is_empty() {
        return None;
    }
    trimmed.parse().ok()
}

/// Parses a trip point temperature file. Returns `Some(millidegrees)` when
/// the value is valid, `None` for invalid sentinels (e.g. -274000) or
/// unparseable content.
pub fn parse_trip_point_temp(content: &str) -> Option<i64> {
    let temp = parse_zone_temp(content)?;
    if temp <= INVALID_TRIP_TEMP {
        None
    } else {
        Some(temp)
    }
}

/// Parses a trip point type file content.
pub fn parse_trip_point_type(content: &str) -> String {
    content.trim().to_owned()
}

#[cfg(target_os = "linux")]
mod imp {
    use super::*;

    fn read_file(path: &str) -> Option<String> {
        std::fs::read_to_string(path).ok()
    }

    pub fn collect_thermal_zones() -> Vec<ThermalZone> {
        let mut zones = Vec::new();
        let base = std::path::Path::new(THERMAL_BASE);

        let entries = match std::fs::read_dir(base) {
            Ok(entries) => entries,
            Err(_) => return zones,
        };

        for entry in entries.flatten() {
            let name = entry.file_name();
            let name_str = name.to_string_lossy();
            if !name_str.starts_with("thermal_zone") {
                continue;
            }

            let zone_dir = entry.path();
            let type_path = zone_dir.join("type");
            let temp_path = zone_dir.join("temp");

            let type_name = read_file(type_path.to_str().unwrap_or(""))
                .map(|c| parse_zone_type(&c))
                .unwrap_or_default();

            let temp_milli =
                read_file(temp_path.to_str().unwrap_or("")).and_then(|c| parse_zone_temp(&c));

            let mut trip_points = Vec::new();
            for i in 0..20 {
                let tp_temp_path = zone_dir.join(format!("trip_point_{i}_temp"));
                let tp_type_path = zone_dir.join(format!("trip_point_{i}_type"));

                let tp_temp = match read_file(tp_temp_path.to_str().unwrap_or("")) {
                    Some(content) => parse_trip_point_temp(&content),
                    None => break,
                };
                let tp_type = read_file(tp_type_path.to_str().unwrap_or(""))
                    .map(|c| parse_trip_point_type(&c))
                    .unwrap_or_default();

                if let Some(temp) = tp_temp {
                    trip_points.push(TripPoint {
                        index: i,
                        temp_milli: temp,
                        point_type: tp_type,
                    });
                }
            }

            zones.push(ThermalZone {
                zone_id: name_str.into_owned(),
                type_name,
                temp_milli,
                trip_points,
            });
        }

        zones.sort_by(|a, b| a.zone_id.cmp(&b.zone_id));
        zones
    }
}

#[cfg(not(target_os = "linux"))]
mod imp {
    use super::ThermalZone;

    pub fn collect_thermal_zones() -> Vec<ThermalZone> {
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_zone_type() {
        assert_eq!(parse_zone_type("  acpitz  "), "acpitz");
        assert_eq!(parse_zone_type("x86_pkg_temp"), "x86_pkg_temp");
        assert_eq!(parse_zone_type("TCPU"), "TCPU");
    }

    #[test]
    fn parses_zone_temp() {
        assert_eq!(parse_zone_temp("53000"), Some(53_000));
        assert_eq!(parse_zone_temp("  20000\n"), Some(20_000));
        assert_eq!(parse_zone_temp("0"), Some(0));
        assert_eq!(parse_zone_temp(""), None);
        assert_eq!(parse_zone_temp("  \n"), None);
        assert_eq!(parse_zone_temp("banana"), None);
    }

    #[test]
    fn parses_trip_point_temp() {
        assert_eq!(parse_trip_point_temp("100000"), Some(100_000));
        assert_eq!(parse_trip_point_temp("80050"), Some(80_050));
        assert_eq!(parse_trip_point_temp("-274000"), None);
        assert_eq!(parse_trip_point_temp("0"), Some(0));
        assert_eq!(parse_trip_point_temp("banana"), None);
        assert_eq!(parse_trip_point_temp(""), None);
    }

    #[test]
    fn parses_trip_point_type() {
        assert_eq!(parse_trip_point_type("critical"), "critical");
        assert_eq!(parse_trip_point_type("  hot  "), "hot");
        assert_eq!(parse_trip_point_type("passive"), "passive");
        assert_eq!(parse_trip_point_type("active"), "active");
    }

    #[test]
    fn thermal_zone_temp_celsius_conversion() {
        let zone = ThermalZone {
            zone_id: "thermal_zone0".into(),
            type_name: "acpitz".into(),
            temp_milli: Some(53_000),
            trip_points: vec![],
        };
        assert_eq!(zone.temp_celsius(), Some(53.0));
    }

    #[test]
    fn thermal_zone_none_temp_yields_none_celsius() {
        let zone = ThermalZone {
            zone_id: "thermal_zone1".into(),
            type_name: "INT3400".into(),
            temp_milli: None,
            trip_points: vec![],
        };
        assert_eq!(zone.temp_celsius(), None);
    }

    #[test]
    fn finds_critical_trip_point() {
        let zone = ThermalZone {
            zone_id: "thermal_zone0".into(),
            type_name: "acpitz".into(),
            temp_milli: Some(53_000),
            trip_points: vec![TripPoint {
                index: 0,
                temp_milli: 100_000,
                point_type: "critical".into(),
            }],
        };
        assert_eq!(zone.critical_temp_celsius(), Some(100.0));
    }

    #[test]
    fn no_critical_trip_point_returns_none() {
        let zone = ThermalZone {
            zone_id: "thermal_zone0".into(),
            type_name: "TCPU".into(),
            temp_milli: Some(48_000),
            trip_points: vec![TripPoint {
                index: 0,
                temp_milli: 110_050,
                point_type: "hot".into(),
            }],
        };
        assert_eq!(zone.critical_temp_celsius(), None);
    }

    #[test]
    fn max_temp_celsius_selects_hottest_zone() {
        let mut monitor = ThermalMonitor::new();
        monitor.zones = vec![
            ThermalZone {
                zone_id: "zone0".into(),
                type_name: "A".into(),
                temp_milli: Some(43_050),
                trip_points: vec![],
            },
            ThermalZone {
                zone_id: "zone1".into(),
                type_name: "B".into(),
                temp_milli: Some(51_000),
                trip_points: vec![],
            },
            ThermalZone {
                zone_id: "zone2".into(),
                type_name: "C".into(),
                temp_milli: None,
                trip_points: vec![],
            },
        ];
        assert_eq!(monitor.max_temp_celsius(), Some(51.0));
    }

    #[test]
    fn max_temp_celsius_empty_zones() {
        let monitor = ThermalMonitor::new();
        assert_eq!(monitor.max_temp_celsius(), None);
    }

    #[test]
    fn trip_point_celsius_conversion() {
        let tp = TripPoint {
            index: 0,
            temp_milli: 100_050,
            point_type: "critical".into(),
        };
        assert!((tp.temp_celsius() - 100.05).abs() < 0.01);
    }
}
