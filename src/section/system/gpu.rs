//! GPU telemetry: backend detection, collection and parsing.
//!
//! P5.2 implements a single read-only backend for NVIDIA GPUs using
//! `nvidia-smi`, executed directly via [`std::process::Command`] with fixed
//! arguments (never through a shell, never with user input). The command
//! runs at most once per second from the System section's update loop and
//! typically completes in tens of milliseconds; its output is parsed into
//! [`GpuInfo`] and cached, so rendering only ever reads cached values.
//!
//! Systems without an NVIDIA driver get the [`GpuBackend::Unsupported`]
//! backend and every metric reads as "Unavailable" — CPU/RAM monitoring is
//! never affected. All queries are strictly read-only: no clocks, power
//! limits, fans, persistence mode or MIG settings are ever modified.

use std::process::Command;

/// One MiB in bytes (nvidia-smi reports memory in MiB with `nounits`).
const MIB: u64 = 1024 * 1024;

/// Fields requested from nvidia-smi, in fixed order.
const NVIDIA_QUERY_FIELDS: &str = "name,driver_version,memory.total,memory.used,memory.free,\
utilization.gpu,temperature.gpu,power.draw,power.limit,clocks.gr,clocks.mem,fan.speed";

/// Number of fields per row; any other count means a malformed line.
const NVIDIA_QUERY_FIELD_COUNT: usize = 12;

/// GPU backend in use. Only NVIDIA is implemented in this phase; AMD and
/// Intel are future extension points of the same collector abstraction.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GpuBackend {
    Nvidia,
    #[allow(dead_code)] // future AMD backend
    Amd,
    #[allow(dead_code)] // future Intel backend
    Intel,
    Unsupported,
}

impl GpuBackend {
    pub fn label(self) -> &'static str {
        match self {
            GpuBackend::Nvidia => "NVIDIA",
            GpuBackend::Amd => "AMD",
            GpuBackend::Intel => "Intel",
            GpuBackend::Unsupported => "Unavailable",
        }
    }
}

/// Read-only snapshot of one GPU.
///
/// Every metric is `Option<T>`: `None` means the value genuinely could not
/// be obtained and must render as "Unavailable" — it is never conflated
/// with a real 0 reading.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GpuInfo {
    pub vendor: Option<String>,
    pub name: Option<String>,
    pub driver_version: Option<String>,
    /// Bytes.
    pub memory_total: Option<u64>,
    /// Bytes.
    pub memory_used: Option<u64>,
    /// Bytes.
    pub memory_free: Option<u64>,
    /// Percent 0-100.
    pub utilization: Option<f32>,
    /// Celsius.
    pub temperature: Option<f32>,
    /// Watts.
    pub power_draw: Option<f32>,
    /// Watts.
    pub power_limit: Option<f32>,
    /// Percent 0-100.
    pub fan_speed: Option<f32>,
    /// MHz.
    pub graphics_clock: Option<u32>,
    /// MHz.
    pub memory_clock: Option<u32>,
}

impl GpuInfo {
    /// Used VRAM as a fraction of total (0.0 - 1.0), or `None` when total
    /// memory is unknown or zero (never conflated with a 0% reading).
    pub fn memory_fraction(&self) -> Option<f32> {
        let total = self.memory_total?;
        if total == 0 {
            return None;
        }
        let used = self.memory_used?;
        Some((used as f32 / total as f32).clamp(0.0, 1.0))
    }
}

/// A read-only source of GPU telemetry. Backends are cheap to call once per
/// second and never block longer than a short command.
pub trait GpuCollector {
    /// One telemetry read; returns one entry per GPU (empty when
    /// unavailable). Failures return `Vec::new()`, never panic.
    fn collect(&mut self) -> Vec<GpuInfo>;
}

/// Backend for systems without a supported GPU.
pub struct NoGpu;

