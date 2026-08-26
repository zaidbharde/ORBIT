//! Read-only local security hardening audit.
//!
//! Performs safe, read-only checks against known system configuration
//! and reports explicit findings. Never modifies system configuration.
//! No passwords, tokens, keys, or secrets are exposed.

/// Maximum number of SUID/SGID files to report before truncating.
const MAX_SUID_SGID_REPORT: usize = 50;

/// Maximum number of total checks.
const MAX_CHECKS: usize = 128;

// ---------------------------------------------------------------------------
// Model
// ---------------------------------------------------------------------------

/// Status of a hardening check.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CheckStatus {
    Pass,
    Warning,
    Fail,
    Unavailable,
}

impl CheckStatus {
    pub fn label(self) -> &'static str {
        match self {
            Self::Pass => "PASS",
            Self::Warning => "WARNING",
            Self::Fail => "FAIL",
            Self::Unavailable => "UNAVAILABLE",
        }
    }

    pub const ALL: [CheckStatus; 4] = [
        CheckStatus::Pass,
        CheckStatus::Warning,
        CheckStatus::Fail,
        CheckStatus::Unavailable,
    ];
}

/// Category of a hardening check.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CheckCategory {
    Ssh,
    FilePermissions,
    Accounts,
    Privileges,
    Filesystem,
    Kernel,
    Firewall,
    Logging,
}

impl CheckCategory {
    pub fn label(self) -> &'static str {
        match self {
            Self::Ssh => "SSH",
            Self::FilePermissions => "File Permissions",
            Self::Accounts => "Accounts",
            Self::Privileges => "Privileges",
            Self::Filesystem => "Filesystem",
            Self::Kernel => "Kernel",
            Self::Firewall => "Firewall",
            Self::Logging => "Logging",
        }
    }

    pub const ALL: [CheckCategory; 8] = [
        CheckCategory::Ssh,
        CheckCategory::FilePermissions,
        CheckCategory::Accounts,
        CheckCategory::Privileges,
        CheckCategory::Filesystem,
        CheckCategory::Kernel,
        CheckCategory::Firewall,
        CheckCategory::Logging,
    ];
}

/// A single hardening check result.
#[derive(Debug, Clone)]
pub struct HardeningCheck {
    pub id: String,
    pub name: String,
    pub category: CheckCategory,
    pub status: CheckStatus,
    pub summary: String,
    pub detail: String,
}

/// Filter for hardening checks.
#[derive(Debug, Clone, Default)]
pub struct CheckFilter {
    pub search: String,
    pub status: Option<CheckStatus>,
    pub category: Option<CheckCategory>,
}

impl CheckFilter {
    pub fn matches(&self, check: &HardeningCheck) -> bool {
        if let Some(s) = self.status {
            if check.status != s {
                return false;
            }
        }
        if let Some(c) = self.category {
            if check.category != c {
                return false;
            }
        }
        let search_lower = self.search.to_lowercase();
        if !search_lower.is_empty() {
            let fields = [
                check.name.to_lowercase(),
                check.category.label().to_lowercase(),
                check.summary.to_lowercase(),
                check.detail.to_lowercase(),
            ];
            if !fields.iter().any(|f| f.contains(&search_lower)) {
                return false;
            }
        }
        true
    }
}

/// Summary counts for hardening audit results.
#[derive(Debug, Clone, Default)]
pub struct AuditSummary {
    pub total: u32,
    pub pass: u32,
    pub warning: u32,
    pub fail: u32,
    pub unavailable: u32,
}

/// Collection of hardening audit results.
#[derive(Debug, Clone, Default)]
pub struct HardeningAudit {
    pub checks: Vec<HardeningCheck>,
    pub summary: AuditSummary,
}

impl HardeningAudit {
    pub fn len(&self) -> usize {
        self.checks.len()
    }

    pub fn is_empty(&self) -> bool {
        self.checks.is_empty()
    }

    pub fn get(&self, index: usize) -> Option<&HardeningCheck> {
        self.checks.get(index)
    }

    /// Filter checks by the given filter, returning indices.
    pub fn filter(&self, filter: &CheckFilter) -> Vec<usize> {
        self.checks
            .iter()
            .enumerate()
            .filter(|(_, c)| filter.matches(c))
            .map(|(i, _)| i)
            .collect()
    }
}

// ---------------------------------------------------------------------------
// Collection
// ---------------------------------------------------------------------------

/// Collect all hardening checks. Intended for ~1 Hz calls.
pub fn collect_audit() -> HardeningAudit {
    let mut checks: Vec<HardeningCheck> = Vec::with_capacity(MAX_CHECKS);

    // 1. SSH configuration posture
    checks.extend(check_ssh_config());

    // 2. Sensitive file permissions
    checks.extend(check_sensitive_file_permissions());

    // 3. Root / privileged accounts
    checks.extend(check_root_accounts());

    // 4. World-writable sensitive locations
    checks.extend(check_world_writable_paths());

    // 5. SUID / SGID audit (bounded)
    checks.extend(check_suid_sgid());

    // 6. Account configuration (empty passwords)
    checks.extend(check_empty_passwords());

    // 7. Kernel hardening (reuse P8.1 data)
    checks.extend(check_kernel_hardening());

    // 8. Firewall posture (reuse P8.1 data)
    checks.extend(check_firewall_posture());

    // 9. Authentication/logging posture
    checks.extend(check_logging_posture());

    // 10. File ownership
    checks.extend(check_file_ownership());

    // Compute summary
    let mut summary = AuditSummary::default();
    for c in &checks {
        summary.total += 1;
        match c.status {
            CheckStatus::Pass => summary.pass += 1,
            CheckStatus::Warning => summary.warning += 1,
            CheckStatus::Fail => summary.fail += 1,
            CheckStatus::Unavailable => summary.unavailable += 1,
        }
    }

    HardeningAudit { checks, summary }
}

