//! System information and live metrics collection.
//!
//! Static info (hostname, OS, kernel, CPU model, ...) is read once when the
//! System section is created. Dynamic metrics (CPU ticks, memory, uptime)
//! are re-read at ~1 Hz by the section's update loop. Everything is sourced
//! directly from Linux `/proc` — no external dependencies. On non-Linux
//! platforms every reader reports `None` and the UI shows "Unavailable".

/// Monotonic CPU tick snapshot: one `Vec<u64>` per `/proc/stat` cpu line,
/// in order (aggregate `cpu` first, then `cpu0`, `cpu1`, ...).
pub type CpuTicks = Vec<Vec<u64>>;

/// Static system information, collected once at construction.
pub struct SystemInfo {
    pub hostname: Option<String>,
    pub os_name: Option<String>,
    pub kernel: Option<String>,
    pub architecture: &'static str,
    pub cpu_model: Option<String>,
    pub logical_cores: Option<usize>,
    pub physical_cores: Option<usize>,
    pub total_ram: Option<u64>,
    pub orbit_version: &'static str,
}

/// Dynamic metrics, refreshed at ~1 Hz by [`super::SystemSection`].
#[derive(Default)]
pub struct SystemMetrics {
    /// Overall CPU usage in percent (0.0 - 100.0).
    pub cpu_usage: Option<f32>,
    /// Per-logical-core CPU usage in percent; empty until the first delta.
    pub per_core_usage: Vec<Option<f32>>,
    pub memory_total: Option<u64>,
    pub memory_used: Option<u64>,
    pub memory_available: Option<u64>,
    /// Used memory as a percent of total (0.0 - 100.0).
    pub memory_usage: Option<f32>,
    pub uptime_secs: Option<u64>,
}

/// Number of CPU time fields per `/proc/stat` cpu line.
const CPU_FIELDS: usize = 8;

/// Index of the `idle` field in a cpu line.
const IDLE_INDEX: usize = 3;

/// Index of the `iowait` field, which counts as idle time.
const IOWAIT_INDEX: usize = 4;

/// Collects all static system information at once.
pub fn collect_system_info() -> SystemInfo {
    imp::collect_system_info()
}

/// Snapshot of `/proc/stat` cpu lines, or `None` when unreadable.
pub fn read_cpu_ticks() -> Option<CpuTicks> {
    imp::read_cpu_ticks()
}

/// Total and available memory in KiB.
pub fn read_memory_kib() -> Option<(u64, u64)> {
    imp::read_memory_kib()
}

/// System uptime in whole seconds.
pub fn read_uptime_secs() -> Option<u64> {
    imp::read_uptime_secs()
}

/// CPU usage percentage between two tick snapshots: overall (the aggregate
/// `cpu` line) plus one value per logical core. Returns `None` when the
/// snapshots are incompatible (different core counts or a shrinking clock).
pub fn cpu_usage_delta(prev: &CpuTicks, current: &CpuTicks) -> Option<(f32, Vec<f32>)> {
    if prev.len() != current.len() {
        return None;
    }
    let overall = usage_between(&prev[0], &current[0])?;
    let per_core = prev
        .iter()
        .zip(current)
        .skip(1)
        .map(|(before, after)| usage_between(before, after))
        .collect::<Option<Vec<_>>>()?;
    Some((overall, per_core))
}

fn usage_between(prev: &[u64], current: &[u64]) -> Option<f32> {
    if prev.len() < CPU_FIELDS || current.len() < CPU_FIELDS {
        return None;
    }
    let prev_total: u64 = prev.iter().take(CPU_FIELDS).sum();
    let current_total: u64 = current.iter().take(CPU_FIELDS).sum();
    let prev_idle = prev[IDLE_INDEX] + prev[IOWAIT_INDEX];
    let current_idle = current[IDLE_INDEX] + current[IOWAIT_INDEX];
    let total_delta = current_total.checked_sub(prev_total)?;
    let idle_delta = current_idle.checked_sub(prev_idle)?;
    if total_delta == 0 {
        return Some(0.0);
    }
    let busy = total_delta.saturating_sub(idle_delta);
    Some(busy as f32 / total_delta as f32 * 100.0)
}

