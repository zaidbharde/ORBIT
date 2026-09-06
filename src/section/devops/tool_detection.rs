//! Read-only DevOps tool detection and version reporting.
//!
//! Checks for Git, Rust toolchain, Docker and Podman by probing PATH and
//! running fixed, minimal version commands. All operations are strictly
//! read-only — no containers are started, images pulled, or state modified.

use std::time::SystemTime;

/// Tool availability status.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToolStatus {
    /// Tool binary found and version successfully retrieved.
    Available,
    /// Tool binary not found in PATH.
    Unavailable,
    /// Tool binary found but version check failed or produced unexpected output.
    Unknown,
}

impl ToolStatus {
    pub fn label(self) -> &'static str {
        match self {
            ToolStatus::Available => "AVAILABLE",
            ToolStatus::Unavailable => "UNAVAILABLE",
            ToolStatus::Unknown => "UNKNOWN",
        }
    }
}

/// Detected information for a single DevOps tool.
#[derive(Clone, Debug)]
pub struct ToolInfo {
    pub name: String,
    pub status: ToolStatus,
    pub version: Option<String>,
    pub detail: String,
}

/// Complete DevOps environment snapshot, collected once per cycle.
#[derive(Clone, Debug)]
pub struct DevOpsSnapshot {
    pub tools: Vec<ToolInfo>,
    pub last_updated: Option<SystemTime>,
}

impl Default for DevOpsSnapshot {
    fn default() -> Self {
        Self {
            tools: Vec::new(),
            last_updated: None,
        }
    }
}

impl DevOpsSnapshot {
    /// Collect environment information for all tracked DevOps tools.
    pub fn collect() -> Self {
        let mut tools = Vec::new();
        tools.push(detect_git());
        tools.push(detect_rustc());
        tools.push(detect_cargo());
        tools.push(detect_docker());
        tools.push(detect_podman());

        Self {
            tools,
            last_updated: Some(SystemTime::now()),
        }
    }

    /// Number of tools with AVAILABLE status.
    pub fn available_count(&self) -> usize {
        self.tools
            .iter()
            .filter(|t| t.status == ToolStatus::Available)
            .count()
    }

    /// Number of tools with UNAVAILABLE status.
    pub fn unavailable_count(&self) -> usize {
        self.tools
            .iter()
            .filter(|t| t.status == ToolStatus::Unavailable)
            .count()
    }

    /// Number of tools with UNKNOWN status.
    pub fn unknown_count(&self) -> usize {
        self.tools
            .iter()
            .filter(|t| t.status == ToolStatus::Unknown)
            .count()
    }
}

// ---------------------------------------------------------------------------
// PATH lookup
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// Fixed, read-only command execution
// ---------------------------------------------------------------------------

/// Run a fixed command with known arguments. Returns stdout on success.
fn run_version_cmd(cmd: &str, args: &[&str]) -> Option<String> {
    std::process::Command::new(cmd)
        .args(args)
        .output()
        .ok()
        .and_then(|output| {
            if output.status.success() {
                let mut text = String::from_utf8_lossy(&output.stdout).into_owned();
                if text.is_empty() {
                    text = String::from_utf8_lossy(&output.stderr).into_owned();
                }
                Some(text.trim().to_owned())
            } else {
                None
            }
        })
}

/// Parse the first line of version output, stripping common prefixes.
fn parse_version_line(output: &str) -> Option<String> {
    let line = output.lines().next()?.trim();
    // Some tools prefix with "tool name version " — just return the whole first line.
    Some(line.to_owned())
}

// ---------------------------------------------------------------------------
// Individual tool detectors
// ---------------------------------------------------------------------------

fn detect_git() -> ToolInfo {
    let name = "Git".to_owned();
    if which("git").is_none() {
        return ToolInfo {
            name,
            status: ToolStatus::Unavailable,
            version: None,
            detail: "git binary not found in PATH".to_owned(),
        };
    }
    match run_version_cmd("git", &["--version"]) {
        Some(output) => {
            let version = parse_version_line(&output);
            ToolInfo {
                name,
                status: ToolStatus::Available,
                version: version.clone(),
                detail: format!("Version: {}", version.unwrap_or_else(|| "unknown".into())),
            }
        }
        None => ToolInfo {
            name,
            status: ToolStatus::Unknown,
            version: None,
            detail: "git found but --version failed".to_owned(),
        },
    }
}