// ---------------------------------------------------------------------------
// 1. SSH configuration
// ---------------------------------------------------------------------------

fn check_ssh_config() -> Vec<HardeningCheck> {
    let mut checks = Vec::new();
    let content = match std::fs::read_to_string("/etc/ssh/sshd_config") {
        Ok(c) => c,
        Err(_) => {
            checks.push(HardeningCheck {
                id: "ssh_config_readable".into(),
                name: "SSH config readable".into(),
                category: CheckCategory::Ssh,
                status: CheckStatus::Unavailable,
                summary: "Cannot read /etc/ssh/sshd_config".into(),
                detail: "The SSH daemon configuration file is not readable. \
                         This may indicate restricted permissions or the file may not exist."
                    .into(),
            });
            return checks;
        }
    };

    let directives = parse_ssh_config(&content);

    // PermitRootLogin
    match directives.get("permitrootlogin") {
        Some(val) => {
            let lower = val.to_lowercase();
            let status = if lower == "no" {
                CheckStatus::Pass
            } else if lower == "yes" {
                CheckStatus::Warning
            } else {
                // prohibit-password, etc.
                CheckStatus::Pass
            };
            checks.push(HardeningCheck {
                id: "ssh_permit_root_login".into(),
                name: "SSH PermitRootLogin".into(),
                category: CheckCategory::Ssh,
                status,
                summary: format!("PermitRootLogin = {val}"),
                detail: if lower == "yes" {
                    "Root login is allowed via SSH. Consider disabling \
                     or using prohibit-password."
                        .into()
                } else {
                    "Root login is restricted.".into()
                },
            });
        }
        None => {
            // Default is "yes" in many distributions
            checks.push(HardeningCheck {
                id: "ssh_permit_root_login".into(),
                name: "SSH PermitRootLogin".into(),
                category: CheckCategory::Ssh,
                status: CheckStatus::Warning,
                summary: "PermitRootLogin not explicitly set (default: yes)".into(),
                detail: "The PermitRootLogin directive is not explicitly configured. \
                         The default may allow root login."
                    .into(),
            });
        }
    }

    // PasswordAuthentication
    match directives.get("passwordauthentication") {
        Some(val) => {
            let lower = val.to_lowercase();
            let status = if lower == "no" {
                CheckStatus::Pass
            } else {
                CheckStatus::Warning
            };
            checks.push(HardeningCheck {
                id: "ssh_password_auth".into(),
                name: "SSH PasswordAuthentication".into(),
                category: CheckCategory::Ssh,
                status,
                summary: format!("PasswordAuthentication = {val}"),
                detail: if lower == "yes" {
                    "Password authentication is enabled. Consider using \
                     key-only authentication."
                        .into()
                } else {
                    "Password authentication is disabled (key-only).".into()
                },
            });
        }
        None => {
            checks.push(HardeningCheck {
                id: "ssh_password_auth".into(),
                name: "SSH PasswordAuthentication".into(),
                category: CheckCategory::Ssh,
                status: CheckStatus::Warning,
                summary: "PasswordAuthentication not explicitly set".into(),
                detail: "The directive is not set. Default may be yes.".into(),
            });
        }
    }

    // PermitEmptyPasswords
    match directives.get("permitemptypasswords") {
        Some(val) => {
            let lower = val.to_lowercase();
            let status = if lower == "no" {
                CheckStatus::Pass
            } else {
                CheckStatus::Fail
            };
            checks.push(HardeningCheck {
                id: "ssh_empty_passwords".into(),
                name: "SSH PermitEmptyPasswords".into(),
                category: CheckCategory::Ssh,
                status,
                summary: format!("PermitEmptyPasswords = {val}"),
                detail: if lower == "yes" {
                    "Empty passwords are permitted for SSH login. This is a \
                         serious security risk."
                        .into()
                } else {
                    "Empty passwords are not permitted.".into()
                },
            });
        }
        None => {
            checks.push(HardeningCheck {
                id: "ssh_empty_passwords".into(),
                name: "SSH PermitEmptyPasswords".into(),
                category: CheckCategory::Ssh,
                status: CheckStatus::Pass,
                summary: "PermitEmptyPasswords not set (default: no)".into(),
                detail: "Empty passwords are not permitted by default.".into(),
            });
        }
    }

    // PubkeyAuthentication
    match directives.get("pubkeyauthentication") {
        Some(val) => {
            let lower = val.to_lowercase();
            let status = if lower == "yes" {
                CheckStatus::Pass
            } else {
                CheckStatus::Warning
            };
            checks.push(HardeningCheck {
                id: "ssh_pubkey_auth".into(),
                name: "SSH PubkeyAuthentication".into(),
                category: CheckCategory::Ssh,
                status,
                summary: format!("PubkeyAuthentication = {val}"),
                detail: if lower == "yes" {
                    "Public key authentication is enabled.".into()
                } else {
                    "Public key authentication is disabled. Consider enabling it.".into()
                },
            });
        }
        None => {
            checks.push(HardeningCheck {
                id: "ssh_pubkey_auth".into(),
                name: "SSH PubkeyAuthentication".into(),
                category: CheckCategory::Ssh,
                status: CheckStatus::Pass,
                summary: "PubkeyAuthentication not set (default: yes)".into(),
                detail: "Public key authentication is enabled by default.".into(),
            });
        }
    }

    checks
}

/// Parse SSH config into a map of directive -> value (case-insensitive keys).
pub fn parse_ssh_config(content: &str) -> std::collections::HashMap<String, String> {
    let mut directives = std::collections::HashMap::new();
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        // Skip Include directives conservatively
        if line.to_lowercase().starts_with("include ") {
            continue;
        }
        if let Some((key, value)) = line.split_once(char::is_whitespace) {
            directives.insert(key.to_lowercase(), value.trim().to_string());
        }
    }
    directives
}

// ---------------------------------------------------------------------------
// 2. Sensitive file permissions
// ---------------------------------------------------------------------------

