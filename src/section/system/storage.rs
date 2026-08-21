//! Disk and storage telemetry: mount discovery, filesystem capacity and
//! read/write throughput.
//!
//! P5.3 implements read-only storage monitoring sourced directly from Linux
//! `/proc` and `statvfs(2)`. Mount information is read from
//! `/proc/self/mounts`, virtual and pseudo-filesystems are filtered, and
//! capacity is queried via `statvfs`. Disk I/O counters are read from
//! `/proc/diskstats`, summed across whole physical disks, and converted to
//! instantaneous throughput (bytes/sec) from consecutive samples.
//!
//! On non-Linux platforms every reader returns safe stubs so the project
//! compiles. All queries are strictly read-only: no files are created,
//! modified or deleted, no processes are spawned, and no filesystem
//! metadata beyond a single `statvfs` per mount is queried.

use std::time::{Duration, Instant};

/// Bytes per sector as reported by `/proc/diskstats` (Linux standard).
const SECTOR_SIZE: u64 = 512;

/// Minimum throughput floor in bytes/sec used when the history is empty to
/// avoid division by zero in the normalised graph.
pub const THROUGHPUT_FLOOR_BPS: f32 = 1024.0;

// ---------------------------------------------------------------------------
// Data models
// ---------------------------------------------------------------------------

/// A discovered mounted filesystem with cached capacity information.
#[derive(Clone, Debug, PartialEq)]
pub struct StorageMount {
    /// Absolute mount point path (e.g. `/`, `/home`, `/boot/efi`).
    pub mount_point: String,
    /// Block device path (e.g. `/dev/nvme0n1p2`), if available.
    pub device: Option<String>,
    /// Filesystem type string from the mount table (e.g. `ext4`, `vfat`).
    pub filesystem: String,
    /// Total capacity in bytes (`f_blocks × f_frsize`), or `None` when
    /// `statvfs` failed.
    pub total_bytes: Option<u64>,
    /// Used capacity in bytes (`total - f_bavail × f_frsize`), or `None`
    /// when total or available is unknown.
    pub used_bytes: Option<u64>,
    /// Available capacity in bytes (`f_bavail × f_frsize`), or `None`
    /// when `statvfs` failed.
    pub available_bytes: Option<u64>,
}

impl StorageMount {
    /// Used space as a fraction of total (0.0 – 1.0). Returns `None` when
    /// total is unknown or zero — never conflated with a 0% reading.
    pub fn usage_fraction(&self) -> Option<f32> {
        let total = self.total_bytes?;
        if total == 0 {
            return None;
        }
        let used = self.used_bytes?;
        Some((used as f32 / total as f32).clamp(0.0, 1.0))
    }
}

/// Instantaneous disk I/O throughput derived from two `/proc/diskstats`
/// samples.
#[derive(Clone, Debug, PartialEq)]
pub struct DiskIoMetrics {
    /// Aggregate read throughput across all whole physical disks, in
    /// bytes per second, or `None` on the first sample / unavailable.
    pub read_bytes_per_sec: Option<f32>,
    /// Aggregate write throughput, in bytes per second.
    pub write_bytes_per_sec: Option<f32>,
    /// Wall-clock instant of this sample (for display ordering).
    pub timestamp: Instant,
}

/// Cumulative sector counters for a single device, extracted from two rows
/// of `/proc/diskstats`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DiskCounters {
    /// Cumulative 512-byte sectors read.
    pub read_sectors: u64,
    /// Cumulative 512-byte sectors written.
    pub write_sectors: u64,
}

// ---------------------------------------------------------------------------
// Mount parsing (pure, testable)
// ---------------------------------------------------------------------------

/// A single raw mount-table entry before capacity is queried.
#[derive(Clone, Debug, PartialEq)]
pub struct RawMount {
    pub device: String,
    pub mount_point: String,
    pub filesystem: String,
}

/// Parses `/proc/self/mounts` (or any file in the same format).
///
/// Each line has at least three whitespace-separated fields: device,
/// mount-point, filesystem.  Some mount points contain spaces encoded as
/// octal `\040` sequences (the kernel escapes them); those are decoded
/// before being returned.
pub fn parse_mounts(content: &str) -> Vec<RawMount> {
    content.lines().filter_map(parse_mount_line).collect()
}

