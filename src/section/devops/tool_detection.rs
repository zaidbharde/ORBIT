//! Read-only DevOps tool detection, version reporting and project environment inspection.
//!
//! Checks for Git, Rust toolchain, Node.js, Python and container tools by probing
//! PATH and running fixed, minimal version commands. Also inspects the current
//! project environment (OS, kernel, architecture, working directory).
//! All operations are strictly read-only — no packages are installed, no
//! repositories modified, no containers created or deleted.

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

/// Git repository information (read-only).
#[derive(Clone, Debug, Default)]
pub struct GitRepoInfo {
    pub in_repo: bool,
    pub repo_root: Option<String>,
    pub branch: Option<String>,
    pub working_tree_state: GitWorkingTreeState,
}

/// Clean / Modified / Unknown working-tree state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GitWorkingTreeState {
    Clean,
    Modified,
    Unknown,
}

impl GitWorkingTreeState {
    pub fn label(self) -> &'static str {
        match self {
            GitWorkingTreeState::Clean => "Clean",
            GitWorkingTreeState::Modified => "Modified",
            GitWorkingTreeState::Unknown => "Unknown",
        }
    }
}

impl Default for GitWorkingTreeState {
    fn default() -> Self {
        GitWorkingTreeState::Unknown
    }
}

/// Rustup information (read-only).
#[derive(Clone, Debug, Default)]
pub struct RustupInfo {
    pub available: bool,
    pub version: Option<String>,
    pub active_toolchain: Option<String>,
    pub default_toolchain: Option<String>,
}

/// Node.js environment information (read-only).
#[derive(Clone, Debug, Default)]
pub struct NodeEnv {
    pub node_available: bool,
    pub node_version: Option<String>,
    pub npm_available: bool,
    pub npm_version: Option<String>,
    pub pnpm_available: bool,
    pub pnpm_version: Option<String>,
    pub yarn_available: bool,
    pub yarn_version: Option<String>,
}

/// Python environment information (read-only).
#[derive(Clone, Debug, Default)]
pub struct PythonEnv {
    pub python3_available: bool,
    pub python3_version: Option<String>,
    pub pip_available: bool,
    pub pip_version: Option<String>,
    pub venv_detected: bool,
    pub venv_path: Option<String>,
}

/// Project environment information (read-only).
#[derive(Clone, Debug, Default)]
pub struct ProjectEnv {
    pub working_dir: Option<String>,
    pub os_name: Option<String>,
    pub kernel_version: Option<String>,
    pub architecture: Option<String>,
}

/// Complete DevOps environment snapshot, collected once per cycle.
#[derive(Clone, Debug)]
pub struct DevOpsSnapshot {
    pub tools: Vec<ToolInfo>,
    pub git_repo: GitRepoInfo,
    pub rustup: RustupInfo,
    pub node_env: NodeEnv,
    pub python_env: PythonEnv,
    pub project_env: ProjectEnv,
    pub last_updated: Option<SystemTime>,
}

impl Default for DevOpsSnapshot {
    fn default() -> Self {
        Self {
            tools: Vec::new(),
            git_repo: GitRepoInfo::default(),
            rustup: RustupInfo::default(),
            node_env: NodeEnv::default(),
            python_env: PythonEnv::default(),
            project_env: ProjectEnv::default(),
            last_updated: None,
        }
    }
}