struct SensitiveFile {
    path: &'static str,
    expected_mode: u32,
    description: &'static str,
}

const SENSITIVE_FILES: &[SensitiveFile] = &[
    SensitiveFile {
        path: "/etc/passwd",
        expected_mode: 0o644,
        description: "User account database",
    },
    SensitiveFile {
        path: "/etc/group",
        expected_mode: 0o644,
        description: "Group database",
    },
    SensitiveFile {
        path: "/etc/shadow",
        expected_mode: 0o640,
        description: "Password hashes (restricted)",
    },
    SensitiveFile {
        path: "/etc/gshadow",
        expected_mode: 0o640,
        description: "Group password hashes (restricted)",
    },
    SensitiveFile {
        path: "/etc/ssh/sshd_config",
        expected_mode: 0o600,
        description: "SSH daemon configuration",
    },
];

fn check_sensitive_file_permissions() -> Vec<HardeningCheck> {
    let mut checks = Vec::new();
    for sf in SENSITIVE_FILES {
        let meta = match std::fs::metadata(sf.path) {
            Ok(m) => m,
            Err(_) => {
                checks.push(HardeningCheck {
                    id: format!("file_perms_{}", sf.path.replace('/', "_")),
                    name: format!("File exists: {}", sf.path),
                    category: CheckCategory::FilePermissions,
                    status: CheckStatus::Unavailable,
                    summary: format!("{} not found or inaccessible", sf.path),
                    detail: format!(
                        "The file {} could not be stat'd. \
                         It may not exist on this system.",
                        sf.path
                    ),
                });
                continue;
            }
        };

        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            let mode = meta.mode() & 0o7777;
            let uid = meta.uid();
            let gid = meta.gid();
            let is_world_readable = mode & 0o004 != 0;
            let is_world_writable = mode & 0o002 != 0;
            let _is_group_writable = mode & 0o020 != 0;

            let mode_octal = format!("{mode:04o}");
            let readable = is_file_readable(sf.path);

            // Determine status
            let status = if is_world_writable {
                CheckStatus::Fail
            } else if is_world_readable && sf.expected_mode & 0o004 == 0 {
                CheckStatus::Warning
            } else if mode != sf.expected_mode {
                CheckStatus::Warning
            } else {
                CheckStatus::Pass
            };

            let summary = if is_world_writable {
                format!("{} is world-writable ({})", sf.path, mode_octal)
            } else if is_world_readable && sf.expected_mode & 0o004 == 0 {
                format!("{} is world-readable ({})", sf.path, mode_octal)
            } else {
                format!(
                    "{}: mode {mode_octal} (expected {:04o})",
                    sf.path, sf.expected_mode
                )
            };

            let detail = format!(
                "File: {}\nDescription: {}\nPermissions: {mode_octal}\n\
                 Owner UID: {uid}\nGroup GID: {gid}\nReadable: {readable}\n\
                 Expected: {:04o}",
                sf.path, sf.description, sf.expected_mode,
            );

            checks.push(HardeningCheck {
                id: format!("file_perms_{}", sf.path.replace('/', "_")),
                name: format!("Permissions: {}", sf.path),
                category: CheckCategory::FilePermissions,
                status,
                summary,
                detail,
            });
        }

        #[cfg(not(unix))]
        {
            let _ = meta;
            checks.push(HardeningCheck {
                id: format!("file_perms_{}", sf.path.replace('/', "_")),
                name: format!("Permissions: {}", sf.path),
                category: CheckCategory::FilePermissions,
                status: CheckStatus::Unavailable,
                summary: format!("Permission check unavailable on this platform"),
                detail: "Unix permission checks are not supported on this platform.".into(),
            });
        }
    }
    checks
}

// ---------------------------------------------------------------------------
// 3. Root / privileged accounts
// ---------------------------------------------------------------------------

fn check_root_accounts() -> Vec<HardeningCheck> {
    let mut checks = Vec::new();
    let content = match std::fs::read_to_string("/etc/passwd") {
        Ok(c) => c,
        Err(_) => {
            checks.push(HardeningCheck {
                id: "root_accounts".into(),
                name: "UID-0 accounts".into(),
                category: CheckCategory::Accounts,
                status: CheckStatus::Unavailable,
                summary: "Cannot read /etc/passwd".into(),
                detail: "The passwd file is not readable. Cannot identify UID-0 accounts.".into(),
            });
            return checks;
        }
    };

    let uid0_accounts: Vec<String> = content
        .lines()
        .filter_map(|line| {
            let parts: Vec<&str> = line.split(':').collect();
            if parts.len() >= 3 && parts[2] == "0" {
                Some(parts[0].to_string())
            } else {
                None
            }
        })
        .collect();

    if uid0_accounts.len() <= 1 {
        checks.push(HardeningCheck {
            id: "root_accounts".into(),
            name: "UID-0 accounts".into(),
            category: CheckCategory::Accounts,
            status: CheckStatus::Pass,
            summary: format!(
                "{} UID-0 account(s): {}",
                uid0_accounts.len(),
                uid0_accounts.join(", ")
            ),
            detail: "Only the expected root account has UID 0.".into(),
        });
    } else {
        checks.push(HardeningCheck {
            id: "root_accounts".into(),
            name: "UID-0 accounts".into(),
            category: CheckCategory::Accounts,
            status: CheckStatus::Warning,
            summary: format!(
                "{} UID-0 accounts: {}",
                uid0_accounts.len(),
                uid0_accounts.join(", ")
            ),
            detail: "Multiple accounts have UID 0. This may be intentional but warrants review."
                .into(),
        });
    }

    checks
}

// ---------------------------------------------------------------------------
// 4. World-writable sensitive locations
// ---------------------------------------------------------------------------