fn parse_cpu_line(line: &str) -> Option<Vec<u64>> {
    let mut fields = line.split_whitespace().skip(1);
    let mut values = Vec::with_capacity(CPU_FIELDS);
    for _ in 0..CPU_FIELDS {
        values.push(fields.next()?.parse().ok()?);
    }
    Some(values)
}

fn parse_meminfo(content: &str) -> Option<(u64, u64)> {
    let mut total = None;
    let mut available = None;
    let mut free = None;
    let mut buffers = None;
    let mut cached = None;
    for line in content.lines() {
        let mut parts = line.split_whitespace();
        let (Some(key), Some(value)) = (parts.next(), parts.next()) else {
            continue;
        };
        let Ok(value) = value.parse::<u64>() else {
            continue;
        };
        match key.trim_end_matches(':') {
            "MemTotal" => total = Some(value),
            "MemAvailable" => available = Some(value),
            "MemFree" => free = Some(value),
            "Buffers" => buffers = Some(value),
            "Cached" => cached = Some(value),
            _ => {}
        }
    }
    // Older kernels lack MemAvailable; approximate it as free + buffers + cached.
    let available = available.or_else(|| match (free, buffers, cached) {
        (Some(free), Some(buffers), Some(cached)) => Some(free + buffers + cached),
        _ => None,
    });
    Some((total?, available?))
}

fn parse_os_release(content: &str) -> Option<String> {
    let mut name = None;
    let mut version = None;
    let mut pretty = None;
    for line in content.lines() {
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let value = value.trim().trim_matches('"').to_owned();
        match key.trim() {
            "PRETTY_NAME" => pretty = Some(value),
            "NAME" => name = Some(value),
            "VERSION" => version = Some(value),
            _ => {}
        }
    }
    pretty.or_else(|| match (name, version) {
        (Some(name), Some(version)) => Some(format!("{name} {version}")),
        (Some(name), None) => Some(name),
        _ => None,
    })
}

fn parse_uptime(content: &str) -> Option<u64> {
    let first = content.split_whitespace().next()?;
    let secs = first.parse::<f64>().ok()?;
    Some(secs.max(0.0) as u64)
}

fn parse_cpuinfo(content: &str) -> (Option<String>, Option<usize>, Option<usize>) {
    let mut model = None;
    let mut processors = 0usize;
    let mut cores = None;
    for line in content.lines() {
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let key = key.trim();
        let value = value.trim();
        match key {
            "model name" => model = Some(value.to_owned()),
            "Hardware" | "Processor" if model.is_none() => model = Some(value.to_owned()),
            "processor" => processors += 1,
            "cpu cores" => cores = value.parse().ok(),
            _ => {}
        }
    }
    let logical = if processors > 0 {
        Some(processors)
    } else {
        None
    };
    (model, logical, cores)
}

#[cfg(target_os = "linux")]
mod imp {
    use super::{
        CpuTicks, SystemInfo, parse_cpu_line, parse_cpuinfo, parse_meminfo, parse_os_release,
        parse_uptime,
    };

    fn read_file(path: &str) -> Option<String> {
        std::fs::read_to_string(path).ok()
    }

    pub fn collect_system_info() -> SystemInfo {
        let hostname = read_file("/proc/sys/kernel/hostname").map(|value| value.trim().to_owned());
        let kernel = read_file("/proc/sys/kernel/osrelease").map(|value| value.trim().to_owned());
        let os_name = read_file("/etc/os-release").and_then(|content| parse_os_release(&content));
        let (cpu_model, mut logical_cores, physical_cores) = read_file("/proc/cpuinfo")
            .map(|content| parse_cpuinfo(&content))
            .unwrap_or((None, None, None));
        if logical_cores.is_none() {
            logical_cores = read_cpu_ticks()
                .map(|ticks| ticks.len().saturating_sub(1))
                .filter(|count| *count > 0);
        }
        let total_ram = read_memory_kib().map(|(total, _)| total * 1024);
        SystemInfo {
            hostname,
            os_name,
            kernel,
            architecture: std::env::consts::ARCH,
            cpu_model,
            logical_cores,
            physical_cores,
            total_ram,
            orbit_version: env!("CARGO_PKG_VERSION"),
        }
    }

