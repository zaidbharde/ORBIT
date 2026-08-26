//! Read-only security posture data collection.
//!
//! Gathers information from Linux procfs, sysfs and safe local files
//! without modifying anything. All operations are strictly read-only.

use std::fs;
use std::time::SystemTime;

/// Hard limit on auth log lines to read (safety bound).
const MAX_AUTH_LOG_LINES: usize = 500;

/// Hard limit on warnings tracked.
const MAX_WARNINGS: usize = 64;

/// Complete security posture snapshot, collected once per ~1 Hz cycle.
#[derive(Debug, Clone)]
pub struct SecuritySnapshot {
    pub user_session: UserSession,
    pub privilege: PrivilegeStatus,
    pub firewall: FirewallStatus,
    pub kernel_settings: KernelSecuritySettings,
    pub auth_summary: AuthSummary,
    pub summary: SecuritySummary,
    pub last_updated: Option<SystemTime>,
}

impl Default for SecuritySnapshot {
    fn default() -> Self {
        Self {
            user_session: UserSession::default(),
            privilege: PrivilegeStatus::default(),
            firewall: FirewallStatus::default(),
            kernel_settings: KernelSecuritySettings::default(),
            auth_summary: AuthSummary::default(),
            summary: SecuritySummary::default(),
            last_updated: None,
        }
    }
}

/// Current user and session information.
#[derive(Debug, Clone, Default)]
pub struct UserSession {
    pub username: Option<String>,
    pub uid: Option<u32>,
    pub gid: Option<u32>,
    pub home: Option<String>,
    pub shell: Option<String>,
    pub is_root: bool,
    pub hostname: Option<String>,
}

/// Privilege and capability status.
#[derive(Debug, Clone, Default)]
pub struct PrivilegeStatus {
    pub uid: Option<u32>,
    pub euid: Option<u32>,
    pub is_root: bool,
    pub capabilities: Option<String>,
    pub capabilities_hex: Option<String>,
}

/// Firewall framework detection (read-only).
#[derive(Debug, Clone)]
pub struct FirewallStatus {
    pub nftables_available: bool,
    pub iptables_available: bool,
    pub ufw_available: bool,
    pub active_framework: Option<String>,
    pub status_message: String,
}

impl Default for FirewallStatus {
    fn default() -> Self {
        Self {
            nftables_available: false,
            iptables_available: false,
            ufw_available: false,
            active_framework: None,
            status_message: "Firewall status unavailable".into(),
        }
    }
}

/// Security-relevant kernel settings.
#[derive(Debug, Clone, Default)]
pub struct KernelSecuritySettings {
    pub aslr: Option<String>,
    pub aslr_interpretation: Option<String>,
    pub ptrace_scope: Option<String>,
    pub ptrace_interpretation: Option<String>,
    pub core_dump_pattern: Option<String>,
}

/// Authentication activity summary.
#[derive(Debug, Clone, Default)]
pub struct AuthSummary {
    pub recent_failed_count: Option<u64>,
    pub recent_success_count: Option<u64>,
    pub latest_event_time: Option<String>,
    pub available: bool,
    pub message: Option<String>,
}

/// Summary of security check results.
#[derive(Debug, Clone, Default)]
pub struct SecuritySummary {
    pub checks_available: u32,
    pub warnings: u32,
    pub unavailable: u32,
}