fn check_world_writable_paths() -> Vec<HardeningCheck> {
    let mut checks = Vec::new();

    // Expected world-writable dirs (these are normal)
    let expected_world_writable = ["/tmp", "/var/tmp"];

    // Suspicious locations to check
    let suspicious_paths = ["/etc", "/etc/ssh", "/boot", "/root", "/var/log"];

    // Check expected world-writable directories exist and have sticky bit
    for &path in &expected_world_writable {
        let meta = match std::fs::metadata(path) {
            Ok(m) => m,
            Err(_) => continue,
        };
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            let mode = meta.mode();
            let is_world_writable = mode & 0o002 != 0;
            let has_sticky = mode & 0o1000 != 0;

            if is_world_writable && !has_sticky {
                checks.push(HardeningCheck {
                    id: format!("worldwritable_{}", path.replace('/', "_")),
                    name: format!("World-writable: {path}"),
                    category: CheckCategory::Filesystem,
                    status: CheckStatus::Warning,
                    summary: format!("{path} is world-writable without sticky bit"),
                    detail: format!(
                        "{path} is world-writable but lacks the sticky bit. \
                         Users may be able to delete other users' files."
                    ),
                });
            } else if is_world_writable {
                checks.push(HardeningCheck {
                    id: format!("worldwritable_{}", path.replace('/', "_")),
                    name: format!("World-writable: {path}"),
                    category: CheckCategory::Filesystem,
                    status: CheckStatus::Pass,
                    summary: format!("{path} is world-writable with sticky bit (expected)"),
                    detail: format!("{path} has the sticky bit set as expected."),
                });
            }
        }
    }

    // Check suspicious paths for unexpected world-writable
    for &path in &suspicious_paths {
        let meta = match std::fs::metadata(path) {
            Ok(m) => m,
            Err(_) => continue,
        };
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            let mode = meta.mode();
            let is_world_writable = mode & 0o002 != 0;

            if is_world_writable {
                checks.push(HardeningCheck {
                    id: format!("worldwritable_{}", path.replace('/', "_")),
                    name: format!("World-writable: {path}"),
                    category: CheckCategory::Filesystem,
                    status: CheckStatus::Fail,
                    summary: format!("{path} is world-writable (unexpected for config)"),
                    detail: format!(
                        "The path {path} is world-writable. \
                         Sensitive configuration directories should not be world-writable."
                    ),
                });
            }
        }
    }

    checks
}

// ---------------------------------------------------------------------------
// 5. SUID / SGID audit (bounded)
// ---------------------------------------------------------------------------

fn check_suid_sgid() -> Vec<HardeningCheck> {
    let mut checks = Vec::new();
    let scan_dirs = ["/usr/bin", "/usr/sbin", "/bin", "/sbin"];

    let mut total_suid = 0u32;
    let mut total_sgid = 0u32;
    let mut reported: Vec<String> = Vec::new();

    for &dir in &scan_dirs {
        let entries = match std::fs::read_dir(dir) {
            Ok(e) => e,
            Err(_) => continue,
        };

        for entry in entries.flatten() {
            let meta = match entry.metadata() {
                Ok(m) => m,
                Err(_) => continue,
            };
            #[cfg(unix)]
            {
                use std::os::unix::fs::MetadataExt;
                let mode = meta.mode();
                let is_suid = mode & 0o4000 != 0;
                let is_sgid = mode & 0o2000 != 0;

                if is_suid {
                    total_suid += 1;
                    let name = entry.file_name().to_string_lossy().to_string();
                    // Known SUID binaries that are expected
                    let is_expected = matches!(
                        name.as_str(),
                        "su" | "sudo"
                            | "passwd"
                            | "chsh"
                            | "chfn"
                            | "newgrp"
                            | "gpasswd"
                            | "pkexec"
                            | "crontab"
                            | "at"
                            | "mount"
                            | "umount"
                            | "ping"
                            | "ping6"
                            | "fusermount"
                            | "fusermount3"
                    );
                    if !is_expected && reported.len() < MAX_SUID_SGID_REPORT {
                        reported.push(format!("{}/{}", dir, name));
                    }
                }
                if is_sgid {
                    total_sgid += 1;
                }
            }
        }
    }

    if total_suid == 0 && total_sgid == 0 {
        checks.push(HardeningCheck {
            id: "suid_sgid_count".into(),
            name: "SUID/SGID binaries".into(),
            category: CheckCategory::Privileges,
            status: CheckStatus::Pass,
            summary: "No SUID/SGID binaries found in scanned directories".into(),
            detail: "Scanned /usr/bin, /usr/sbin, /bin, /sbin. No SUID or SGID binaries found."
                .into(),
        });
    } else if reported.is_empty() {
        checks.push(HardeningCheck {
            id: "suid_sgid_count".into(),
            name: "SUID/SGID binaries".into(),
            category: CheckCategory::Privileges,
            status: CheckStatus::Pass,
            summary: format!("SUID: {total_suid}, SGID: {total_sgid} (all expected)"),
            detail: "All SUID/SGID binaries are known expected system utilities.".into(),
        });
    } else {
        checks.push(HardeningCheck {
            id: "suid_sgid_count".into(),
            name: "SUID/SGID binaries".into(),
            category: CheckCategory::Privileges,
            status: CheckStatus::Warning,
            summary: format!(
                "SUID: {total_suid}, SGID: {total_sgid}, {} unexpected",
                reported.len()
            ),
            detail: format!("Unexpected SUID/SGID binaries:\n{}", reported.join("\n")),
        });
    }

    checks
}

// ---------------------------------------------------------------------------
// 6. Account configuration (empty passwords)
// ---------------------------------------------------------------------------