    pub fn read_cpu_ticks() -> Option<CpuTicks> {
        let content = read_file("/proc/stat")?;
        let mut ticks = Vec::new();
        for line in content.lines() {
            if line.starts_with("cpu") {
                if let Some(values) = parse_cpu_line(line) {
                    ticks.push(values);
                }
            }
        }
        if ticks.is_empty() { None } else { Some(ticks) }
    }

    pub fn read_memory_kib() -> Option<(u64, u64)> {
        read_file("/proc/meminfo").and_then(|content| parse_meminfo(&content))
    }

    pub fn read_uptime_secs() -> Option<u64> {
        read_file("/proc/uptime").and_then(|content| parse_uptime(&content))
    }
}

#[cfg(not(target_os = "linux"))]
mod imp {
    use super::{CpuTicks, SystemInfo};

    pub fn collect_system_info() -> SystemInfo {
        SystemInfo {
            hostname: None,
            os_name: None,
            kernel: None,
            architecture: std::env::consts::ARCH,
            cpu_model: None,
            logical_cores: None,
            physical_cores: None,
            total_ram: None,
            orbit_version: env!("CARGO_PKG_VERSION"),
        }
    }

    pub fn read_cpu_ticks() -> Option<CpuTicks> {
        None
    }

    pub fn read_memory_kib() -> Option<(u64, u64)> {
        None
    }

    pub fn read_uptime_secs() -> Option<u64> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_full_proc_stat_cpu_line() {
        let line = "cpu0 25035 118 15030 1000629 5303 0 3226 0 0 0";
        let values = parse_cpu_line(line).unwrap();
        assert_eq!(values.len(), 8);
        assert_eq!(values[0], 25_035);
        assert_eq!(values[3], 1_000_629);
        assert_eq!(values[4], 5_303);
    }

    #[test]
    fn rejects_cpu_lines_with_too_few_fields() {
        assert!(parse_cpu_line("cpu0 1 2 3").is_none());
        assert!(parse_cpu_line("cpu0").is_none());
    }

    #[test]
    fn usage_is_zero_when_only_idle_time_grows() {
        let prev = vec![vec![100, 0, 0, 900, 0, 0, 0, 0]];
        let current = vec![vec![100, 0, 0, 990, 0, 0, 0, 0]];
        let (overall, per_core) = cpu_usage_delta(&prev, &current).unwrap();
        assert_eq!(overall, 0.0);
        assert!(per_core.is_empty());
    }

    #[test]
    fn usage_is_one_hundred_when_only_busy_time_grows() {
        let prev = vec![vec![0, 0, 0, 100, 0, 0, 0, 0]];
        let current = vec![vec![50, 0, 0, 100, 0, 0, 0, 0]];
        let (overall, _) = cpu_usage_delta(&prev, &current).unwrap();
        assert!((overall - 100.0).abs() < 0.001);
    }

    #[test]
    fn usage_counts_iowait_as_idle() {
        let prev = vec![vec![0, 0, 0, 100, 50, 0, 0, 0]];
        let current = vec![vec![0, 0, 0, 100, 100, 0, 0, 0]];
        let (overall, _) = cpu_usage_delta(&prev, &current).unwrap();
        assert_eq!(overall, 0.0);
    }

    #[test]
    fn usage_is_rejected_when_tick_counts_differ() {
        let prev = vec![vec![1, 1, 1, 1, 1, 1, 1, 1]];
        let current = vec![vec![2, 2, 2, 2, 2, 2, 2, 2], vec![2, 2, 2, 2, 2, 2, 2, 2]];
        assert!(cpu_usage_delta(&prev, &current).is_none());
    }

    #[test]
    fn per_core_usage_matches_each_cpu_line() {
        let prev = vec![
            vec![0, 0, 0, 100, 0, 0, 0, 0],
            vec![0, 0, 0, 100, 0, 0, 0, 0],
            vec![0, 0, 0, 100, 0, 0, 0, 0],
        ];
        let current = vec![
            vec![10, 0, 0, 100, 0, 0, 0, 0],
            vec![0, 0, 0, 110, 0, 0, 0, 0],
            vec![5, 5, 0, 100, 0, 0, 0, 0],
        ];
        let (overall, per_core) = cpu_usage_delta(&prev, &current).unwrap();
        assert!((overall - 100.0).abs() < 0.001);
        assert_eq!(per_core.len(), 2);
        assert_eq!(per_core[0], 0.0);
        assert!((per_core[1] - 100.0).abs() < 0.001);
    }