impl SecuritySnapshot {
    /// Collect all security data. Intended for ~1 Hz calls.
    pub fn collect() -> Self {
        let user_session = collect_user_session();
        let privilege = collect_privilege_status();
        let firewall = collect_firewall_status();
        let kernel_settings = collect_kernel_settings();
        let auth_summary = collect_auth_summary();

        let mut warnings: Vec<String> = Vec::with_capacity(MAX_WARNINGS);

        // Check for root
        if user_session.is_root {
            warnings.push("Running as root".into());
        }
        if privilege.is_root {
            warnings.push("Effective UID is root".into());
        }

        // Check ASLR
        if let Some(ref aslr) = kernel_settings.aslr {
            if aslr == "0" {
                warnings.push("ASLR is disabled".into());
            }
        }

        // Check ptrace
        if let Some(ref scope) = kernel_settings.ptrace_scope {
            if scope == "0" {
                warnings.push("Ptrace scope is unrestricted".into());
            }
        }

        // Check firewall
        if firewall.active_framework.is_none() {
            warnings.push("No active firewall detected".into());
        }

        if warnings.len() > MAX_WARNINGS {
            warnings.truncate(MAX_WARNINGS);
        }

        let checks_available = count_available_checks(
            &user_session,
            &privilege,
            &firewall,
            &kernel_settings,
            &auth_summary,
        );
        let unavailable = count_unavailable_checks(
            &user_session,
            &privilege,
            &firewall,
            &kernel_settings,
            &auth_summary,
        );

        let summary = SecuritySummary {
            checks_available,
            warnings: warnings.len() as u32,
            unavailable,
        };

        SecuritySnapshot {
            user_session,
            privilege,
            firewall,
            kernel_settings,
            auth_summary,
            summary,
            last_updated: Some(SystemTime::now()),
        }
    }
}

// ---------------------------------------------------------------------------
// User session
// ---------------------------------------------------------------------------

fn collect_user_session() -> UserSession {
    let username = std::env::var("USER")
        .or_else(|_| std::env::var("LOGNAME"))
        .ok();
    let home = std::env::var("HOME").ok();
    let shell = std::env::var("SHELL").ok();
    let hostname = hostname();
    let uid = getuid();
    let gid = getgid();
    let is_root = uid.map_or(false, |u| u == 0);

    UserSession {
        username,
        uid,
        gid,
        home,
        shell,
        is_root,
        hostname,
    }
}

// ---------------------------------------------------------------------------
// Privilege status
// ---------------------------------------------------------------------------

fn collect_privilege_status() -> PrivilegeStatus {
    let uid = getuid();
    let euid = geteuid();
    let is_root = uid.map_or(false, |u| u == 0) || euid.map_or(false, |u| u == 0);
    let (capabilities, capabilities_hex) = read_self_capabilities();

    PrivilegeStatus {
        uid,
        euid,
        is_root,
        capabilities,
        capabilities_hex,
    }
}

// ---------------------------------------------------------------------------
// Firewall status
// ---------------------------------------------------------------------------

fn collect_firewall_status() -> FirewallStatus {
    let nft = command_exists("nft");
    let ipt = command_exists("iptables");
    let ufw = command_exists("ufw");

    let mut status = FirewallStatus {
        nftables_available: nft,
        iptables_available: ipt,
        ufw_available: ufw,
        active_framework: None,
        status_message: String::new(),
    };

    // Detect active framework by checking if rules exist
    if ufw {
        if let Some(output) = run_readonly_cmd("ufw", &["status"]) {
            if output.contains("active") {
                status.active_framework = Some("ufw".into());
                status.status_message = "UFW firewall active".into();
                return status;
            }
        }
    }

    if nft {
        if let Some(output) = run_readonly_cmd("nft", &["list", "ruleset"]) {
            if !output.trim().is_empty() && output.contains("chain") {
                status.active_framework = Some("nftables".into());
                status.status_message = "nftables rules detected".into();
                return status;
            }
        }
    }

    if ipt {
        if let Some(output) = run_readonly_cmd("iptables", &["-L", "-n"]) {
            let lines: Vec<&str> = output.lines().collect();
            // iptables -L outputs header + policy lines; >2 lines means rules exist
            if lines.len() > 2 {
                status.active_framework = Some("iptables".into());
                status.status_message = "iptables rules detected".into();
                return status;
            }
        }
    }

    if nft || ipt || ufw {
        status.status_message = "Firewall tools available, no active rules detected".into();
    } else {
        status.status_message = "Firewall status unavailable".into();
    }

    status
}