fn parse_mount_line(line: &str) -> Option<RawMount> {
    let mut fields = line.split_whitespace();
    let device = unescape_mount(fields.next()?)?;
    let mount_point = unescape_mount(fields.next()?)?;
    let filesystem = unescape_mount(fields.next()?)?;
    Some(RawMount {
        device,
        mount_point,
        filesystem,
    })
}

/// Decodes the octal escapes used by `/proc/[self]/mounts`
/// (`\040` → space, `\011` → tab, etc.).
fn unescape_mount(field: &str) -> Option<String> {
    if !field.contains('\\') {
        return Some(field.to_owned());
    }
    let mut out = String::with_capacity(field.len());
    let mut chars = field.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        let mut value = 0u32;
        for _ in 0..3 {
            value = value * 8 + chars.next()?.to_digit(8)?;
        }
        out.push(char::from_u32(value)?);
    }
    Some(out)
}

// ---------------------------------------------------------------------------
// Filesystem classification (pure, testable)
// ---------------------------------------------------------------------------

/// Known pseudo / virtual / kernel-internal filesystem types that should
/// not appear in a storage summary.
const VIRTUAL_FILESYSTEMS: &[&str] = &[
    "autofs",
    "binfmt_misc",
    "bpf",
    "cgroup",
    "cgroup2",
    "configfs",
    "debugfs",
    "devpts",
    "devtmpfs",
    "efivarfs",
    "fusectl",
    "hugetlbfs",
    "mqueue",
    "none",
    "nsfs",
    "overlay",
    "proc",
    "procfs",
    "pstore",
    "ramfs",
    "rpc_pipefs",
    "securityfs",
    "selinuxfs",
    "squashfs",
    "sysfs",
    "tracefs",
    "tmpfs",
];

/// Returns `true` for filesystem types that represent virtual / pseudo
/// mounts and should not appear in a user-facing storage summary.
pub fn is_virtual_filesystem(fstype: &str) -> bool {
    if VIRTUAL_FILESYSTEMS.contains(&fstype) {
        return true;
    }
    // Covers userspace FUSE mounts like `fuse.portal`, `fuse.gvfsd-fuse`,
    // but deliberately excludes `fuseblk` (ntfs-3g etc.).
    fstype.starts_with("fuse.")
}

/// A mount is considered "interesting" (i.e. a real storage device) when
/// it is not a virtual filesystem AND the device path refers to an
/// actual block device under `/dev/` that is not a loop, RAM, or zram
/// device.
pub fn is_interesting_mount(raw: &RawMount) -> bool {
    if is_virtual_filesystem(&raw.filesystem) {
        return false;
    }
    let Some(rest) = raw.device.strip_prefix("/dev/") else {
        return false;
    };
    !(rest.starts_with("loop")
        || rest.starts_with("ram")
        || rest.starts_with("zram")
        || rest.starts_with("sr"))
}

/// Sorts mounts so that `/` appears first, then alphabetically by mount
/// point.
pub fn sort_mounts(mounts: &mut [StorageMount]) {
    mounts.sort_by(|a, b| {
        let a_root = a.mount_point == "/";
        let b_root = b.mount_point == "/";
        b_root
            .cmp(&a_root)
            .then_with(|| a.mount_point.cmp(&b.mount_point))
    });
}

// ---------------------------------------------------------------------------
// /proc/diskstats parsing (pure, testable)
// ---------------------------------------------------------------------------

/// Parses a single `/proc/diskstats` line.
///
/// Format (Linux ≥ 2.6):
/// ```text
/// major minor name reads_completed reads_merged sectors_read
///   ms_reading writes_completed writes_merged sectors_written
///   ms_writing ios_in_progress ms_doing_io weighted_ms_doing_io
/// ```
fn parse_diskstats_line(line: &str) -> Option<(String, DiskCounters)> {
    let fields: Vec<&str> = line.split_whitespace().collect();
    if fields.len() < 10 {
        return None;
    }
    let name = fields[2];
    let read_sectors = fields[5].parse::<u64>().ok()?;
    let write_sectors = fields[9].parse::<u64>().ok()?;
    Some((
        name.to_owned(),
        DiskCounters {
            read_sectors,
            write_sectors,
        },
    ))
}