impl GpuCollector for NoGpu {
    fn collect(&mut self) -> Vec<GpuInfo> {
        Vec::new()
    }
}

/// NVIDIA backend: runs `nvidia-smi` directly with fixed arguments.
pub struct NvidiaSmi;

impl NvidiaSmi {
    /// Probes the backend with one real query. `Some` only when nvidia-smi
    /// exists, succeeds and reports at least one parseable GPU row.
    pub fn detect() -> Option<NvidiaSmi> {
        let rows = run_query()?;
        if rows.is_empty() {
            None
        } else {
            Some(NvidiaSmi)
        }
    }
}

impl GpuCollector for NvidiaSmi {
    fn collect(&mut self) -> Vec<GpuInfo> {
        run_query().unwrap_or_default()
    }
}

/// Executes `nvidia-smi --query-gpu=... --format=csv,noheader,nounits`.
///
/// Fixed arguments only, executed directly (no `/bin/sh`, no shell strings,
/// no user input); stdout is captured, stderr is discarded. Any failure
/// (missing binary, driver error, permission problem, malformed output)
/// yields `None`.
fn run_query() -> Option<Vec<GpuInfo>> {
    let output = Command::new("nvidia-smi")
        .arg(format!("--query-gpu={NVIDIA_QUERY_FIELDS}"))
        .args(["--format=csv,noheader,nounits"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let stdout = String::from_utf8(output.stdout).ok()?;
    let rows = parse_gpu_csv(&stdout);
    if rows.is_empty() { None } else { Some(rows) }
}

/// Parses `nvidia-smi --format=csv,noheader,nounits` output: one GPU per
/// line. Lines that do not have exactly the fixed field count are skipped
/// entirely — a malformed row never yields a partial GPU entry.
pub fn parse_gpu_csv(content: &str) -> Vec<GpuInfo> {
    content
        .lines()
        .filter_map(|line| {
            let fields = line.split(',').map(str::trim).collect::<Vec<_>>();
            parse_gpu_row(&fields)
        })
        .collect()
}

fn parse_gpu_row(fields: &[&str]) -> Option<GpuInfo> {
    if fields.len() != NVIDIA_QUERY_FIELD_COUNT {
        return None;
    }
    let memory_mib = |value: &str| parse_number(value).map(|mib| (mib * MIB as f64).round() as u64);
    Some(GpuInfo {
        vendor: Some("NVIDIA".to_owned()),
        name: parse_value(fields[0]),
        driver_version: parse_value(fields[1]),
        memory_total: memory_mib(fields[2]),
        memory_used: memory_mib(fields[3]),
        memory_free: memory_mib(fields[4]),
        utilization: parse_number(fields[5]).map(|value| value as f32),
        temperature: parse_number(fields[6]).map(|value| value as f32),
        power_draw: parse_number(fields[7]).map(|value| value as f32),
        power_limit: parse_number(fields[8]).map(|value| value as f32),
        graphics_clock: parse_number(fields[9]).map(|value| value as u32),
        memory_clock: parse_number(fields[10]).map(|value| value as u32),
        fan_speed: parse_number(fields[11]).map(|value| value as f32),
    })
}

/// Parses a CSV value: trims whitespace and surrounding quotes, and treats
/// empty values plus `N/A` / `[N/A]` markers as unavailable.
fn parse_value(value: &str) -> Option<String> {
    let trimmed = value.trim().trim_matches('"');
    if trimmed.is_empty()
        || trimmed.eq_ignore_ascii_case("N/A")
        || trimmed.eq_ignore_ascii_case("[N/A]")
    {
        None
    } else {
        Some(trimmed.to_owned())
    }
}

/// Parses a numeric CSV value with the same rules as [`parse_value`].
fn parse_number(value: &str) -> Option<f64> {
    parse_value(value)?.parse().ok()
}

/// Cached GPU state owned by the System section.
///
/// Rendering only reads cached values; [`GpuMonitor::poll`] is called at
/// most once per second from the section's update loop.
pub struct GpuMonitor {
    backend: GpuBackend,
    collector: Box<dyn GpuCollector>,
    gpus: Vec<GpuInfo>,
}

impl GpuMonitor {
    /// Detects the best available backend with one short query. Never
    /// fails: unsupported systems simply get [`GpuBackend::Unsupported`].
    pub fn new() -> Self {
        let (backend, collector) = detect();
        Self {
            backend,
            collector,
            gpus: Vec::new(),
        }
    }

    /// Runs one telemetry read. On failure the cached list is cleared so
    /// the UI shows "Telemetry unavailable" instead of stale values.
    pub fn poll(&mut self) {
        self.gpus = self.collector.collect();
    }

    pub fn backend(&self) -> GpuBackend {
        self.backend
    }

    pub fn gpus(&self) -> &[GpuInfo] {
        &self.gpus
    }

    pub fn primary(&self) -> Option<&GpuInfo> {
        self.gpus.first()
    }
}

/// Detects the best available GPU backend. Detection runs one short query
/// and the outcome never fails startup: no NVIDIA tooling simply means the
/// `Unsupported` backend.
pub fn detect() -> (GpuBackend, Box<dyn GpuCollector>) {
    if let Some(backend) = NvidiaSmi::detect() {
        (GpuBackend::Nvidia, Box::new(backend))
    } else {
        (GpuBackend::Unsupported, Box::new(NoGpu))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Real output captured on the development machine
    /// (RTX 3050 Laptop GPU, driver 595.84): power.limit and fan.speed
    /// report `[N/A]` on this hardware.
    const REAL_ROW: &str = "NVIDIA GeForce RTX 3050 Laptop GPU, 595.84, 4096, 12, 3759, 0, 44, 6.77, [N/A], 1057, 5501, [N/A]";

    #[test]
    fn parses_a_real_nvidia_smi_row() {
        let gpus = parse_gpu_csv(REAL_ROW);
        assert_eq!(gpus.len(), 1);
        let gpu = &gpus[0];
        assert_eq!(gpu.vendor.as_deref(), Some("NVIDIA"));
        assert_eq!(
            gpu.name.as_deref(),
            Some("NVIDIA GeForce RTX 3050 Laptop GPU")
        );
        assert_eq!(gpu.driver_version.as_deref(), Some("595.84"));
        assert_eq!(gpu.memory_total, Some(4_294_967_296)); // 4096 MiB
        assert_eq!(gpu.memory_used, Some(12 * 1024 * 1024));
        assert_eq!(gpu.memory_free, Some(3759 * 1024 * 1024));
        assert_eq!(gpu.utilization, Some(0.0));
        assert_eq!(gpu.temperature, Some(44.0));
        assert_eq!(gpu.power_draw, Some(6.77));
        assert_eq!(gpu.power_limit, None); // [N/A] on this hardware
        assert_eq!(gpu.graphics_clock, Some(1057));
        assert_eq!(gpu.memory_clock, Some(5501));
        assert_eq!(gpu.fan_speed, None); // [N/A] on this hardware
    }

    #[test]
    fn parses_multiple_gpu_rows() {
        let content = format!(
            "{REAL_ROW}\nNVIDIA GeForce GTX 1080 Ti, 550.54.14, 11264, 2048, 9216, 34, 62, 88.4, 250, 1480, 5505, 42"
        );
        let gpus = parse_gpu_csv(&content);
        assert_eq!(gpus.len(), 2);
        assert_eq!(gpus[1].name.as_deref(), Some("NVIDIA GeForce GTX 1080 Ti"));
        assert_eq!(gpus[1].power_limit, Some(250.0));
        assert_eq!(gpus[1].fan_speed, Some(42.0));
        assert_eq!(gpus[1].utilization, Some(34.0));
    }

    #[test]
    fn treats_na_markers_as_unavailable() {
        for marker in ["N/A", "[N/A]", "\"[N/A]\"", ""] {
            assert_eq!(parse_number(marker), None, "marker {marker:?} must be None");
            assert_eq!(parse_value(marker), None, "marker {marker:?} must be None");
        }
    }

    #[test]
    fn parses_plain_numbers_and_whitespace() {
        assert_eq!(parse_number("  4096  "), Some(4096.0));
        assert_eq!(parse_number("6.77"), Some(6.77));
        assert_eq!(parse_number("0"), Some(0.0));
        assert_eq!(
            parse_value("\"quoted value\"").as_deref(),
            Some("quoted value")
        );
    }

    #[test]
    fn memory_is_converted_from_mib_to_bytes() {
        let gpu = &parse_gpu_csv("GPU, 1.0, 4096, 2048, 2048, 50, 60, 10, 20, 1000, 5000, 30")[0];
        assert_eq!(gpu.memory_total, Some(4_294_967_296));
        assert_eq!(gpu.memory_used, Some(2_147_483_648));
        assert_eq!(gpu.memory_free, Some(2_147_483_648));
        assert_eq!(gpu.memory_fraction(), Some(0.5));
    }

    #[test]
    fn zero_total_memory_is_handled_safely() {
        let gpu = &parse_gpu_csv("GPU, 1.0, 0, 0, 0, 50, 60, 10, 20, 1000, 5000, 30")[0];
        assert_eq!(gpu.memory_total, Some(0));
        assert_eq!(gpu.memory_fraction(), None);
    }

    #[test]
    fn missing_fields_yield_partial_metrics_but_a_valid_gpu() {
        let gpu =
            &parse_gpu_csv("GPU, 1.0, 4096, 2048, 2048, N/A, N/A, N/A, N/A, N/A, N/A, N/A")[0];
        assert_eq!(gpu.name.as_deref(), Some("GPU"));
        assert!(gpu.utilization.is_none());
        assert!(gpu.temperature.is_none());
        assert!(gpu.power_draw.is_none());
        assert!(gpu.power_limit.is_none());
        assert!(gpu.fan_speed.is_none());
        assert_eq!(gpu.memory_total, Some(4_294_967_296));
        assert_eq!(gpu.memory_fraction(), Some(0.5));
    }

    #[test]
    fn malformed_rows_are_skipped() {
        assert!(parse_gpu_csv("").is_empty());
        assert!(parse_gpu_csv("not a csv row").is_empty());
        assert!(parse_gpu_csv("only,five,fields").is_empty());
        assert!(parse_gpu_csv("a,b,c,d,e,f,g,h,i,j,k").is_empty()); // 11 fields
    }

    #[test]
    fn row_with_too_many_fields_is_skipped() {
        let content = format!("{REAL_ROW},extra\n{REAL_ROW}");
        let gpus = parse_gpu_csv(&content);
        assert_eq!(gpus.len(), 1);
    }

    #[test]
    fn garbage_numbers_are_unavailable_not_crashes() {
        let gpu =
            &parse_gpu_csv("GPU, 1.0, 4096, 12, 3759, banana, 44, 6.77, [N/A], 1057, 5501, [N/A]")
                [0];
        assert_eq!(gpu.utilization, None);
        assert_eq!(gpu.temperature, Some(44.0));
        assert_eq!(gpu.graphics_clock, Some(1057));
    }

    #[test]
    fn unsupported_backend_yields_no_telemetry() {
        let mut no_gpu = NoGpu;
        assert!(no_gpu.collect().is_empty());
        assert_eq!(GpuBackend::Unsupported.label(), "Unavailable");
        assert_eq!(GpuBackend::Nvidia.label(), "NVIDIA");
        assert_eq!(GpuBackend::Amd.label(), "AMD");
        assert_eq!(GpuBackend::Intel.label(), "Intel");
    }
}