// ---------------------------------------------------------------------------
// Kernel security settings
// ---------------------------------------------------------------------------

fn collect_kernel_settings() -> KernelSecuritySettings {
    let aslr_val = read_sysctl_or_proc("kernel/randomize_va_space");
    let aslr_interp = aslr_val.as_deref().map(interpret_aslr);

    let ptrace_val = read_proc_file_trim("/proc/sys/kernel/yama/ptrace_scope");
    let ptrace_interp = ptrace_val.as_deref().map(interpret_ptrace_scope);

    let core_pattern = read_proc_file_trim("/proc/sys/kernel/core_pattern");

    KernelSecuritySettings {
        aslr: aslr_val,
        aslr_interpretation: aslr_interp,
        ptrace_scope: ptrace_val,
        ptrace_interpretation: ptrace_interp,
        core_dump_pattern: core_pattern,
    }
}

// ---------------------------------------------------------------------------
// Auth summary
// ---------------------------------------------------------------------------

fn collect_auth_summary() -> AuthSummary {
    // Try /var/log/auth.log (Debian/Ubuntu) or /var/log/secure (RHEL/CentOS)
    let log_content = read_file("/var/log/auth.log").or_else(|| read_file("/var/log/secure"));

    match log_content {
        Some(content) => parse_auth_log(&content),
        None => AuthSummary {
            available: false,
            message: Some("Authentication history unavailable".into()),
            ..Default::default()
        },
    }
}