fn detect_rustc() -> ToolInfo {
    let name = "Rust (rustc)".to_owned();
    if which("rustc").is_none() {
        return ToolInfo {
            name,
            status: ToolStatus::Unavailable,
            version: None,
            detail: "rustc binary not found in PATH".to_owned(),
        };
    }
    match run_version_cmd("rustc", &["--version"]) {
        Some(output) => {
            let version = parse_version_line(&output);
            ToolInfo {
                name,
                status: ToolStatus::Available,
                version: version.clone(),
                detail: format!("Version: {}", version.unwrap_or_else(|| "unknown".into())),
            }
        }
        None => ToolInfo {
            name,
            status: ToolStatus::Unknown,
            version: None,
            detail: "rustc found but --version failed".to_owned(),
        },
    }
}

fn detect_cargo() -> ToolInfo {
    let name = "Cargo".to_owned();
    if which("cargo").is_none() {
        return ToolInfo {
            name,
            status: ToolStatus::Unavailable,
            version: None,
            detail: "cargo binary not found in PATH".to_owned(),
        };
    }
    match run_version_cmd("cargo", &["--version"]) {
        Some(output) => {
            let version = parse_version_line(&output);
            ToolInfo {
                name,
                status: ToolStatus::Available,
                version: version.clone(),
                detail: format!("Version: {}", version.unwrap_or_else(|| "unknown".into())),
            }
        }
        None => ToolInfo {
            name,
            status: ToolStatus::Unknown,
            version: None,
            detail: "cargo found but --version failed".to_owned(),
        },
    }
}

fn detect_docker() -> ToolInfo {
    let name = "Docker".to_owned();
    if which("docker").is_none() {
        return ToolInfo {
            name,
            status: ToolStatus::Unavailable,
            version: None,
            detail: "docker binary not found in PATH".to_owned(),
        };
    }
    // Try version check first
    let version_output = run_version_cmd("docker", &["--version"]);
    let version = version_output.as_deref().and_then(parse_version_line);

    // Try a safe, read-only daemon check: `docker info` (does not modify state)
    let daemon_ok =
        run_version_cmd("docker", &["info", "--format", "{{.ServerVersion}}"]).is_some();

    let (status, detail) = if daemon_ok {
        (
            ToolStatus::Available,
            format!(
                "Daemon reachable{}",
                if let Some(ref v) = version {
                    format!(", {v}")
                } else {
                    String::new()
                }
            ),
        )
    } else if version.is_some() {
        (
            ToolStatus::Unknown,
            "Binary found but daemon not reachable".to_owned(),
        )
    } else {
        (
            ToolStatus::Unknown,
            "docker found but --version failed".to_owned(),
        )
    };

    ToolInfo {
        name,
        status,
        version,
        detail,
    }
}