/// Parses all parseable lines from a `/proc/diskstats` snapshot.
pub fn parse_diskstats(content: &str) -> Vec<(String, DiskCounters)> {
    content.lines().filter_map(parse_diskstats_line).collect()
}

/// Returns `true` when `name` names a whole physical disk rather than a
/// partition or virtual device.
///
/// The heuristic recognises standard Linux naming conventions:
/// - `sd[a-z]+` (SCSI/SATA/USB)
/// - `hd[a-z]+` (IDE)
/// - `vd[a-z]+` (virtio)
/// - `xvd[a-z]+` (Xen)
/// - `nvme<digits>n<digits>` (NVMe)
/// - `mmcblk<digits>` (eMMC/SD)
///
/// A device is classified as a partition when the name contains a trailing
/// digit (or `p<digit>`) after the base name, which is the standard
/// kernel partition-suffix convention.  Loop devices (`loop*`), RAM
/// disks (`ram*`, `zram*`), device-mapper (`dm-*`), software RAID
/// (`md*`), etc. are excluded outright.
pub fn is_whole_disk_name(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    if lower.starts_with("loop")
        || lower.starts_with("ram")
        || lower.starts_with("zram")
        || lower.starts_with("dm-")
        || lower.starts_with("md")
        || lower.starts_with("sr")
        || lower.starts_with("fd")
        || lower.starts_with("nbd")
        || lower.starts_with("pmem")
        || lower.starts_with("null")
    {
        return false;
    }
    for prefix in &["xvd", "sd", "hd", "vd"] {
        if let Some(rest) = lower.strip_prefix(*prefix) {
            if rest.is_empty() {
                return false;
            }
            // sd, hd, vd: partitions end with a trailing digit after
            // one or more letters.
            return rest.bytes().all(|b| b.is_ascii_lowercase());
        }
    }
    if let Some(rest) = lower.strip_prefix("nvme") {
        // nvme0n1 → whole disk;  nvme0n1p1 → partition.
        let mut parts = rest.splitn(2, 'n');
        let disk_part = match parts.next() {
            Some(p) => p,
            None => return false,
        };
        let rest_after_n = match parts.next() {
            Some(p) => p,
            None => return false,
        };
        // disk_part must be all digits; rest_after_n must be digits
        // (no trailing 'p' + digits).
        return !disk_part.is_empty()
            && disk_part.bytes().all(|b| b.is_ascii_digit())
            && !rest_after_n.is_empty()
            && rest_after_n.bytes().all(|b| b.is_ascii_digit());
    }
    if let Some(rest) = lower.strip_prefix("mmcblk") {
        // mmcblk0 → whole disk; mmcblk0p1 → partition.
        return !rest.is_empty() && rest.bytes().all(|b| b.is_ascii_digit());
    }
    false
}

/// Aggregates sector counters across all whole physical disks in a
/// `/proc/diskstats` snapshot.
pub fn sum_whole_disk_counters(content: &str) -> Option<DiskCounters> {
    let mut total = DiskCounters::default();
    let mut found = false;
    for (name, counters) in parse_diskstats(content) {
        if is_whole_disk_name(&name) {
            total.read_sectors += counters.read_sectors;
            total.write_sectors += counters.write_sectors;
            found = true;
        }
    }
    if found { Some(total) } else { None }
}

/// Converts two `DiskCounters` samples taken `elapsed` apart into
/// instantaneous throughput in bytes per second.
///
/// Counter resets (current < previous) are handled gracefully: the
/// affected counter is treated as having transferred zero bytes in this
/// interval.  Returns `None` when elapsed time is zero or non-finite.
pub fn throughput(
    prev: DiskCounters,
    current: DiskCounters,
    elapsed: Duration,
) -> Option<(f32, f32)> {
    let secs = elapsed.as_secs_f32();
    if secs <= 0.0 || !secs.is_finite() {
        return None;
    }
    let read_bytes = current
        .read_sectors
        .saturating_sub(prev.read_sectors)
        .saturating_mul(SECTOR_SIZE);
    let write_bytes = current
        .write_sectors
        .saturating_sub(prev.write_sectors)
        .saturating_mul(SECTOR_SIZE);
    Some((read_bytes as f32 / secs, write_bytes as f32 / secs))
}