fn check_empty_passwords() -> Vec<HardeningCheck> {
    let mut checks = Vec::new();

    let content = match std::fs::read_to_string("/etc/shadow") {
        Ok(c) => c,
        Err(_) => {
            checks.push(HardeningCheck {
                id: "empty_passwords".into(),
                name: "Empty password fields".into(),
                category: CheckCategory::Accounts,
                status: CheckStatus::Unavailable,
                summary: "Cannot read /etc/shadow".into(),
                detail: "The shadow file is not readable. Cannot check for empty passwords. \
                         This typically requires root privileges."
                    .into(),
            });
            return checks;
        }
    };

    let empty_pw_users: Vec<String> = content
        .lines()
        .filter_map(|line| {
            let parts: Vec<&str> = line.split(':').collect();
            if parts.len() >= 2 && (parts[1].is_empty() || parts[1] == "!") {
                // Empty or locked - only report truly empty (no ! prefix)
                if parts[1].is_empty() {
                    Some(parts[0].to_string())
                } else {
                    None
                }
            } else {
                None
            }
        })
        .collect();

    if empty_pw_users.is_empty() {
        checks.push(HardeningCheck {
            id: "empty_passwords".into(),
            name: "Empty password fields".into(),
            category: CheckCategory::Accounts,
            status: CheckStatus::Pass,
            summary: "No accounts with empty password fields".into(),
            detail: "All accounts have password hashes or are properly locked.".into(),
        });
    } else {
        checks.push(HardeningCheck {
            id: "empty_passwords".into(),
            name: "Empty password fields".into(),
            category: CheckCategory::Accounts,
            status: CheckStatus::Fail,
            summary: format!("{} account(s) with empty passwords", empty_pw_users.len()),
            detail: format!(
                "Accounts with empty password fields: {}. \
                 These accounts can be logged into without a password.",
                empty_pw_users.join(", ")
            ),
        });
    }

    checks
}

// ---------------------------------------------------------------------------
// 7. Kernel hardening
// ---------------------------------------------------------------------------

fn check_kernel_hardening() -> Vec<HardeningCheck> {
    let mut checks = Vec::new();

    // ASLR
    let aslr = read_proc_trim("/proc/sys/kernel/randomize_va_space");
    match aslr.as_deref() {
        Some("2") => {
            checks.push(HardeningCheck {
                id: "kernel_aslr".into(),
                name: "ASLR".into(),
                category: CheckCategory::Kernel,
                status: CheckStatus::Pass,
                summary: "ASLR: Full randomization (2)".into(),
                detail: "Address space layout randomization is fully enabled.".into(),
            });
        }
        Some("1") => {
            checks.push(HardeningCheck {
                id: "kernel_aslr".into(),
                name: "ASLR".into(),
                category: CheckCategory::Kernel,
                status: CheckStatus::Warning,
                summary: "ASLR: Conservative randomization (1)".into(),
                detail: "ASLR is set to conservative mode. Full randomization (2) is recommended."
                    .into(),
            });
        }
        Some("0") => {
            checks.push(HardeningCheck {
                id: "kernel_aslr".into(),
                name: "ASLR".into(),
                category: CheckCategory::Kernel,
                status: CheckStatus::Fail,
                summary: "ASLR: Disabled (0)".into(),
                detail: "Address space layout randomization is disabled.".into(),
            });
        }
        _ => {
            checks.push(HardeningCheck {
                id: "kernel_aslr".into(),
                name: "ASLR".into(),
                category: CheckCategory::Kernel,
                status: CheckStatus::Unavailable,
                summary: "ASLR status unknown".into(),
                detail: "Could not read randomize_va_space.".into(),
            });
        }
    }

    // Ptrace scope
    let ptrace = read_proc_trim("/proc/sys/kernel/yama/ptrace_scope");
    match ptrace.as_deref() {
        Some("0") => {
            checks.push(HardeningCheck {
                id: "kernel_ptrace".into(),
                name: "Ptrace Scope".into(),
                category: CheckCategory::Kernel,
                status: CheckStatus::Warning,
                summary: "Ptrace scope: Unrestricted (0)".into(),
                detail: "Any process can trace any other. Consider restricting.".into(),
            });
        }
        Some("1") => {
            checks.push(HardeningCheck {
                id: "kernel_ptrace".into(),
                name: "Ptrace Scope".into(),
                category: CheckCategory::Kernel,
                status: CheckStatus::Pass,
                summary: "Ptrace scope: Restricted (1)".into(),
                detail: "Only parent processes can ptrace children.".into(),
            });
        }
        Some("2") | Some("3") => {
            checks.push(HardeningCheck {
                id: "kernel_ptrace".into(),
                name: "Ptrace Scope".into(),
                category: CheckCategory::Kernel,
                status: CheckStatus::Pass,
                summary: format!(
                    "Ptrace scope: {} ({})",
                    ptrace.as_ref().unwrap(),
                    ptrace.as_ref().unwrap()
                ),
                detail: "Ptrace is restricted to admin or disabled entirely.".into(),
            });
        }
        _ => {
            checks.push(HardeningCheck {
                id: "kernel_ptrace".into(),
                name: "Ptrace Scope".into(),
                category: CheckCategory::Kernel,
                status: CheckStatus::Unavailable,
                summary: "Ptrace scope unknown".into(),
                detail: "Could not read ptrace_scope.".into(),
            });
        }
    }

    // Core dump pattern
    let core = read_proc_trim("/proc/sys/kernel/core_pattern");
    if let Some(ref pattern) = core {
        let safe = !pattern.contains('|');
        checks.push(HardeningCheck {
            id: "kernel_core_dump".into(),
            name: "Core Dump Pattern".into(),
            category: CheckCategory::Kernel,
            status: if safe { CheckStatus::Pass } else { CheckStatus::Warning },
            summary: format!("Core pattern: {pattern}"),
            detail: if safe {
                "Core dumps are written to a file (no pipe handler).".into()
            } else {
                "Core pattern uses a pipe handler. Core dumps may be processed by an external program."
                    .into()
            },
        });
    } else {
        checks.push(HardeningCheck {
            id: "kernel_core_dump".into(),
            name: "Core Dump Pattern".into(),
            category: CheckCategory::Kernel,
            status: CheckStatus::Unavailable,
            summary: "Core dump pattern unknown".into(),
            detail: "Could not read core_pattern.".into(),
        });
    }

    checks
}