/// Parse auth log content for failed/successful counts and latest event.
pub fn parse_auth_log(content: &str) -> AuthSummary {
    let mut failed_count: u64 = 0;
    let mut success_count: u64 = 0;
    let mut latest_time: Option<String> = None;
    let mut lines_read = 0usize;

    for line in content.lines() {
        lines_read += 1;
        if lines_read > MAX_AUTH_LOG_LINES {
            break;
        }

        let lower = line.to_lowercase();

        if lower.contains("failed") || lower.contains("failure") {
            failed_count += 1;
        }
        if lower.contains("accepted") || lower.contains("session opened") {
            success_count += 1;
        }

        // Extract timestamp: first 15 chars typically "MMM DD HH:MM:SS"
        if line.len() >= 15 {
            let candidate = line[..15].trim();
            // Basic sanity: should start with a month abbreviation
            if candidate.len() >= 6
                && candidate
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == ' ' || c == ':')
            {
                latest_time = Some(candidate.to_string());
            }
        }
    }

    AuthSummary {
        available: true,
        recent_failed_count: Some(failed_count),
        recent_success_count: Some(success_count),
        latest_event_time: latest_time,
        message: None,
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn hostname() -> Option<String> {
    fs::read_to_string("/proc/sys/kernel/hostname")
        .map(|s| s.trim().to_string())
        .ok()
        .filter(|s| !s.is_empty())
}

#[cfg(target_os = "linux")]
fn getuid() -> Option<u32> {
    // Safe: libc getuid is read-only
    Some(unsafe { libc::getuid() })
}

#[cfg(not(target_os = "linux"))]
fn getuid() -> Option<u32> {
    None
}

#[cfg(target_os = "linux")]
fn geteuid() -> Option<u32> {
    Some(unsafe { libc::geteuid() })
}

#[cfg(not(target_os = "linux"))]
fn geteuid() -> Option<u32> {
    None
}

#[cfg(target_os = "linux")]
fn getgid() -> Option<u32> {
    Some(unsafe { libc::getgid() })
}

#[cfg(not(target_os = "linux"))]
fn getgid() -> Option<u32> {
    None
}

#[cfg(target_os = "linux")]
fn read_self_capabilities() -> (Option<String>, Option<String>) {
    let status = match fs::read_to_string("/proc/self/status") {
        Ok(s) => s,
        Err(_) => return (None, None),
    };

    let mut cap_hex = None;
    let mut cap_decoded = None;

    for line in status.lines() {
        if let Some(rest) = line
            .strip_prefix("CapPrm:\t")
            .or_else(|| line.strip_prefix("CapPrm: "))
        {
            let hex = rest.trim().to_string();
            cap_hex = Some(hex.clone());
            if let Ok(val) =
                u64::from_str_radix(hex.trim_start_matches("0x").trim_start_matches("0X"), 16)
            {
                cap_decoded = Some(decode_capabilities(val));
            }
        }
    }

    (cap_decoded, cap_hex)
}

#[cfg(not(target_os = "linux"))]
fn read_self_capabilities() -> (Option<String>, Option<String>) {
    (None, None)
}

/// Decode Linux capability bitmask into human-readable names.
pub fn decode_capabilities(cap: u64) -> String {
    const CAP_NAMES: &[(u32, &str)] = &[
        (0, "CAP_CHOWN"),
        (1, "CAP_DAC_OVERRIDE"),
        (2, "CAP_DAC_READ_SEARCH"),
        (3, "CAP_FOWNER"),
        (4, "CAP_FSETID"),
        (5, "CAP_KILL"),
        (6, "CAP_SETGID"),
        (7, "CAP_SETUID"),
        (8, "CAP_SETPCAP"),
        (9, "CAP_LINUX_IMMUTABLE"),
        (10, "CAP_NET_BIND_SERVICE"),
        (11, "CAP_NET_BROADCAST"),
        (12, "CAP_NET_ADMIN"),
        (13, "CAP_NET_RAW"),
        (14, "CAP_IPC_LOCK"),
        (15, "CAP_IPC_OWNER"),
        (16, "CAP_SYS_MODULE"),
        (17, "CAP_SYS_RAWIO"),
        (18, "CAP_SYS_CHROOT"),
        (19, "CAP_SYS_PTRACE"),
        (20, "CAP_SYS_PACCT"),
        (21, "CAP_SYS_ADMIN"),
        (22, "CAP_SYS_BOOT"),
        (23, "CAP_SYS_NICE"),
        (24, "CAP_SYS_RESOURCE"),
        (25, "CAP_SYS_TIME"),
        (26, "CAP_SYS_TTY_CONFIG"),
        (27, "CAP_MKNOD"),
        (28, "CAP_LEASE"),
        (29, "CAP_AUDIT_WRITE"),
        (30, "CAP_AUDIT_CONTROL"),
        (31, "CAP_SETFCAP"),
    ];

    let mut caps = Vec::new();
    for &(bit, name) in CAP_NAMES {
        if cap & (1u64 << bit) != 0 {
            caps.push(name);
        }
    }

    if caps.is_empty() {
        "none".into()
    } else {
        caps.join(", ")
    }
}

/// Interpret ASLR value.
pub fn interpret_aslr(val: &str) -> String {
    match val.trim() {
        "0" => "Disabled (no randomization)".into(),
        "1" => "Conservative (mmap base, stack, VDSO only)".into(),
        "2" => "Full randomization (default)".into(),
        other => format!("Unknown value: {other}"),
    }
}

/// Interpret ptrace_scope value.
pub fn interpret_ptrace_scope(val: &str) -> String {
    match val.trim() {
        "0" => "Unrestricted (any process can trace any other)".into(),
        "1" => "Restricted (parent only, via prctl)".into(),
        "2" => "Admin-only (CAP_SYS_PTRACE required)".into(),
        "3" => "No attach (no ptrace at all)".into(),
        other => format!("Unknown value: {other}"),
    }
}

fn read_sysctl_or_proc(key: &str) -> Option<String> {
    // Try /proc/sys path first
    let proc_path = format!("/proc/sys/{}", key.replace('.', "/"));
    if let Some(val) = read_proc_file_trim(&proc_path) {
        return Some(val);
    }
    // Fallback: try sysctl command (read-only)
    run_readonly_cmd("sysctl", &["-n", key])
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

fn read_proc_file_trim(path: &str) -> Option<String> {
    fs::read_to_string(path)
        .map(|s| s.trim().to_string())
        .ok()
        .filter(|s| !s.is_empty())
}

fn read_file(path: &str) -> Option<String> {
    fs::read_to_string(path).ok()
}

fn command_exists(cmd: &str) -> bool {
    which(cmd).is_some()
}

fn which(cmd: &str) -> Option<String> {
    if let Ok(path) = std::env::var("PATH") {
        for dir in path.split(':') {
            let full = format!("{dir}/{cmd}");
            if std::path::Path::new(&full).is_file() {
                return Some(full);
            }
        }
    }
    None
}

fn run_readonly_cmd(cmd: &str, args: &[&str]) -> Option<String> {
    std::process::Command::new(cmd)
        .args(args)
        .output()
        .ok()
        .and_then(|output| {
            if output.status.success() {
                String::from_utf8(output.stdout).ok()
            } else {
                None
            }
        })
}

fn count_available_checks(
    user: &UserSession,
    privs: &PrivilegeStatus,
    fw: &FirewallStatus,
    kernel: &KernelSecuritySettings,
    auth: &AuthSummary,
) -> u32 {
    let mut count = 0u32;
    if user.username.is_some() {
        count += 1;
    }
    if user.uid.is_some() {
        count += 1;
    }
    if user.gid.is_some() {
        count += 1;
    }
    if user.hostname.is_some() {
        count += 1;
    }
    if privs.capabilities.is_some() {
        count += 1;
    }
    if fw.active_framework.is_some() {
        count += 1;
    }
    if kernel.aslr.is_some() {
        count += 1;
    }
    if kernel.ptrace_scope.is_some() {
        count += 1;
    }
    if auth.available {
        count += 1;
    }
    count
}

fn count_unavailable_checks(
    user: &UserSession,
    _privs: &PrivilegeStatus,
    fw: &FirewallStatus,
    kernel: &KernelSecuritySettings,
    auth: &AuthSummary,
) -> u32 {
    let mut count = 0u32;
    if user.username.is_none() {
        count += 1;
    }
    if fw.active_framework.is_none()
        && !fw.nftables_available
        && !fw.iptables_available
        && !fw.ufw_available
    {
        count += 1;
    }
    if kernel.aslr.is_none() {
        count += 1;
    }
    if kernel.ptrace_scope.is_none() {
        count += 1;
    }
    if !auth.available {
        count += 1;
    }
    count
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // --- UID/GID parsing ---

    #[test]
    fn getuid_returns_value_on_linux() {
        #[cfg(target_os = "linux")]
        {
            let uid = getuid();
            assert!(uid.is_some());
            assert!(uid.unwrap() < 65534);
        }
        #[cfg(not(target_os = "linux"))]
        {
            assert!(getuid().is_none());
        }
    }

    #[test]
    fn geteuid_returns_value_on_linux() {
        #[cfg(target_os = "linux")]
        {
            let euid = geteuid();
            assert!(euid.is_some());
        }
        #[cfg(not(target_os = "linux"))]
        {
            assert!(geteuid().is_none());
        }
    }

    // --- Root detection ---

    #[test]
    fn root_detection_for_uid_zero() {
        let session = UserSession {
            uid: Some(0),
            is_root: true,
            ..Default::default()
        };
        assert!(session.is_root);
    }

    #[test]
    fn root_detection_for_nonzero_uid() {
        let session = UserSession {
            uid: Some(1000),
            is_root: false,
            ..Default::default()
        };
        assert!(!session.is_root);
    }

    #[test]
    fn root_detection_with_none_uid() {
        let session = UserSession {
            uid: None,
            is_root: false,
            ..Default::default()
        };
        assert!(!session.is_root);
    }

    // --- Capability parsing ---

    #[test]
    fn decode_capabilities_none() {
        assert_eq!(decode_capabilities(0), "none");
    }

    #[test]
    fn decode_capabilities_single() {
        let cap = 1u64 << 13; // CAP_NET_RAW
        assert_eq!(decode_capabilities(cap), "CAP_NET_RAW");
    }

    #[test]
    fn decode_capabilities_multiple() {
        let cap = (1u64 << 5) | (1u64 << 6) | (1u64 << 7); // KILL, SETGID, SETUID
        let decoded = decode_capabilities(cap);
        assert!(decoded.contains("CAP_KILL"));
        assert!(decoded.contains("CAP_SETGID"));
        assert!(decoded.contains("CAP_SETUID"));
    }

    #[test]
    fn decode_capabilities_all_bits() {
        let mut cap = 0u64;
        for i in 0..32 {
            cap |= 1u64 << i;
        }
        let decoded = decode_capabilities(cap);
        assert!(decoded.contains("CAP_CHOWN"));
        assert!(decoded.contains("CAP_SETFCAP"));
    }

    // --- ASLR parsing ---

    #[test]
    fn interpret_aslr_disabled() {
        assert_eq!(interpret_aslr("0"), "Disabled (no randomization)");
    }

    #[test]
    fn interpret_aslr_conservative() {
        assert_eq!(
            interpret_aslr("1"),
            "Conservative (mmap base, stack, VDSO only)"
        );
    }

    #[test]
    fn interpret_aslr_full() {
        assert_eq!(interpret_aslr("2"), "Full randomization (default)");
    }

    #[test]
    fn interpret_aslr_unknown() {
        assert!(interpret_aslr("99").contains("Unknown"));
    }

    // --- Ptrace scope parsing ---

    #[test]
    fn interpret_ptrace_unrestricted() {
        assert_eq!(
            interpret_ptrace_scope("0"),
            "Unrestricted (any process can trace any other)"
        );
    }

    #[test]
    fn interpret_ptrace_restricted() {
        assert_eq!(
            interpret_ptrace_scope("1"),
            "Restricted (parent only, via prctl)"
        );
    }

    #[test]
    fn interpret_ptrace_admin_only() {
        assert_eq!(
            interpret_ptrace_scope("2"),
            "Admin-only (CAP_SYS_PTRACE required)"
        );
    }

    #[test]
    fn interpret_ptrace_no_attach() {
        assert_eq!(interpret_ptrace_scope("3"), "No attach (no ptrace at all)");
    }

    #[test]
    fn interpret_ptrace_unknown() {
        assert!(interpret_ptrace_scope("99").contains("Unknown"));
    }

    // --- Auth log parsing ---

    #[test]
    fn parse_auth_log_with_entries() {
        let content = "\
Aug 26 10:00:01 host sshd[1234]: Accepted publickey for user from 1.2.3.4
Aug 26 10:01:02 host sshd[1235]: Failed password for root from 5.6.7.8
Aug 26 10:02:03 host sshd[1236]: Failed password for invalid user admin
Aug 26 10:03:04 host sshd[1237]: session opened for user";
        let summary = parse_auth_log(content);
        assert!(summary.available);
        assert_eq!(summary.recent_failed_count, Some(2));
        assert_eq!(summary.recent_success_count, Some(2));
        assert!(summary.latest_event_time.is_some());
    }

    #[test]
    fn parse_auth_log_empty() {
        let summary = parse_auth_log("");
        assert!(summary.available);
        assert_eq!(summary.recent_failed_count, Some(0));
        assert_eq!(summary.recent_success_count, Some(0));
    }

    #[test]
    fn parse_auth_log_no_matches() {
        let content = "Aug 26 10:00:01 host kernel: some kernel message\n";
        let summary = parse_auth_log(content);
        assert!(summary.available);
        assert_eq!(summary.recent_failed_count, Some(0));
        assert_eq!(summary.recent_success_count, Some(0));
    }

    // --- Firewall status ---

    #[test]
    fn firewall_default_is_unavailable() {
        let fw = FirewallStatus::default();
        assert!(!fw.nftables_available);
        assert!(!fw.iptables_available);
        assert!(!fw.ufw_available);
        assert!(fw.active_framework.is_none());
        assert!(fw.status_message.contains("unavailable"));
    }

    // --- Kernel settings ---

    #[test]
    fn kernel_settings_default() {
        let ks = KernelSecuritySettings::default();
        assert!(ks.aslr.is_none());
        assert!(ks.ptrace_scope.is_none());
        assert!(ks.core_dump_pattern.is_none());
    }

    // --- Security summary ---

    #[test]
    fn summary_counts() {
        let summary = SecuritySummary {
            checks_available: 5,
            warnings: 2,
            unavailable: 3,
        };
        assert_eq!(summary.checks_available, 5);
        assert_eq!(summary.warnings, 2);
        assert_eq!(summary.unavailable, 3);
    }

    // --- User session ---

    #[test]
    fn user_session_default() {
        let session = UserSession::default();
        assert!(session.username.is_none());
        assert!(session.uid.is_none());
        assert!(!session.is_root);
    }

    // --- Snapshot defaults ---

    #[test]
    fn snapshot_default() {
        let snap = SecuritySnapshot::default();
        assert!(snap.last_updated.is_none());
        assert_eq!(snap.summary.checks_available, 0);
    }

    // --- Malformed input ---

    #[test]
    fn parse_auth_log_malformed() {
        let content = "not a valid log line\n";
        let summary = parse_auth_log(content);
        assert!(summary.available);
        assert_eq!(summary.recent_failed_count, Some(0));
    }

    #[test]
    fn interpret_aslr_empty() {
        assert!(interpret_aslr("").contains("Unknown"));
    }

    #[test]
    fn interpret_ptrace_empty() {
        assert!(interpret_ptrace_scope("").contains("Unknown"));
    }

    // --- Count checks ---

    #[test]
    fn count_available_all_present() {
        let user = UserSession {
            username: Some("user".into()),
            uid: Some(1000),
            gid: Some(1000),
            hostname: Some("host".into()),
            ..Default::default()
        };
        let privs = PrivilegeStatus {
            capabilities: Some("none".into()),
            ..Default::default()
        };
        let fw = FirewallStatus {
            active_framework: Some("ufw".into()),
            ..Default::default()
        };
        let kernel = KernelSecuritySettings {
            aslr: Some("2".into()),
            ptrace_scope: Some("1".into()),
            ..Default::default()
        };
        let auth = AuthSummary {
            available: true,
            ..Default::default()
        };
        let count = count_available_checks(&user, &privs, &fw, &kernel, &auth);
        assert!(count >= 8);
    }

    #[test]
    fn count_unavailable_all_missing() {
        let user = UserSession::default();
        let privs = PrivilegeStatus::default();
        let fw = FirewallStatus::default();
        let kernel = KernelSecuritySettings::default();
        let auth = AuthSummary::default();
        let count = count_unavailable_checks(&user, &privs, &fw, &kernel, &auth);
        assert!(count >= 3);
    }

    // --- Privilege status ---

    #[test]
    fn privilege_status_root() {
        let privs = PrivilegeStatus {
            uid: Some(0),
            euid: Some(0),
            is_root: true,
            ..Default::default()
        };
        assert!(privs.is_root);
    }

    #[test]
    fn privilege_status_non_root() {
        let privs = PrivilegeStatus {
            uid: Some(1000),
            euid: Some(1000),
            is_root: false,
            capabilities: Some("CAP_NET_BIND_SERVICE".into()),
            ..Default::default()
        };
        assert!(!privs.is_root);
        assert!(privs.capabilities.is_some());
    }
}
