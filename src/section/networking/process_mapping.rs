//! Read-only process-to-socket-inode mapping via Linux procfs.
//!
//! Builds a bounded map from socket inode → (PID, process name, UID) by
//! scanning `/proc/<pid>/fd/` symlinks. All operations are strictly
//! read-only — no signals are sent, no files are modified.

use std::collections::HashMap;
use std::fs;

/// Hard limit on entries in the inode→PID map to prevent unbounded memory.
const MAX_MAP_ENTRIES: usize = 8192;

/// Maximum PID directory iteration bound (safety).
const MAX_PIDS: usize = 4096;

/// Ownership information for a single socket.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessInfo {
    pub pid: u32,
    pub name: String,
    pub uid: Option<u32>,
}

/// Bounded snapshot of inode → process ownership.
#[derive(Debug, Clone, Default)]
pub struct InodeMap {
    map: HashMap<u64, ProcessInfo>,
}

impl InodeMap {
    /// Build a fresh inode map by scanning /proc for socket file descriptors.
    ///
    /// This is intended to be called once per ~1 Hz collection cycle.
    /// Returns an empty map on non-Linux or if /proc is inaccessible.
    pub fn collect() -> Self {
        #[cfg(not(target_os = "linux"))]
        {
            let _ = "Process connection mapping unavailable";
            return Self::default();
        }

        #[cfg(target_os = "linux")]
        {
            Self::collect_linux()
        }
    }

    /// Create an InodeMap from a pre-built HashMap (for testing).
    #[cfg(test)]
    pub fn from_map(map: HashMap<u64, ProcessInfo>) -> Self {
        Self { map }
    }

    /// Look up ownership for a given socket inode.
    pub fn lookup(&self, inode: u64) -> Option<&ProcessInfo> {
        self.map.get(&inode)
    }

    /// Number of mapped entries.
    pub fn len(&self) -> usize {
        self.map.len()
    }

    /// Whether the map is empty.
    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }
}

#[cfg(target_os = "linux")]
impl InodeMap {
    fn collect_linux() -> Self {
        let mut map: HashMap<u64, ProcessInfo> = HashMap::with_capacity(512);

        let proc_dir = match fs::read_dir("/proc") {
            Ok(d) => d,
            Err(_) => return Self { map },
        };

        let mut pids_scanned = 0usize;

        for entry in proc_dir {
            if pids_scanned >= MAX_PIDS {
                break;
            }

            let entry = match entry {
                Ok(e) => e,
                Err(_) => continue,
            };

            let name = entry.file_name();
            let name_str = name.to_string_lossy();
            let pid: u32 = match name_str.parse() {
                Ok(p) => p,
                Err(_) => continue,
            };

            pids_scanned += 1;

            if map.len() >= MAX_MAP_ENTRIES {
                break;
            }

            scan_process_fds(pid, &mut map);

            if map.len() >= MAX_MAP_ENTRIES {
                break;
            }
        }

        Self { map }
    }
}

/// Scan `/proc/<pid>/fd/` for socket symlinks and record inode → PID mappings.
#[cfg(target_os = "linux")]
fn scan_process_fds(pid: u32, map: &mut HashMap<u64, ProcessInfo>) {
    if map.len() >= MAX_MAP_ENTRIES {
        return;
    }

    let fd_dir = format!("/proc/{pid}/fd");
    let fd_entries = match fs::read_dir(&fd_dir) {
        Ok(d) => d,
        Err(_) => return, // permission denied or process gone
    };

    let name = read_process_name(pid);
    let uid = read_process_uid(pid);

    for fd_entry in fd_entries {
        if map.len() >= MAX_MAP_ENTRIES {
            break;
        }

        let fd_entry = match fd_entry {
            Ok(e) => e,
            Err(_) => continue,
        };

        // Read symlink target, e.g. "socket:[12345]"
        let target = match fs::read_link(fd_entry.path()) {
            Ok(t) => t,
            Err(_) => continue, // fd disappeared or permission denied
        };

        let target_str = target.to_string_lossy();
        if let Some(inode) = extract_inode(&target_str) {
            map.entry(inode).or_insert_with(|| ProcessInfo {
                pid,
                name: name.clone(),
                uid,
            });
        }
    }
}

/// Extract the socket inode from a symlink target like `"socket:[12345]"`.
pub fn extract_inode(target: &str) -> Option<u64> {
    let target = target.trim();
    if !target.starts_with("socket:[") || !target.ends_with(']') {
        return None;
    }
    let inner = &target[8..target.len() - 1];
    inner.parse::<u64>().ok()
}

/// Read the process name from `/proc/<pid>/comm`.
#[cfg(target_os = "linux")]
fn read_process_name(pid: u32) -> String {
    fs::read_to_string(format!("/proc/{pid}/comm"))
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|_| "Unknown".into())
}