fn detect_podman() -> ToolInfo {
    let name = "Podman".to_owned();
    if which("podman").is_none() {
        return ToolInfo {
            name,
            status: ToolStatus::Unavailable,
            version: None,
            detail: "podman binary not found in PATH".to_owned(),
        };
    }
    let version_output = run_version_cmd("podman", &["--version"]);
    let version = version_output.as_deref().and_then(parse_version_line);

    // Safe daemon/availability check: `podman info --format {{.Host.RemoteSocket.Path}}`
    let info_ok =
        run_version_cmd("podman", &["info", "--format", "{{.Version.Version}}"]).is_some();

    let (status, detail) = if info_ok {
        (
            ToolStatus::Available,
            format!(
                "Available{}",
                if let Some(ref v) = version {
                    format!(", {v}")
                } else {
                    String::new()
                }
            ),
        )
    } else if version.is_some() {
        (
            ToolStatus::Unknown,
            "Binary found but basic info check failed".to_owned(),
        )
    } else {
        (
            ToolStatus::Unknown,
            "podman found but --version failed".to_owned(),
        )
    };

    ToolInfo {
        name,
        status,
        version,
        detail,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_status_labels() {
        assert_eq!(ToolStatus::Available.label(), "AVAILABLE");
        assert_eq!(ToolStatus::Unavailable.label(), "UNAVAILABLE");
        assert_eq!(ToolStatus::Unknown.label(), "UNKNOWN");
    }

    #[test]
    fn parse_version_line_extracts_first_line() {
        assert_eq!(
            parse_version_line("git version 2.43.0\nother line"),
            Some("git version 2.43.0".into())
        );
    }

    #[test]
    fn parse_version_line_returns_none_for_empty() {
        assert_eq!(parse_version_line(""), None);
    }

    #[test]
    fn parse_version_line_trims_whitespace() {
        assert_eq!(
            parse_version_line("  rustc 1.75.0  \n"),
            Some("rustc 1.75.0".into())
        );
    }

    #[test]
    fn parse_version_line_handles_single_line() {
        assert_eq!(
            parse_version_line("cargo 1.75.0 (1d8b0ac10 2023-11-20)"),
            Some("cargo 1.75.0 (1d8b0ac10 2023-11-20)".into())
        );
    }

    #[test]
    fn snapshot_defaults_are_empty() {
        let snap = DevOpsSnapshot::default();
        assert!(snap.tools.is_empty());
        assert!(snap.last_updated.is_none());
    }

    #[test]
    fn snapshot_collect_populates_all_tools() {
        let snap = DevOpsSnapshot::collect();
        assert_eq!(snap.tools.len(), 5);
        assert!(snap.last_updated.is_some());
    }

    #[test]
    fn snapshot_counts_are_consistent() {
        let snap = DevOpsSnapshot::collect();
        let total = snap.available_count() + snap.unavailable_count() + snap.unknown_count();
        assert_eq!(total, snap.tools.len());
    }

    #[test]
    fn git_detection_does_not_panic() {
        let info = detect_git();
        assert_eq!(info.name, "Git");
        assert!(matches!(
            info.status,
            ToolStatus::Available | ToolStatus::Unavailable | ToolStatus::Unknown
        ));
    }

    #[test]
    fn rustc_detection_does_not_panic() {
        let info = detect_rustc();
        assert_eq!(info.name, "Rust (rustc)");
        assert!(matches!(
            info.status,
            ToolStatus::Available | ToolStatus::Unavailable | ToolStatus::Unknown
        ));
    }

    #[test]
    fn cargo_detection_does_not_panic() {
        let info = detect_cargo();
        assert_eq!(info.name, "Cargo");
        assert!(matches!(
            info.status,
            ToolStatus::Available | ToolStatus::Unavailable | ToolStatus::Unknown
        ));
    }

    #[test]
    fn docker_detection_does_not_panic() {
        let info = detect_docker();
        assert_eq!(info.name, "Docker");
        assert!(matches!(
            info.status,
            ToolStatus::Available | ToolStatus::Unavailable | ToolStatus::Unknown
        ));
    }

    #[test]
    fn podman_detection_does_not_panic() {
        let info = detect_podman();
        assert_eq!(info.name, "Podman");
        assert!(matches!(
            info.status,
            ToolStatus::Available | ToolStatus::Unavailable | ToolStatus::Unknown
        ));
    }

    #[test]
    fn detect_nonexistent_tool_returns_unavailable() {
        // A binary that definitely does not exist
        let info = ToolInfo {
            name: "FakeTool".into(),
            status: if which("nonexistent_tool_xyz_999").is_some() {
                ToolStatus::Available
            } else {
                ToolStatus::Unavailable
            },
            version: None,
            detail: String::new(),
        };
        assert_eq!(info.status, ToolStatus::Unavailable);
    }

    #[test]
    fn tool_info_fields_are_preserved() {
        let info = ToolInfo {
            name: "Test".into(),
            status: ToolStatus::Available,
            version: Some("1.0.0".into()),
            detail: "test detail".into(),
        };
        assert_eq!(info.name, "Test");
        assert_eq!(info.status, ToolStatus::Available);
        assert_eq!(info.version.as_deref(), Some("1.0.0"));
        assert_eq!(info.detail, "test detail");
    }

    #[test]
    fn collect_is_idempotent() {
        let s1 = DevOpsSnapshot::collect();
        let s2 = DevOpsSnapshot::collect();
        assert_eq!(s1.tools.len(), s2.tools.len());
        for (a, b) in s1.tools.iter().zip(s2.tools.iter()) {
            assert_eq!(a.name, b.name);
            assert_eq!(a.status, b.status);
        }
    }

    #[test]
    fn which_returns_none_for_garbage_name() {
        assert!(which("not_a_real_binary_name_12345").is_none());
    }

    #[test]
    fn run_version_cmd_returns_none_for_missing_binary() {
        assert!(run_version_cmd("not_a_real_binary_name_12345", &["--version"]).is_none());
    }

    #[test]
    fn parse_version_line_strips_trailing_newline() {
        assert_eq!(
            parse_version_line("Docker version 24.0.7, build afdd53b\n"),
            Some("Docker version 24.0.7, build afdd53b".into())
        );
    }
}