    #[test]
    fn parses_memtotal_and_memavailable() {
        let content = "MemTotal:       16384000 kB\nMemFree:         1000000 kB\nMemAvailable:    8000000 kB\nBuffers:          200000 kB\nCached:          3000000 kB\n";
        let (total, available) = parse_meminfo(content).unwrap();
        assert_eq!(total, 16_384_000);
        assert_eq!(available, 8_000_000);
    }

    #[test]
    fn meminfo_falls_back_when_memavailable_is_missing() {
        let content = "MemTotal:       16384000 kB\nMemFree:         1000000 kB\nBuffers:          200000 kB\nCached:          3000000 kB\n";
        let (total, available) = parse_meminfo(content).unwrap();
        assert_eq!(total, 16_384_000);
        assert_eq!(available, 1_000_000 + 200_000 + 3_000_000);
    }

    #[test]
    fn meminfo_without_any_fields_is_unavailable() {
        assert!(parse_meminfo("SwapTotal: 0 kB\n").is_none());
    }

    #[test]
    fn os_release_prefers_pretty_name() {
        let content = "NAME=\"Ubuntu\"\nVERSION=\"24.04 LTS (Noble Numbat)\"\nPRETTY_NAME=\"Ubuntu 24.04.1 LTS\"\nID=ubuntu\n";
        assert_eq!(
            parse_os_release(content).as_deref(),
            Some("Ubuntu 24.04.1 LTS")
        );
    }

    #[test]
    fn os_release_falls_back_to_name_and_version() {
        let content = "NAME=\"Ubuntu\"\nVERSION=\"24.04 LTS (Noble Numbat)\"\nID=ubuntu\n";
        assert_eq!(
            parse_os_release(content).as_deref(),
            Some("Ubuntu 24.04 LTS (Noble Numbat)")
        );
    }

    #[test]
    fn os_release_unquoted_values_parse() {
        let content = "NAME=Ubuntu\nVERSION=24.04\n";
        assert_eq!(parse_os_release(content).as_deref(), Some("Ubuntu 24.04"));
    }

    #[test]
    fn os_release_missing_name_is_unavailable() {
        assert_eq!(parse_os_release("ID=ubuntu\n"), None);
    }

    #[test]
    fn parses_uptime_as_whole_seconds() {
        assert_eq!(parse_uptime("72345.11 88930.44\n"), Some(72_345));
    }

    #[test]
    fn parses_model_processor_and_core_counts() {
        let content = "processor\t: 0\nmodel name\t: Intel(R) Core(TM) i7-9750H\ncpu cores\t: 6\n\nprocessor\t: 1\nmodel name\t: Intel(R) Core(TM) i7-9750H\ncpu cores\t: 6\n";
        let (model, logical, physical) = parse_cpuinfo(content);
        assert_eq!(model.as_deref(), Some("Intel(R) Core(TM) i7-9750H"));
        assert_eq!(logical, Some(2));
        assert_eq!(physical, Some(6));
    }

    #[test]
    fn cpuinfo_prefers_model_name_over_hardware() {
        let content = "Hardware\t: SomeBoard\nmodel name\t: ARM Cortex-A72\n";
        let (model, _, _) = parse_cpuinfo(content);
        assert_eq!(model.as_deref(), Some("ARM Cortex-A72"));
    }

    #[test]
    fn cpuinfo_falls_back_to_hardware_field() {
        let content = "Hardware\t: Raspberry Pi 5\n";
        let (model, _, _) = parse_cpuinfo(content);
        assert_eq!(model.as_deref(), Some("Raspberry Pi 5"));
    }

    #[test]
    fn cpuinfo_without_processors_is_unavailable() {
        let (model, logical, _) = parse_cpuinfo("bogus : line\n");
        assert!(model.is_none());
        assert_eq!(logical, None);
    }
}