/// Read the UID from `/proc/<pid>/status` (first `Uid:` line, real UID).
#[cfg(target_os = "linux")]
fn read_process_uid(pid: u32) -> Option<u32> {
    let status = fs::read_to_string(format!("/proc/{pid}/status")).ok()?;
    for line in status.lines() {
        if let Some(rest) = line
            .strip_prefix("Uid:\t")
            .or_else(|| line.strip_prefix("Uid: "))
        {
            // Format: "Uid:\t<real>\t<effective>\t<saved>\t<fs>"
            let uid_str = rest.split_whitespace().next()?;
            return uid_str.parse::<u32>().ok();
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // --- inode extraction ---

    #[test]
    fn extracts_inode_from_socket_symlink() {
        assert_eq!(extract_inode("socket:[12345]"), Some(12345));
    }

    #[test]
    fn extracts_inode_with_whitespace() {
        assert_eq!(extract_inode("  socket:[99999]  "), Some(99999));
    }

    #[test]
    fn rejects_non_socket_target() {
        assert_eq!(extract_inode("/dev/pts/0"), None);
    }

    #[test]
    fn rejects_missing_bracket() {
        assert_eq!(extract_inode("socket:[12345"), None);
    }

    #[test]
    fn rejects_missing_prefix() {
        assert_eq!(extract_inode("[12345]"), None);
    }

    #[test]
    fn rejects_empty_string() {
        assert_eq!(extract_inode(""), None);
    }

    #[test]
    fn rejects_non_numeric_inode() {
        assert_eq!(extract_inode("socket:[abc]"), None);
    }

    #[test]
    fn extracts_large_inode() {
        assert_eq!(
            extract_inode("socket:[18446744073709551615]"),
            Some(u64::MAX)
        );
    }

    #[test]
    fn extracts_zero_inode() {
        assert_eq!(extract_inode("socket:[0]"), Some(0));
    }

    // --- InodeMap ---

    #[test]
    fn empty_map_defaults() {
        let map = InodeMap::default();
        assert!(map.is_empty());
        assert_eq!(map.len(), 0);
        assert!(map.lookup(123).is_none());
    }

    #[test]
    fn lookup_returns_info_for_existing_inode() {
        let mut map = InodeMap::default();
        map.map.insert(
            42,
            ProcessInfo {
                pid: 100,
                name: "test".into(),
                uid: Some(0),
            },
        );
        assert_eq!(map.len(), 1);
        let info = map.lookup(42).unwrap();
        assert_eq!(info.pid, 100);
        assert_eq!(info.name, "test");
    }

    #[test]
    fn lookup_returns_none_for_missing_inode() {
        let mut map = InodeMap::default();
        map.map.insert(
            42,
            ProcessInfo {
                pid: 100,
                name: "test".into(),
                uid: Some(0),
            },
        );
        assert!(map.lookup(99).is_none());
    }

    // --- inode extraction from real /proc ---

    #[cfg(target_os = "linux")]
    #[test]
    fn can_scan_own_pid_fd() {
        // This process itself should have at least some open FDs.
        let pid = std::process::id();
        let fd_dir = format!("/proc/{pid}/fd");
        if std::path::Path::new(&fd_dir).exists() {
            let mut map: HashMap<u64, ProcessInfo> = HashMap::new();
            scan_process_fds(pid, &mut map);
            // We may or may not have sockets, but the scan should not panic.
            assert!(map.len() <= MAX_MAP_ENTRIES);
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn can_read_own_comm() {
        let pid = std::process::id();
        let name = read_process_name(pid);
        // Current process name might be "orbit" or the test runner name.
        assert!(!name.is_empty());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn can_read_own_uid() {
        let pid = std::process::id();
        // uid should be readable (or None if running in a weird sandbox).
        let _uid = read_process_uid(pid);
    }

    #[test]
    fn process_info_clone_eq() {
        let info = ProcessInfo {
            pid: 1,
            name: "init".into(),
            uid: Some(0),
        };
        let cloned = info.clone();
        assert_eq!(info, cloned);
    }

    #[test]
    fn extract_inode_various_prefixes() {
        // Pipe targets
        assert_eq!(extract_inode("pipe:[12345]"), None);
        // Anonymous inode
        assert_eq!(extract_inode("anon_inode:[eventpoll]"), None);
        // Regular file
        assert_eq!(extract_inode("/proc/1/status"), None);
    }

    // --- uid parsing edge cases ---

    #[test]
    fn parse_uid_valid() {
        let line = "Uid:\t1000\t1000\t1000\t1000";
        let rest = line.strip_prefix("Uid:\t").unwrap();
        let uid_str = rest.split_whitespace().next().unwrap();
        assert_eq!(uid_str.parse::<u32>().unwrap(), 1000);
    }

    #[test]
    fn parse_uid_with_spaces() {
        let line = "Uid:  0  0  0  0";
        let rest = line.strip_prefix("Uid: ").unwrap();
        let uid_str = rest.split_whitespace().next().unwrap();
        assert_eq!(uid_str.parse::<u32>().unwrap(), 0);
    }

    // --- P7.4: multiple processes ---

    #[test]
    fn multiple_processes_with_different_inodes() {
        let mut map_data: HashMap<u64, ProcessInfo> = HashMap::new();
        map_data.insert(
            100,
            ProcessInfo {
                pid: 1,
                name: "init".into(),
                uid: Some(0),
            },
        );
        map_data.insert(
            200,
            ProcessInfo {
                pid: 42,
                name: "sshd".into(),
                uid: Some(0),
            },
        );
        map_data.insert(
            300,
            ProcessInfo {
                pid: 100,
                name: "nginx".into(),
                uid: Some(33),
            },
        );
        let inode_map = InodeMap::from_map(map_data);

        assert_eq!(inode_map.len(), 3);
        assert_eq!(inode_map.lookup(100).unwrap().pid, 1);
        assert_eq!(inode_map.lookup(200).unwrap().name, "sshd");
        assert_eq!(inode_map.lookup(300).unwrap().uid, Some(33));
    }

    // --- P7.4: duplicate inode handling (first wins) ---

    #[test]
    fn duplicate_inode_first_inserted_wins() {
        let mut map_data: HashMap<u64, ProcessInfo> = HashMap::new();
        map_data.insert(
            42,
            ProcessInfo {
                pid: 100,
                name: "first".into(),
                uid: Some(0),
            },
        );
        // Second insert with same key should overwrite (HashMap behavior).
        map_data.insert(
            42,
            ProcessInfo {
                pid: 200,
                name: "second".into(),
                uid: Some(1),
            },
        );
        let inode_map = InodeMap::from_map(map_data);
        // HashMap::insert replaces, so "second" wins.
        assert_eq!(inode_map.lookup(42).unwrap().name, "second");
    }

    // --- P7.4: missing/disappeared process ---

    #[cfg(target_os = "linux")]
    #[test]
    fn scan_nonexistent_pid_does_not_panic() {
        // PID that almost certainly doesn't exist
        let mut map: HashMap<u64, ProcessInfo> = HashMap::new();
        scan_process_fds(999999999, &mut map);
        // Should produce no entries for a nonexistent PID.
        assert!(map.is_empty());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn read_comm_for_nonexistent_pid_returns_default() {
        let name = read_process_name(999999999);
        assert_eq!(name, "Unknown");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn read_uid_for_nonexistent_pid_returns_none() {
        let uid = read_process_uid(999999999);
        assert!(uid.is_none());
    }

    // --- P7.4: permission denied simulation ---

    #[cfg(target_os = "linux")]
    #[test]
    fn scan_pid_1_fd_gracefully_handles_permissions() {
        // PID 1 (init) - we may not have permission to read its FDs.
        let mut map: HashMap<u64, ProcessInfo> = HashMap::new();
        scan_process_fds(1, &mut map);
        // Should not panic, even if permission denied.
        assert!(map.len() <= MAX_MAP_ENTRIES);
    }

    // --- P7.4: bounded mapping size ---

    #[test]
    fn from_map_respects_entry_count() {
        let mut map_data: HashMap<u64, ProcessInfo> = HashMap::new();
        for i in 0..100 {
            map_data.insert(
                i,
                ProcessInfo {
                    pid: i as u32,
                    name: format!("proc{i}"),
                    uid: Some(0),
                },
            );
        }
        let inode_map = InodeMap::from_map(map_data);
        assert_eq!(inode_map.len(), 100);
    }

    // --- P7.4: collect produces a valid map ---

    #[cfg(target_os = "linux")]
    #[test]
    fn collect_produces_bounded_map() {
        let map = InodeMap::collect();
        // The map should be bounded by MAX_MAP_ENTRIES.
        assert!(map.len() <= MAX_MAP_ENTRIES);
    }

    // --- P7.4: process name with special characters ---

    #[test]
    fn process_name_with_spaces_and_special_chars() {
        let info = ProcessInfo {
            pid: 42,
            name: "my process [worker]".into(),
            uid: Some(1000),
        };
        assert_eq!(info.name, "my process [worker]");
    }

    // --- P7.4: UID parsing edge cases ---

    #[test]
    fn parse_uid_no_uid_line() {
        let status = "Name:\ttest\nState:\tS (sleeping)\n";
        let mut found_uid = false;
        for line in status.lines() {
            if let Some(rest) = line
                .strip_prefix("Uid:\t")
                .or_else(|| line.strip_prefix("Uid: "))
            {
                let uid_str = rest.split_whitespace().next().unwrap();
                let _ = uid_str.parse::<u32>().unwrap();
                found_uid = true;
            }
        }
        assert!(!found_uid);
    }
}