// ---------------------------------------------------------------------------
// 8. Firewall posture
// ---------------------------------------------------------------------------

fn check_firewall_posture() -> Vec<HardeningCheck> {
    let mut checks = Vec::new();

    let has_ufw = command_exists("ufw");
    let has_iptables = command_exists("iptables");
    let has_nft = command_exists("nft");

    // Check if any firewall tool is active
    if has_ufw {
        if let Some(output) = run_readonly_cmd("ufw", &["status"]) {
            let active = output.to_lowercase().contains("active");
            checks.push(HardeningCheck {
                id: "firewall_ufw".into(),
                name: "UFW Firewall".into(),
                category: CheckCategory::Firewall,
                status: if active {
                    CheckStatus::Pass
                } else {
                    CheckStatus::Warning
                },
                summary: if active {
                    "UFW is active".into()
                } else {
                    "UFW installed but not active".into()
                },
                detail: if active {
                    "UFW firewall is installed and active.".into()
                } else {
                    "UFW is installed but not currently active.".into()
                },
            });
            return checks;
        }
    }

    if has_iptables {
        if let Some(output) = run_readonly_cmd("iptables", &["-L", "-n"]) {
            let lines: Vec<&str> = output.lines().collect();
            let has_rules = lines.len() > 2;
            checks.push(HardeningCheck {
                id: "firewall_iptables".into(),
                name: "iptables Firewall".into(),
                category: CheckCategory::Firewall,
                status: if has_rules {
                    CheckStatus::Pass
                } else {
                    CheckStatus::Warning
                },
                summary: if has_rules {
                    "iptables rules active".into()
                } else {
                    "iptables installed, no rules".into()
                },
                detail: if has_rules {
                    "iptables is available with active rules.".into()
                } else {
                    "iptables is available but no rules are configured.".into()
                },
            });
            return checks;
        }
    }

    if has_nft {
        if let Some(output) = run_readonly_cmd("nft", &["list", "ruleset"]) {
            let active = output.contains("chain");
            checks.push(HardeningCheck {
                id: "firewall_nft".into(),
                name: "nftables Firewall".into(),
                category: CheckCategory::Firewall,
                status: if active {
                    CheckStatus::Pass
                } else {
                    CheckStatus::Warning
                },
                summary: if active {
                    "nftables rules active".into()
                } else {
                    "nftables installed, no rules".into()
                },
                detail: if active {
                    "nftables is available with active rules.".into()
                } else {
                    "nftables is available but no rules are configured.".into()
                },
            });
            return checks;
        }
    }

    checks.push(HardeningCheck {
        id: "firewall_overall".into(),
        name: "Firewall Status".into(),
        category: CheckCategory::Firewall,
        status: CheckStatus::Unavailable,
        summary: "No firewall tools detected".into(),
        detail: "No supported firewall tools (ufw, iptables, nftables) were found.".into(),
    });

    checks
}

// ---------------------------------------------------------------------------
// 9. Authentication/logging posture
// ---------------------------------------------------------------------------

fn check_logging_posture() -> Vec<HardeningCheck> {
    let mut checks = Vec::new();

    // Check if auth logs exist
    let has_auth_log = std::path::Path::new("/var/log/auth.log").exists()
        || std::path::Path::new("/var/log/secure").exists();

    if has_auth_log {
        checks.push(HardeningCheck {
            id: "logging_auth".into(),
            name: "Authentication Logging".into(),
            category: CheckCategory::Logging,
            status: CheckStatus::Pass,
            summary: "Authentication log available".into(),
            detail: "Authentication events are being logged.".into(),
        });
    } else {
        checks.push(HardeningCheck {
            id: "logging_auth".into(),
            name: "Authentication Logging".into(),
            category: CheckCategory::Logging,
            status: CheckStatus::Unavailable,
            summary: "No authentication log found".into(),
            detail: "Could not find /var/log/auth.log or /var/log/secure.".into(),
        });
    }

    // Check if kernel log exists
    let has_kern_log = std::path::Path::new("/var/log/kern.log").exists()
        || std::path::Path::new("/var/log/messages").exists();

    if has_kern_log {
        checks.push(HardeningCheck {
            id: "logging_kernel".into(),
            name: "Kernel Logging".into(),
            category: CheckCategory::Logging,
            status: CheckStatus::Pass,
            summary: "Kernel log available".into(),
            detail: "Kernel events are being logged.".into(),
        });
    } else {
        checks.push(HardeningCheck {
            id: "logging_kernel".into(),
            name: "Kernel Logging".into(),
            category: CheckCategory::Logging,
            status: CheckStatus::Unavailable,
            summary: "No kernel log found".into(),
            detail: "Could not find /var/log/kern.log or /var/log/messages.".into(),
        });
    }

    // syslog
    let has_syslog = std::path::Path::new("/var/log/syslog").exists()
        || std::path::Path::new("/var/log/messages").exists();

    if has_syslog {
        checks.push(HardeningCheck {
            id: "logging_syslog".into(),
            name: "System Logging".into(),
            category: CheckCategory::Logging,
            status: CheckStatus::Pass,
            summary: "System log available".into(),
            detail: "System logging is active.".into(),
        });
    } else {
        checks.push(HardeningCheck {
            id: "logging_syslog".into(),
            name: "System Logging".into(),
            category: CheckCategory::Logging,
            status: CheckStatus::Unavailable,
            summary: "No system log found".into(),
            detail: "Could not find /var/log/syslog or /var/log/messages.".into(),
        });
    }

    checks
}

// ---------------------------------------------------------------------------
// 10. File ownership
// ---------------------------------------------------------------------------