// ---------------------------------------------------------------------------
// Disk I/O monitor (stateful, called once per second)
// ---------------------------------------------------------------------------

/// Cached disk I/O state owned by the [`SystemSection`](super::SystemSection).
///
/// Rendering only reads cached values; [`DiskIoMonitor::poll`] is called
/// at most once per second from the section's update loop.
pub struct DiskIoMonitor {
    prev: Option<(Instant, DiskCounters)>,
    metrics: Option<DiskIoMetrics>,
}

impl DiskIoMonitor {
    pub fn new() -> Self {
        Self {
            prev: None,
            metrics: None,
        }
    }

    /// Samples `/proc/diskstats` and updates the cached throughput metrics.
    pub fn poll(&mut self) {
        let now = Instant::now();
        let Some(counters) = imp::read_whole_disk_counters() else {
            self.metrics = None;
            return;
        };
        let rates = match &self.prev {
            None => None,
            Some((at, prev)) => throughput(*prev, counters, now.duration_since(*at)),
        };
        self.prev = Some((now, counters));
        self.metrics = Some(DiskIoMetrics {
            read_bytes_per_sec: rates.map(|r| r.0),
            write_bytes_per_sec: rates.map(|r| r.1),
            timestamp: now,
        });
    }

    /// The latest throughput snapshot, or `None` on the first sample or
    /// when `/proc/diskstats` is unavailable.
    pub fn metrics(&self) -> Option<&DiskIoMetrics> {
        self.metrics.as_ref()
    }
}

// ---------------------------------------------------------------------------
// Platform-imp: Linux real implementation
// ---------------------------------------------------------------------------

#[cfg(target_os = "linux")]
mod imp {
    use super::RawMount;
    use std::collections::HashMap;
    use std::ffi::CString;

    pub fn collect_storage_mounts() -> Vec<super::StorageMount> {
        let Ok(content) = std::fs::read_to_string("/proc/self/mounts") else {
            return Vec::new();
        };
        let raw_mounts: Vec<RawMount> = super::parse_mounts(&content);
        let mut mounts = Vec::new();
        let mut seen_devices: HashMap<String, ()> = HashMap::new();
        for raw in raw_mounts {
            if !super::is_interesting_mount(&raw) {
                continue;
            }
            if seen_devices.contains_key(&raw.device) {
                continue;
            }
            seen_devices.insert(raw.device.clone(), ());
            let (total, available) = match statvfs_totals(&raw.mount_point) {
                Some((t, a)) => (t, a),
                None => (None, None),
            };
            let used = match (total, available) {
                (Some(t), Some(a)) => Some(t.saturating_sub(a)),
                _ => None,
            };
            mounts.push(super::StorageMount {
                mount_point: raw.mount_point,
                device: Some(raw.device),
                filesystem: raw.filesystem,
                total_bytes: total,
                used_bytes: used,
                available_bytes: available,
            });
        }
        super::sort_mounts(&mut mounts);
        mounts
    }

    /// Queries `statvfs(2)` for a mount point and returns total and
    /// available (to unprivileged users) capacity in bytes.
    fn statvfs_totals(path: &str) -> Option<(Option<u64>, Option<u64>)> {
        let c_path = CString::new(path).ok()?;
        let mut stat: libc::statvfs = unsafe { std::mem::zeroed() };
        // SAFETY: c_path is a valid NUL-terminated C string; stat is a
        // stack-allocated struct writable by the kernel for the duration
        // of the syscall.
        let ret = unsafe { libc::statvfs(c_path.as_ptr(), &mut stat) };
        if ret != 0 {
            return None;
        }
        let frsize = if stat.f_frsize > 0 {
            stat.f_frsize as u64
        } else {
            stat.f_bsize as u64
        };
        if frsize == 0 {
            return None;
        }
        let total = (stat.f_blocks as u64).checked_mul(frsize);
        let available = (stat.f_bavail as u64).saturating_mul(frsize);
        Some((total, Some(available)))
    }

    pub fn read_whole_disk_counters() -> Option<super::DiskCounters> {
        let content = std::fs::read_to_string("/proc/diskstats").ok()?;
        super::sum_whole_disk_counters(&content)
    }
}