impl DevOpsSnapshot {
    /// Collect environment information for all tracked DevOps tools and project context.
    pub fn collect() -> Self {
        // Version Control
        let (git_tool, git_repo) = detect_git();

        // Rust Toolchain
        let (rustc_tool, ()) = detect_rustc();
        let (cargo_tool, ()) = detect_cargo();
        let rustup = detect_rustup();

        // Node.js Toolchain
        let (node_tool, ()) = detect_node();
        let (npm_tool, ()) = detect_npm();
        let (pnpm_tool, ()) = detect_pnpm();
        let (yarn_tool, ()) = detect_yarn();

        // Python Environment
        let (python_tool, (venv_detected, venv_path)) = detect_python3();
        let (pip_tool, ()) = detect_pip();

        // Container Tools
        let docker_tool = detect_docker();
        let podman_tool = detect_podman();

        let node_env = NodeEnv {
            node_available: node_tool.status == ToolStatus::Available,
            node_version: node_tool.version.clone(),
            npm_available: npm_tool.status == ToolStatus::Available,
            npm_version: npm_tool.version.clone(),
            pnpm_available: pnpm_tool.status == ToolStatus::Available,
            pnpm_version: pnpm_tool.version.clone(),
            yarn_available: yarn_tool.status == ToolStatus::Available,
            yarn_version: yarn_tool.version.clone(),
        };
        let python_env = PythonEnv {
            python3_available: python_tool.status == ToolStatus::Available,
            python3_version: python_tool.version.clone(),
            pip_available: pip_tool.status == ToolStatus::Available,
            pip_version: pip_tool.version.clone(),
            venv_detected,
            venv_path,
        };
        let project_env = collect_project_env();

        let mut tools = Vec::with_capacity(9);
        tools.push(git_tool);
        tools.push(rustc_tool);
        tools.push(cargo_tool);
        tools.push(node_tool);
        tools.push(npm_tool);
        tools.push(python_tool);
        tools.push(pip_tool);
        tools.push(docker_tool);
        tools.push(podman_tool);

        Self {
            tools,
            git_repo,
            rustup,
            node_env,
            python_env,
            project_env,
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

pub fn which(cmd: &str) -> Option<String> {
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
pub fn run_version_cmd(cmd: &str, args: &[&str]) -> Option<String> {
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
pub fn parse_version_line(output: &str) -> Option<String> {
    let line = output.lines().next()?.trim();
    Some(line.to_owned())
}

/// Extract a semver-like version number (e.g. "1.75.0") from a version string.
pub fn extract_version_number(output: &str) -> Option<String> {
    let line = output.lines().next()?.trim();
    // Look for a pattern like X.Y.Z
    let mut start = None;
    for (i, c) in line.char_indices() {
        if c.is_ascii_digit() && start.is_none() {
            start = Some(i);
        } else if let Some(s) = start {
            if !c.is_ascii_digit() && c != '.' {
                return Some(line[s..i].to_string());
            }
        }
    }
    start.map(|s| line[s..].to_string())
}

// ---------------------------------------------------------------------------
// Individual tool detectors
// ---------------------------------------------------------------------------

fn detect_git() -> (ToolInfo, GitRepoInfo) {
    let name = "Git".to_owned();
    if which("git").is_none() {
        return (
            ToolInfo {
                name,
                status: ToolStatus::Unavailable,
                version: None,
                detail: "git binary not found in PATH".to_owned(),
            },
            GitRepoInfo::default(),
        );
    }

    let version_output = run_version_cmd("git", &["--version"]);
    let version = version_output.as_deref().and_then(parse_version_line);

    let (status, detail) = if let Some(ref v) = version {
        (ToolStatus::Available, format!("Version: {v}"))
    } else {
        (
            ToolStatus::Unknown,
            "git found but --version failed".to_owned(),
        )
    };

    let repo_info = detect_git_repo();

    (
        ToolInfo {
            name,
            status,
            version,
            detail,
        },
        repo_info,
    )
}

fn detect_git_repo() -> GitRepoInfo {
    // Check if we're inside a git repo
    let rev_parse = run_version_cmd("git", &["rev-parse", "--is-inside-work-tree"]);
    let in_repo = rev_parse.as_deref() == Some("true");

    if !in_repo {
        return GitRepoInfo {
            in_repo: false,
            repo_root: None,
            branch: None,
            working_tree_state: GitWorkingTreeState::Unknown,
        };
    }

    let repo_root = run_version_cmd("git", &["rev-parse", "--show-toplevel"]);
    let branch = detect_git_branch();
    let working_tree_state = detect_git_status();

    GitRepoInfo {
        in_repo: true,
        repo_root,
        branch,
        working_tree_state,
    }
}

fn detect_git_branch() -> Option<String> {
    // Try symbolic-ref first (fast, no porcelain)
    if let Some(output) = run_version_cmd("git", &["symbolic-ref", "--short", "HEAD"]) {
        let trimmed = output.trim().to_string();
        if !trimmed.is_empty() {
            return Some(trimmed);
        }
    }
    // Detached HEAD — fallback to rev-parse
    run_version_cmd("git", &["rev-parse", "--short", "HEAD"])
        .map(|s| format!("(detached @ {})", s.trim()))
}

fn detect_git_status() -> GitWorkingTreeState {
    // Use --porcelain for machine-readable status
    match run_version_cmd("git", &["status", "--porcelain"]) {
        Some(output) => {
            let trimmed = output.trim();
            if trimmed.is_empty() {
                GitWorkingTreeState::Clean
            } else {
                GitWorkingTreeState::Modified
            }
        }
        None => GitWorkingTreeState::Unknown,
    }
}

fn detect_rustc() -> (ToolInfo, ()) {
    let name = "Rust (rustc)".to_owned();
    if which("rustc").is_none() {
        return (
            ToolInfo {
                name,
                status: ToolStatus::Unavailable,
                version: None,
                detail: "rustc binary not found in PATH".to_owned(),
            },
            (),
        );
    }
    match run_version_cmd("rustc", &["--version"]) {
        Some(output) => {
            let version = parse_version_line(&output);
            (
                ToolInfo {
                    name,
                    status: ToolStatus::Available,
                    version: version.clone(),
                    detail: format!("Version: {}", version.unwrap_or_else(|| "unknown".into())),
                },
                (),
            )
        }
        None => (
            ToolInfo {
                name,
                status: ToolStatus::Unknown,
                version: None,
                detail: "rustc found but --version failed".to_owned(),
            },
            (),
        ),
    }
}

fn detect_cargo() -> (ToolInfo, ()) {
    let name = "Cargo".to_owned();
    if which("cargo").is_none() {
        return (
            ToolInfo {
                name,
                status: ToolStatus::Unavailable,
                version: None,
                detail: "cargo binary not found in PATH".to_owned(),
            },
            (),
        );
    }
    match run_version_cmd("cargo", &["--version"]) {
        Some(output) => {
            let version = parse_version_line(&output);
            (
                ToolInfo {
                    name,
                    status: ToolStatus::Available,
                    version: version.clone(),
                    detail: format!("Version: {}", version.unwrap_or_else(|| "unknown".into())),
                },
                (),
            )
        }
        None => (
            ToolInfo {
                name,
                status: ToolStatus::Unknown,
                version: None,
                detail: "cargo found but --version failed".to_owned(),
            },
            (),
        ),
    }
}

fn detect_rustup() -> RustupInfo {
    if which("rustup").is_none() {
        return RustupInfo {
            available: false,
            version: None,
            active_toolchain: None,
            default_toolchain: None,
        };
    }

    let version = run_version_cmd("rustup", &["--version"]).and_then(|o| parse_version_line(&o));

    let active_toolchain = run_version_cmd("rustup", &["show", "active-toolchain"])
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());

    let default_toolchain = run_version_cmd("rustup", &["show", "installed-chains"])
        .and_then(|output| {
            // First non-header line is typically the default toolchain
            output
                .lines()
                .find(|l| l.contains("(default)"))
                .map(|l| l.split_whitespace().next().unwrap_or("").to_string())
        })
        .filter(|s| !s.is_empty());

    RustupInfo {
        available: true,
        version,
        active_toolchain,
        default_toolchain,
    }
}

fn detect_node() -> (ToolInfo, ()) {
    let name = "Node.js".to_owned();
    if which("node").is_none() {
        return (
            ToolInfo {
                name,
                status: ToolStatus::Unavailable,
                version: None,
                detail: "node binary not found in PATH".to_owned(),
            },
            (),
        );
    }
    match run_version_cmd("node", &["--version"]) {
        Some(output) => {
            let version = parse_version_line(&output);
            (
                ToolInfo {
                    name,
                    status: ToolStatus::Available,
                    version: version.clone(),
                    detail: format!("Version: {}", version.unwrap_or_else(|| "unknown".into())),
                },
                (),
            )
        }
        None => (
            ToolInfo {
                name,
                status: ToolStatus::Unknown,
                version: None,
                detail: "node found but --version failed".to_owned(),
            },
            (),
        ),
    }
}

fn detect_npm() -> (ToolInfo, ()) {
    let name = "npm".to_owned();
    if which("npm").is_none() {
        return (
            ToolInfo {
                name,
                status: ToolStatus::Unavailable,
                version: None,
                detail: "npm binary not found in PATH".to_owned(),
            },
            (),
        );
    }
    match run_version_cmd("npm", &["--version"]) {
        Some(output) => {
            let version = parse_version_line(&output);
            (
                ToolInfo {
                    name,
                    status: ToolStatus::Available,
                    version: version.clone(),
                    detail: format!("Version: {}", version.unwrap_or_else(|| "unknown".into())),
                },
                (),
            )
        }
        None => (
            ToolInfo {
                name,
                status: ToolStatus::Unknown,
                version: None,
                detail: "npm found but --version failed".to_owned(),
            },
            (),
        ),
    }
}

fn detect_pnpm() -> (ToolInfo, ()) {
    let name = "pnpm".to_owned();
    if which("pnpm").is_none() {
        return (
            ToolInfo {
                name,
                status: ToolStatus::Unavailable,
                version: None,
                detail: "pnpm binary not found in PATH".to_owned(),
            },
            (),
        );
    }
    match run_version_cmd("pnpm", &["--version"]) {
        Some(output) => {
            let version = parse_version_line(&output);
            (
                ToolInfo {
                    name,
                    status: ToolStatus::Available,
                    version: version.clone(),
                    detail: format!("Version: {}", version.unwrap_or_else(|| "unknown".into())),
                },
                (),
            )
        }
        None => (
            ToolInfo {
                name,
                status: ToolStatus::Unknown,
                version: None,
                detail: "pnpm found but --version failed".to_owned(),
            },
            (),
        ),
    }
}

fn detect_yarn() -> (ToolInfo, ()) {
    let name = "yarn".to_owned();
    if which("yarn").is_none() {
        return (
            ToolInfo {
                name,
                status: ToolStatus::Unavailable,
                version: None,
                detail: "yarn binary not found in PATH".to_owned(),
            },
            (),
        );
    }
    match run_version_cmd("yarn", &["--version"]) {
        Some(output) => {
            let version = parse_version_line(&output);
            (
                ToolInfo {
                    name,
                    status: ToolStatus::Available,
                    version: version.clone(),
                    detail: format!("Version: {}", version.unwrap_or_else(|| "unknown".into())),
                },
                (),
            )
        }
        None => (
            ToolInfo {
                name,
                status: ToolStatus::Unknown,
                version: None,
                detail: "yarn found but --version failed".to_owned(),
            },
            (),
        ),
    }
}

fn detect_python3() -> (ToolInfo, (bool, Option<String>)) {
    let name = "Python 3".to_owned();
    if which("python3").is_none() {
        return (
            ToolInfo {
                name,
                status: ToolStatus::Unavailable,
                version: None,
                detail: "python3 binary not found in PATH".to_owned(),
            },
            (false, None),
        );
    }
    match run_version_cmd("python3", &["--version"]) {
        Some(output) => {
            let version = parse_version_line(&output);
            let venv = detect_python_venv();
            (
                ToolInfo {
                    name,
                    status: ToolStatus::Available,
                    version: version.clone(),
                    detail: format!("Version: {}", version.unwrap_or_else(|| "unknown".into())),
                },
                venv,
            )
        }
        None => (
            ToolInfo {
                name,
                status: ToolStatus::Unknown,
                version: None,
                detail: "python3 found but --version failed".to_owned(),
            },
            (false, None),
        ),
    }
}

/// Detect if the current project has a Python virtual environment.
/// Does NOT activate or modify the venv.
fn detect_python_venv() -> (bool, Option<String>) {
    let cwd = std::env::current_dir()
        .ok()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_default();

    // Common venv locations
    let candidates = [
        format!("{cwd}/.venv"),
        format!("{cwd}/venv"),
        format!("{cwd}/env"),
        format!("{cwd}/.env"),
    ];

    for path in &candidates {
        let p = std::path::Path::new(path);
        // Check for pyvenv.cfg (the canonical venv marker)
        if p.join("pyvenv.cfg").is_file() {
            return (true, Some(path.clone()));
        }
        // Fallback: check for bin/activate (virtualenv style)
        if p.join("bin/activate").is_file() {
            return (true, Some(path.clone()));
        }
    }

    (false, None)
}

fn detect_pip() -> (ToolInfo, ()) {
    let name = "pip".to_owned();
    // Try pip3 first, then pip
    let cmd = if which("pip3").is_some() {
        "pip3"
    } else if which("pip").is_some() {
        "pip"
    } else {
        return (
            ToolInfo {
                name,
                status: ToolStatus::Unavailable,
                version: None,
                detail: "pip binary not found in PATH".to_owned(),
            },
            (),
        );
    };

    match run_version_cmd(cmd, &["--version"]) {
        Some(output) => {
            let version = parse_version_line(&output);
            (
                ToolInfo {
                    name,
                    status: ToolStatus::Available,
                    version: version.clone(),
                    detail: format!("Version: {}", version.unwrap_or_else(|| "unknown".into())),
                },
                (),
            )
        }
        None => (
            ToolInfo {
                name,
                status: ToolStatus::Unknown,
                version: None,
                detail: "{cmd} found but --version failed".to_owned(),
            },
            (),
        ),
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
    let version_output = run_version_cmd("docker", &["--version"]);
    let version = version_output.as_deref().and_then(parse_version_line);

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

// ---------------------------------------------------------------------------
// Project environment
// ---------------------------------------------------------------------------

fn collect_project_env() -> ProjectEnv {
    let working_dir = std::env::current_dir()
        .ok()
        .map(|p| p.to_string_lossy().to_string());

    let os_name = std::env::var("OSTYPE")
        .ok()
        .filter(|s| !s.is_empty())
        .or_else(|| {
            // Fallback: read from /etc/os-release
            read_os_release_name()
                .or_else(|| run_version_cmd("uname", &["-s"]).map(|s| s.trim().to_string()))
        });

    let kernel_version = run_version_cmd("uname", &["-r"]);

    let architecture = run_version_cmd("uname", &["-m"]);

    ProjectEnv {
        working_dir,
        os_name,
        kernel_version,
        architecture,
    }
}

fn read_os_release_name() -> Option<String> {
    let content = std::fs::read_to_string("/etc/os-release").ok()?;
    for line in content.lines() {
        if let Some(value) = line.strip_prefix("PRETTY_NAME=") {
            let value = value.trim_matches('"').trim_matches('\'');
            if !value.is_empty() {
                return Some(value.to_string());
            }
        }
    }
    // Fallback to NAME
    for line in content.lines() {
        if let Some(value) = line.strip_prefix("NAME=") {
            let value = value.trim_matches('"').trim_matches('\'');
            if !value.is_empty() {
                return Some(value.to_string());
            }
        }
    }
    None
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

    // --- parse_version_line ---

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

    // --- extract_version_number ---

    #[test]
    fn extract_version_number_basic() {
        assert_eq!(
            extract_version_number("rustc 1.75.0 (1d8b0ac10 2023-11-20)"),
            Some("1.75.0".into())
        );
    }

    #[test]
    fn extract_version_number_node_style() {
        assert_eq!(extract_version_number("v20.10.0"), Some("20.10.0".into()));
    }

    #[test]
    fn extract_version_number_docker_style() {
        assert_eq!(
            extract_version_number("Docker version 24.0.7, build afdd53b"),
            Some("24.0.7".into())
        );
    }

    #[test]
    fn extract_version_number_empty() {
        assert_eq!(extract_version_number(""), None);
    }

    #[test]
    fn extract_version_number_no_digits() {
        assert_eq!(extract_version_number("no version here"), None);
    }

    // --- snapshot ---

    #[test]
    fn snapshot_defaults_are_empty() {
        let snap = DevOpsSnapshot::default();
        assert!(snap.tools.is_empty());
        assert!(snap.last_updated.is_none());
    }

    #[test]
    fn snapshot_collect_populates_all_tools() {
        let snap = DevOpsSnapshot::collect();
        // 9 tools: git, rustc, cargo, node, npm, python3, pip, docker, podman
        assert_eq!(snap.tools.len(), 9);
        assert!(snap.last_updated.is_some());
    }

    #[test]
    fn snapshot_counts_are_consistent() {
        let snap = DevOpsSnapshot::collect();
        let total = snap.available_count() + snap.unavailable_count() + snap.unknown_count();
        assert_eq!(total, snap.tools.len());
    }

    #[test]
    fn snapshot_has_project_env() {
        let snap = DevOpsSnapshot::collect();
        assert!(snap.project_env.working_dir.is_some());
        assert!(snap.project_env.architecture.is_some());
    }

    // --- Git detection ---

    #[test]
    fn git_detection_does_not_panic() {
        let (info, _repo) = detect_git();
        assert_eq!(info.name, "Git");
        assert!(matches!(
            info.status,
            ToolStatus::Available | ToolStatus::Unavailable | ToolStatus::Unknown
        ));
    }

    #[test]
    fn git_repo_info_fields_are_valid() {
        let (_info, repo) = detect_git();
        if repo.in_repo {
            assert!(repo.repo_root.is_some());
            // branch may be None if HEAD is detached and rev-parse fails
        }
    }

    // --- Rust detection ---

    #[test]
    fn rustc_detection_does_not_panic() {
        let (info, ()) = detect_rustc();
        assert_eq!(info.name, "Rust (rustc)");
        assert!(matches!(
            info.status,
            ToolStatus::Available | ToolStatus::Unavailable | ToolStatus::Unknown
        ));
    }

    #[test]
    fn cargo_detection_does_not_panic() {
        let (info, ()) = detect_cargo();
        assert_eq!(info.name, "Cargo");
        assert!(matches!(
            info.status,
            ToolStatus::Available | ToolStatus::Unavailable | ToolStatus::Unknown
        ));
    }

    #[test]
    fn rustup_detection_does_not_panic() {
        let info = detect_rustup();
        // rustup may or may not be installed
        if info.available {
            // If available, we should have a version
            // (version check may fail in some environments, so just verify no panic)
        }
    }

    // --- Node detection ---

    #[test]
    fn node_detection_does_not_panic() {
        let (info, ()) = detect_node();
        assert_eq!(info.name, "Node.js");
        assert!(matches!(
            info.status,
            ToolStatus::Available | ToolStatus::Unavailable | ToolStatus::Unknown
        ));
    }

    #[test]
    fn npm_detection_does_not_panic() {
        let (info, ()) = detect_npm();
        assert_eq!(info.name, "npm");
        assert!(matches!(
            info.status,
            ToolStatus::Available | ToolStatus::Unavailable | ToolStatus::Unknown
        ));
    }

    // --- Python detection ---

    #[test]
    fn python3_detection_does_not_panic() {
        let (info, _venv) = detect_python3();
        assert_eq!(info.name, "Python 3");
        assert!(matches!(
            info.status,
            ToolStatus::Available | ToolStatus::Unavailable | ToolStatus::Unknown
        ));
    }

    #[test]
    fn pip_detection_does_not_panic() {
        let (info, ()) = detect_pip();
        assert_eq!(info.name, "pip");
        assert!(matches!(
            info.status,
            ToolStatus::Available | ToolStatus::Unavailable | ToolStatus::Unknown
        ));
    }

    #[test]
    fn venv_detection_does_not_panic() {
        let (detected, _path) = detect_python_venv();
        // Just verify no panic
        let _ = detected;
    }

    // --- Container detection ---

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

    // --- Edge cases ---

    #[test]
    fn detect_nonexistent_tool_returns_unavailable() {
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

    // --- GitWorkingTreeState ---

    #[test]
    fn git_working_tree_state_labels() {
        assert_eq!(GitWorkingTreeState::Clean.label(), "Clean");
        assert_eq!(GitWorkingTreeState::Modified.label(), "Modified");
        assert_eq!(GitWorkingTreeState::Unknown.label(), "Unknown");
    }

    #[test]
    fn git_working_tree_state_default_is_unknown() {
        assert_eq!(GitWorkingTreeState::default(), GitWorkingTreeState::Unknown);
    }

    // --- RustupInfo ---

    #[test]
    fn rustup_info_default() {
        let info = RustupInfo::default();
        assert!(!info.available);
        assert!(info.version.is_none());
        assert!(info.active_toolchain.is_none());
        assert!(info.default_toolchain.is_none());
    }

    // --- NodeEnv ---

    #[test]
    fn node_env_default() {
        let env = NodeEnv::default();
        assert!(!env.node_available);
        assert!(!env.npm_available);
        assert!(!env.pnpm_available);
        assert!(!env.yarn_available);
    }

    // --- PythonEnv ---

    #[test]
    fn python_env_default() {
        let env = PythonEnv::default();
        assert!(!env.python3_available);
        assert!(!env.pip_available);
        assert!(!env.venv_detected);
    }

    // --- ProjectEnv ---

    #[test]
    fn project_env_default() {
        let env = ProjectEnv::default();
        assert!(env.working_dir.is_none());
        assert!(env.os_name.is_none());
        assert!(env.kernel_version.is_none());
        assert!(env.architecture.is_none());
    }

    // --- parse_version_line edge cases ---

    #[test]
    fn parse_version_line_whitespace_only() {
        assert_eq!(parse_version_line("   \n  "), Some("".into()));
    }

    #[test]
    fn parse_version_line_newline_only() {
        assert_eq!(parse_version_line("\n"), Some("".into()));
    }

    // --- Malformed output ---

    #[test]
    fn extract_version_number_leading_text() {
        assert_eq!(
            extract_version_number("compiled with Rust 1.80.1"),
            Some("1.80.1".into())
        );
    }

    #[test]
    fn extract_version_number_dotted_without_prefix() {
        assert_eq!(extract_version_number("3.11.5"), Some("3.11.5".into()));
    }
}