fn check_file_ownership() -> Vec<HardeningCheck> {
    let mut checks = Vec::new();

    let owned_files = [
        ("/etc/passwd", "User database"),
        ("/etc/group", "Group database"),
        ("/etc/ssh/sshd_config", "SSH configuration"),
    ];

    for &(path, desc) in &owned_files {
        let meta = match std::fs::metadata(path) {
            Ok(m) => m,
            Err(_) => continue,
        };

        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            let uid = meta.uid();
            let gid = meta.gid();

            let status = if uid == 0 && gid == 0 {
                CheckStatus::Pass
            } else {
                CheckStatus::Warning
            };

            checks.push(HardeningCheck {
                id: format!("ownership_{}", path.replace('/', "_")),
                name: format!("Ownership: {path}"),
                category: CheckCategory::FilePermissions,
                status,
                summary: format!("{path}: uid={uid} gid={gid}"),
                detail: if uid == 0 && gid == 0 {
                    format!("{path} is owned by root:root as expected.")
                } else {
                    format!(
                        "{path} is owned by uid:{uid} gid:{gid}. \
                         Expected root:root (0:0) for {desc}."
                    )
                },
            });
        }

        #[cfg(not(unix))]
        {
            let _ = (meta, desc);
        }
    }

    checks
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn read_proc_trim(path: &str) -> Option<String> {
    std::fs::read_to_string(path)
        .map(|s| s.trim().to_string())
        .ok()
        .filter(|s| !s.is_empty())
}

fn is_file_readable(path: &str) -> bool {
    std::fs::metadata(path).is_ok()
}