#[cfg(not(target_os = "linux"))]
mod imp {
    pub fn collect_storage_mounts() -> Vec<super::StorageMount> {
        Vec::new()
    }

    pub fn read_whole_disk_counters() -> Option<super::DiskCounters> {
        None
    }
}

/// Collects all interesting storage mounts with their capacity metrics.
pub fn collect_storage_mounts() -> Vec<StorageMount> {
    imp::collect_storage_mounts()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // ---- parse_mounts ----

    #[test]
    fn parses_normal_mount_line() {
        let line = "/dev/nvme0n1p2 / ext4 rw,relatime,stripe=128 0 0";
        let mounts = parse_mounts(line);
        assert_eq!(mounts.len(), 1);
        assert_eq!(mounts[0].device, "/dev/nvme0n1p2");
        assert_eq!(mounts[0].mount_point, "/");
        assert_eq!(mounts[0].filesystem, "ext4");
    }

    #[test]
    fn parses_vfat_boot_efi() {
        let line = "/dev/nvme0n1p1 /boot/efi vfat rw,relatime,fmask=0022,dmask=0022,codepage=437,iocharset=iso8859-1,shortname=mixed,errors=remount-ro 0 0";
        let mounts = parse_mounts(line);
        assert_eq!(mounts.len(), 1);
        assert_eq!(mounts[0].device, "/dev/nvme0n1p1");
        assert_eq!(mounts[0].mount_point, "/boot/efi");
        assert_eq!(mounts[0].filesystem, "vfat");
    }

    #[test]
    fn decodes_octal_space_escape() {
        // `\040` is octal for space (0x20).
        let line = "/dev/sda1 /media/my\\040data ext4 rw 0 0";
        let mounts = parse_mounts(line);
        assert_eq!(mounts.len(), 1);
        assert_eq!(mounts[0].mount_point, "/media/my data");
    }

    #[test]
    fn decodes_multiple_octal_escapes() {
        let line = "/dev/sda1 /mnt/a\\040b\\040c ext4 rw 0 0";
        let mounts = parse_mounts(line);
        assert_eq!(mounts[0].mount_point, "/mnt/a b c");
    }

    #[test]
    fn skips_lines_with_too_few_fields() {
        assert!(parse_mount_line("/dev/sda1 /").is_none());
        assert!(parse_mount_line("").is_none());
        assert!(parse_mount_line("only").is_none());
        assert!(parse_mount_line("one field").is_none());
    }

    #[test]
    fn parses_multiple_mount_lines() {
        let content = "/dev/nvme0n1p2 / ext4 rw 0 0\n/dev/nvme0n1p1 /boot/efi vfat rw 0 0\n";
        let mounts = parse_mounts(content);
        assert_eq!(mounts.len(), 2);
    }

    // ---- is_virtual_filesystem ----

    #[test]
    fn virtual_filesystems_are_filtered() {
        for fs in [
            "proc",
            "sysfs",
            "devtmpfs",
            "devpts",
            "tmpfs",
            "cgroup",
            "cgroup2",
            "overlay",
            "squashfs",
            "autofs",
            "efivarfs",
            "bpf",
            "configfs",
            "securityfs",
            "debugfs",
            "tracefs",
            "mqueue",
            "hugetlbfs",
            "fusectl",
            "nsfs",
            "binfmt_misc",
            "pstore",
            "ramfs",
            "none",
            "selinuxfs",
        ] {
            assert!(is_virtual_filesystem(fs), "expected virtual: {fs}");
        }
    }

    #[test]
    fn fuse_dot_prefixed_are_virtual_but_fuseblk_is_not() {
        assert!(is_virtual_filesystem("fuse.portal"));
        assert!(is_virtual_filesystem("fuse.gvfsd-fuse"));
        assert!(!is_virtual_filesystem("fuseblk"));
    }

    #[test]
    fn real_filesystems_are_not_virtual() {
        for fs in [
            "ext4", "ext3", "ext2", "btrfs", "xfs", "zfs", "ntfs", "ntfs3", "vfat", "exfat",
            "f2fs", "jfs", "reiserfs", "iso9660",
        ] {
            assert!(!is_virtual_filesystem(fs), "expected real: {fs}");
        }
    }

    // ---- is_interesting_mount ----

    #[test]
    fn ext4_on_dev_is_interesting() {
        let raw = RawMount {
            device: "/dev/nvme0n1p2".into(),
            mount_point: "/".into(),
            filesystem: "ext4".into(),
        };
        assert!(is_interesting_mount(&raw));
    }

    #[test]
    fn tmpfs_is_not_interesting() {
        let raw = RawMount {
            device: "tmpfs".into(),
            mount_point: "/run".into(),
            filesystem: "tmpfs".into(),
        };
        assert!(!is_interesting_mount(&raw));
    }

    #[test]
    fn loop_devices_are_not_interesting() {
        let raw = RawMount {
            device: "/dev/loop0".into(),
            mount_point: "/snap/core/12345".into(),
            filesystem: "squashfs".into(),
        };
        assert!(!is_interesting_mount(&raw));
    }

    #[test]
    fn ram_devices_are_not_interesting() {
        let raw = RawMount {
            device: "/dev/ram0".into(),
            mount_point: "/mnt/ram".into(),
            filesystem: "ext4".into(),
        };
        assert!(!is_interesting_mount(&raw));
    }

    #[test]
    fn non_dev_device_is_not_interesting() {
        let raw = RawMount {
            device: "host:/share".into(),
            mount_point: "/mnt/nfs".into(),
            filesystem: "nfs".into(),
        };
        assert!(!is_interesting_mount(&raw));
    }

    #[test]
    fn squashfs_on_loop_is_not_interesting() {
        let raw = RawMount {
            device: "/dev/loop9".into(),
            mount_point: "/snap/firefox/8736".into(),
            filesystem: "squashfs".into(),
        };
        assert!(!is_interesting_mount(&raw));
    }

    // ---- sort_mounts ----

    #[test]
    fn root_mount_appears_first() {
        let mut mounts = vec![
            StorageMount {
                mount_point: "/home".into(),
                device: None,
                filesystem: "ext4".into(),
                total_bytes: None,
                used_bytes: None,
                available_bytes: None,
            },
            StorageMount {
                mount_point: "/".into(),
                device: None,
                filesystem: "ext4".into(),
                total_bytes: None,
                used_bytes: None,
                available_bytes: None,
            },
            StorageMount {
                mount_point: "/data".into(),
                device: None,
                filesystem: "ext4".into(),
                total_bytes: None,
                used_bytes: None,
                available_bytes: None,
            },
        ];
        sort_mounts(&mut mounts);
        assert_eq!(mounts[0].mount_point, "/");
        assert_eq!(mounts[1].mount_point, "/data");
        assert_eq!(mounts[2].mount_point, "/home");
    }

    // ---- is_whole_disk_name ----

    #[test]
    fn whole_sda_is_detected() {
        assert!(is_whole_disk_name("sda"));
        assert!(is_whole_disk_name("sdb"));
        assert!(is_whole_disk_name("sdaa"));
    }

    #[test]
    fn sd_partitions_are_not_whole() {
        assert!(!is_whole_disk_name("sda1"));
        assert!(!is_whole_disk_name("sda2"));
        assert!(!is_whole_disk_name("sdb16"));
    }

    #[test]
    fn nvme_whole_disk_is_detected() {
        assert!(is_whole_disk_name("nvme0n1"));
        assert!(is_whole_disk_name("nvme1n1"));
        assert!(is_whole_disk_name("nvme0n2"));
    }

    #[test]
    fn nvme_partitions_are_not_whole() {
        assert!(!is_whole_disk_name("nvme0n1p1"));
        assert!(!is_whole_disk_name("nvme0n1p3"));
    }

    #[test]
    fn mmcblk_whole_disk_is_detected() {
        assert!(is_whole_disk_name("mmcblk0"));
        assert!(is_whole_disk_name("mmcblk1"));
    }

    #[test]
    fn mmcblk_partitions_are_not_whole() {
        assert!(!is_whole_disk_name("mmcblk0p1"));
    }

    #[test]
    fn vd_xvd_hd_are_detected() {
        assert!(is_whole_disk_name("vda"));
        assert!(is_whole_disk_name("xvda"));
        assert!(is_whole_disk_name("hda"));
        assert!(!is_whole_disk_name("vda1"));
        assert!(!is_whole_disk_name("xvda1"));
    }

    #[test]
    fn virtual_devices_are_not_whole() {
        assert!(!is_whole_disk_name("loop0"));
        assert!(!is_whole_disk_name("zram0"));
        assert!(!is_whole_disk_name("dm-0"));
        assert!(!is_whole_disk_name("md0"));
        assert!(!is_whole_disk_name("sr0"));
        assert!(!is_whole_disk_name("ram0"));
    }

    #[test]
    fn unknown_names_are_not_whole() {
        assert!(!is_whole_disk_name(""));
        assert!(!is_whole_disk_name("nbd0"));
        assert!(!is_whole_disk_name("pmem0"));
    }

    // ---- parse_diskstats_line ----

    #[test]
    fn parses_real_nvme_line() {
        let line = " 259       0 nvme0n1 63949 23672 7285543 13450 31831 43374 3384066 40089 0 8462 54165 0 0 0 0 2500 625";
        let (name, counters) = parse_diskstats_line(line).unwrap();
        assert_eq!(name, "nvme0n1");
        assert_eq!(counters.read_sectors, 7285543);
        assert_eq!(counters.write_sectors, 3384066);
    }

    #[test]
    fn parses_partition_line() {
        let line = " 259       2 nvme0n1p2 63341 22135 7258138 12998 31828 43374 3384064 40083 0 9405 53081 0 0 0 0 0 0";
        let (name, counters) = parse_diskstats_line(line).unwrap();
        assert_eq!(name, "nvme0n1p2");
        assert_eq!(counters.read_sectors, 7258138);
        assert_eq!(counters.write_sectors, 3384064);
    }

    #[test]
    fn parses_loop_line() {
        let line = "   7       0 loop0 50 0 1352 6 0 0 0 0 0 7 6 0 0 0 0 0 0";
        let (name, counters) = parse_diskstats_line(line).unwrap();
        assert_eq!(name, "loop0");
        assert_eq!(counters.read_sectors, 1352);
        assert_eq!(counters.write_sectors, 0);
    }

    #[test]
    fn malformed_lines_are_skipped() {
        assert!(parse_diskstats_line("").is_none());
        assert!(parse_diskstats_line("not a diskstats line").is_none());
        assert!(parse_diskstats_line("1 2 only three fields").is_none());
    }

    #[test]
    fn non_numeric_sectors_are_skipped() {
        assert!(parse_diskstats_line("  7   0 loop0 1 2 3 4 5 6 abc 8 9 10 11 12").is_none());
    }

    #[test]
    fn parses_multiple_lines() {
        let content = " 259       0 nvme0n1 63949 23672 7285543 13450 31831 43374 3384066 40089 0 8462 54165 0 0 0 0 2500 625\n   7       0 loop0 50 0 1352 6 0 0 0 0 0 7 6 0 0 0 0 0 0\n";
        let stats = parse_diskstats(content);
        assert_eq!(stats.len(), 2);
        assert_eq!(stats[0].0, "nvme0n1");
        assert_eq!(stats[1].0, "loop0");
    }

    // ---- sum_whole_disk_counters ----

    #[test]
    fn sums_only_whole_disks() {
        let content = " 259       0 nvme0n1 100 0 2000 0 100 0 2000 0 0 0 0 0\n 259       1 nvme0n1p1 50 0 500 0 20 0 200 0 0 0 0 0\n 259       2 nvme0n1p2 50 0 1500 0 80 0 1800 0 0 0 0 0\n";
        let counters = sum_whole_disk_counters(content).unwrap();
        assert_eq!(counters.read_sectors, 2000);
        assert_eq!(counters.write_sectors, 2000);
    }

    #[test]
    fn no_whole_disks_returns_none() {
        let content = "   7       0 loop0 50 0 1352 6 0 0 0 0 0 7 6 0 0 0 0 0 0\n";
        assert!(sum_whole_disk_counters(content).is_none());
    }

    #[test]
    fn empty_content_returns_none() {
        assert!(sum_whole_disk_counters("").is_none());
    }

    // ---- throughput ----

    #[test]
    fn computes_throughput_from_sector_delta() {
        let prev = DiskCounters {
            read_sectors: 1000,
            write_sectors: 500,
        };
        let current = DiskCounters {
            read_sectors: 2000,
            write_sectors: 1500,
        };
        let (read, write) = throughput(prev, current, Duration::from_secs(1)).unwrap();
        // delta = 1000 sectors * 512 = 512000 bytes, / 1 sec = 512000 B/s
        assert!((read - 512_000.0).abs() < 0.1);
        assert!((write - 512_000.0).abs() < 0.1);
    }

    #[test]
    fn handles_half_second_elapsed() {
        let prev = DiskCounters {
            read_sectors: 0,
            write_sectors: 0,
        };
        let current = DiskCounters {
            read_sectors: 1000,
            write_sectors: 500,
        };
        let (read, write) = throughput(prev, current, Duration::from_millis(500)).unwrap();
        // 1000 * 512 / 0.5 = 1024000
        assert!((read - 1_024_000.0).abs() < 0.1);
        assert!((write - 512_000.0).abs() < 0.1);
    }

    #[test]
    fn counter_reset_yields_zero_throughput() {
        let prev = DiskCounters {
            read_sectors: 5000,
            write_sectors: 3000,
        };
        let current = DiskCounters {
            read_sectors: 100,
            write_sectors: 50,
        };
        let (read, write) = throughput(prev, current, Duration::from_secs(1)).unwrap();
        // saturating_sub → 0
        assert_eq!(read, 0.0);
        assert_eq!(write, 0.0);
    }

    #[test]
    fn zero_elapsed_returns_none() {
        let prev = DiskCounters {
            read_sectors: 0,
            write_sectors: 0,
        };
        let current = DiskCounters {
            read_sectors: 100,
            write_sectors: 50,
        };
        assert!(throughput(prev, current, Duration::ZERO).is_none());
    }

    #[test]
    fn no_change_yields_zero_throughput() {
        let c = DiskCounters {
            read_sectors: 42,
            write_sectors: 10,
        };
        let (read, write) = throughput(c, c, Duration::from_secs(1)).unwrap();
        assert_eq!(read, 0.0);
        assert_eq!(write, 0.0);
    }

    // ---- StorageMount::usage_fraction ----

    #[test]
    fn usage_fraction_normal() {
        let mount = StorageMount {
            mount_point: "/".into(),
            device: Some("/dev/sda1".into()),
            filesystem: "ext4".into(),
            total_bytes: Some(100_000),
            used_bytes: Some(50_000),
            available_bytes: Some(50_000),
        };
        assert!((mount.usage_fraction().unwrap() - 0.5).abs() < 0.001);
    }

    #[test]
    fn usage_fraction_zero_total_is_none() {
        let mount = StorageMount {
            mount_point: "/".into(),
            device: None,
            filesystem: "ext4".into(),
            total_bytes: Some(0),
            used_bytes: Some(0),
            available_bytes: Some(0),
        };
        assert!(mount.usage_fraction().is_none());
    }

    #[test]
    fn usage_fraction_none_total_is_none() {
        let mount = StorageMount {
            mount_point: "/".into(),
            device: None,
            filesystem: "ext4".into(),
            total_bytes: None,
            used_bytes: None,
            available_bytes: None,
        };
        assert!(mount.usage_fraction().is_none());
    }

    // ---- DiskIoMonitor ----

    #[test]
    fn monitor_first_poll_has_no_metrics() {
        let mut monitor = DiskIoMonitor::new();
        monitor.poll();
        // First sample has no baseline → metrics should be None (or Some
        // with None rates, depending on whether /proc/diskstats is readable).
        // On non-Linux, metrics is always None.
        if let Some(m) = monitor.metrics() {
            assert!(m.read_bytes_per_sec.is_none());
            assert!(m.write_bytes_per_sec.is_none());
        }
    }

    // ---- comprehensive diskstats with extra fields ----

    #[test]
    fn parses_line_with_many_extra_fields() {
        // Kernel 5.5+ adds discard fields (indices 11-14) and flush (15).
        let line = " 259       0 nvme0n1 63949 23672 7285543 13450 31831 43374 3384066 40089 0 8462 54165 0 0 0 0 2500 625 0 0 0";
        let (name, counters) = parse_diskstats_line(line).unwrap();
        assert_eq!(name, "nvme0n1");
        assert_eq!(counters.read_sectors, 7285543);
        assert_eq!(counters.write_sectors, 3384066);
    }
}