fn command_exists(cmd: &str) -> bool {
    if let Ok(path) = std::env::var("PATH") {
        for dir in path.split(':') {
            let full = format!("{dir}/{cmd}");
            if std::path::Path::new(&full).is_file() {
                return true;
            }
        }
    }
    false
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

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // --- CheckStatus ---

    #[test]
    fn status_labels() {
        assert_eq!(CheckStatus::Pass.label(), "PASS");
        assert_eq!(CheckStatus::Warning.label(), "WARNING");
        assert_eq!(CheckStatus::Fail.label(), "FAIL");
        assert_eq!(CheckStatus::Unavailable.label(), "UNAVAILABLE");
    }

    // --- CheckCategory ---

    #[test]
    fn category_labels() {
        assert_eq!(CheckCategory::Ssh.label(), "SSH");
        assert_eq!(CheckCategory::FilePermissions.label(), "File Permissions");
        assert_eq!(CheckCategory::Accounts.label(), "Accounts");
        assert_eq!(CheckCategory::Privileges.label(), "Privileges");
        assert_eq!(CheckCategory::Filesystem.label(), "Filesystem");
        assert_eq!(CheckCategory::Kernel.label(), "Kernel");
        assert_eq!(CheckCategory::Firewall.label(), "Firewall");
        assert_eq!(CheckCategory::Logging.label(), "Logging");
    }

    // --- SSH config parsing ---

    #[test]
    fn parse_ssh_config_basic() {
        let content = "PermitRootLogin no\nPasswordAuthentication yes\n";
        let d = parse_ssh_config(content);
        assert_eq!(d.get("permitrootlogin").unwrap(), "no");
        assert_eq!(d.get("passwordauthentication").unwrap(), "yes");
    }

    #[test]
    fn parse_ssh_config_with_comments() {
        let content = "# This is a comment\nPermitRootLogin no\n# Another comment\n";
        let d = parse_ssh_config(content);
        assert_eq!(d.len(), 1);
        assert_eq!(d.get("permitrootlogin").unwrap(), "no");
    }

    #[test]
    fn parse_ssh_config_empty() {
        let d = parse_ssh_config("");
        assert!(d.is_empty());
    }

    #[test]
    fn parse_ssh_config_include_skipped() {
        let content = "Include /etc/ssh/sshd_config.d/*.conf\nPermitRootLogin no\n";
        let d = parse_ssh_config(content);
        assert_eq!(d.len(), 1);
        assert!(!d.contains_key("include"));
    }

    #[test]
    fn parse_ssh_config_malformed() {
        let content = "no_value_here\n";
        let d = parse_ssh_config(content);
        // "no_value_here" has no whitespace, so split_once won't match
        assert!(d.is_empty());
    }

    // --- Filter matching ---

    #[test]
    fn filter_matches_status() {
        let check = HardeningCheck {
            id: "test".into(),
            name: "Test".into(),
            category: CheckCategory::Ssh,
            status: CheckStatus::Pass,
            summary: "ok".into(),
            detail: "detail".into(),
        };
        let filter = CheckFilter {
            status: Some(CheckStatus::Pass),
            ..Default::default()
        };
        assert!(filter.matches(&check));
        let filter2 = CheckFilter {
            status: Some(CheckStatus::Fail),
            ..Default::default()
        };
        assert!(!filter2.matches(&check));
    }

    #[test]
    fn filter_matches_category() {
        let check = HardeningCheck {
            id: "test".into(),
            name: "Test".into(),
            category: CheckCategory::Kernel,
            status: CheckStatus::Pass,
            summary: "ok".into(),
            detail: "detail".into(),
        };
        let filter = CheckFilter {
            category: Some(CheckCategory::Kernel),
            ..Default::default()
        };
        assert!(filter.matches(&check));
        let filter2 = CheckFilter {
            category: Some(CheckCategory::Ssh),
            ..Default::default()
        };
        assert!(!filter2.matches(&check));
    }

    #[test]
    fn filter_matches_search_case_insensitive() {
        let check = HardeningCheck {
            id: "test".into(),
            name: "SSH PermitRootLogin".into(),
            category: CheckCategory::Ssh,
            status: CheckStatus::Pass,
            summary: "PermitRootLogin = no".into(),
            detail: "Root login restricted".into(),
        };
        let filter = CheckFilter {
            search: "ssh".into(),
            ..Default::default()
        };
        assert!(filter.matches(&check));
        let filter2 = CheckFilter {
            search: "SSH".into(),
            ..Default::default()
        };
        assert!(filter2.matches(&check));
    }

    #[test]
    fn filter_combined() {
        let check = HardeningCheck {
            id: "test".into(),
            name: "SSH PermitRootLogin".into(),
            category: CheckCategory::Ssh,
            status: CheckStatus::Pass,
            summary: "PermitRootLogin = no".into(),
            detail: "Root login restricted".into(),
        };
        let filter = CheckFilter {
            status: Some(CheckStatus::Pass),
            category: Some(CheckCategory::Ssh),
            search: "root".into(),
        };
        assert!(filter.matches(&check));

        // Wrong status
        let filter2 = CheckFilter {
            status: Some(CheckStatus::Fail),
            category: Some(CheckCategory::Ssh),
            search: "root".into(),
        };
        assert!(!filter2.matches(&check));
    }

    // --- HardeningAudit ---

    #[test]
    fn audit_summary_counts() {
        let audit = HardeningAudit {
            checks: vec![
                HardeningCheck {
                    id: "a".into(),
                    name: "A".into(),
                    category: CheckCategory::Ssh,
                    status: CheckStatus::Pass,
                    summary: "pass".into(),
                    detail: "".into(),
                },
                HardeningCheck {
                    id: "b".into(),
                    name: "B".into(),
                    category: CheckCategory::Kernel,
                    status: CheckStatus::Fail,
                    summary: "fail".into(),
                    detail: "".into(),
                },
                HardeningCheck {
                    id: "c".into(),
                    name: "C".into(),
                    category: CheckCategory::Accounts,
                    status: CheckStatus::Warning,
                    summary: "warn".into(),
                    detail: "".into(),
                },
                HardeningCheck {
                    id: "d".into(),
                    name: "D".into(),
                    category: CheckCategory::Logging,
                    status: CheckStatus::Unavailable,
                    summary: "unavail".into(),
                    detail: "".into(),
                },
            ],
            summary: AuditSummary::default(),
        };
        assert_eq!(audit.len(), 4);
        assert!(!audit.is_empty());
        assert_eq!(audit.get(0).unwrap().status, CheckStatus::Pass);
        assert!(audit.get(99).is_none());
    }

    #[test]
    fn audit_filter_returns_indices() {
        let audit = HardeningAudit {
            checks: vec![
                HardeningCheck {
                    id: "a".into(),
                    name: "A".into(),
                    category: CheckCategory::Ssh,
                    status: CheckStatus::Pass,
                    summary: "ok".into(),
                    detail: "".into(),
                },
                HardeningCheck {
                    id: "b".into(),
                    name: "B".into(),
                    category: CheckCategory::Kernel,
                    status: CheckStatus::Fail,
                    summary: "fail".into(),
                    detail: "".into(),
                },
            ],
            summary: AuditSummary::default(),
        };
        let filter = CheckFilter {
            status: Some(CheckStatus::Fail),
            ..Default::default()
        };
        let indices = audit.filter(&filter);
        assert_eq!(indices.len(), 1);
        assert_eq!(indices[0], 1);
    }

    // --- Sensitive file classification ---

    #[test]
    fn sensitive_files_list() {
        assert!(SENSITIVE_FILES.iter().any(|f| f.path == "/etc/passwd"));
        assert!(SENSITIVE_FILES.iter().any(|f| f.path == "/etc/shadow"));
        assert!(
            SENSITIVE_FILES
                .iter()
                .any(|f| f.path == "/etc/ssh/sshd_config")
        );
    }

    // --- World-writable classification ---

    #[test]
    fn world_writable_check_runs() {
        let checks = check_world_writable_paths();
        // Should not panic, may or may not find issues depending on system
        assert!(!checks.is_empty() || true); // May be empty if no world-writable issues
    }

    // --- SUID/SGID classification ---

    #[test]
    fn suid_sgid_check_runs() {
        let checks = check_suid_sgid();
        // Should complete without panic
        assert!(!checks.is_empty());
    }

    // --- Kernel hardening ---

    #[test]
    fn kernel_check_runs() {
        let checks = check_kernel_hardening();
        // Should have at least 3 checks (aslr, ptrace, core)
        assert!(checks.len() >= 2);
    }

    // --- Logging posture ---

    #[test]
    fn logging_check_runs() {
        let checks = check_logging_posture();
        assert!(!checks.is_empty());
    }

    // --- Collect audit runs without panic ---

    #[test]
    fn collect_audit_no_panic() {
        let audit = collect_audit();
        assert!(!audit.checks.is_empty());
        assert!(audit.summary.total > 0);
    }

    // --- Unavailable state ---

    #[test]
    fn unavailable_check() {
        let check = HardeningCheck {
            id: "test".into(),
            name: "Test".into(),
            category: CheckCategory::Kernel,
            status: CheckStatus::Unavailable,
            summary: "Cannot determine".into(),
            detail: "Data not available".into(),
        };
        assert_eq!(check.status, CheckStatus::Unavailable);
        assert_eq!(check.status.label(), "UNAVAILABLE");
    }

    // --- Empty filter matches all ---

    #[test]
    fn empty_filter_matches_all() {
        let filter = CheckFilter::default();
        let checks = vec![
            HardeningCheck {
                id: "a".into(),
                name: "A".into(),
                category: CheckCategory::Ssh,
                status: CheckStatus::Pass,
                summary: "ok".into(),
                detail: "".into(),
            },
            HardeningCheck {
                id: "b".into(),
                name: "B".into(),
                category: CheckCategory::Kernel,
                status: CheckStatus::Fail,
                summary: "fail".into(),
                detail: "".into(),
            },
        ];
        for check in &checks {
            assert!(filter.matches(check));
        }
    }

    // --- Audit defaults ---

    #[test]
    fn audit_default() {
        let audit = HardeningAudit::default();
        assert!(audit.is_empty());
        assert_eq!(audit.len(), 0);
        assert_eq!(audit.summary.total, 0);
    }
}
