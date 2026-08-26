//! Read-only local security event monitoring.
//!
//! Parses bounded sections of authentication and kernel logs to extract
//! structured security events. All operations are strictly read-only.
//! No passwords, tokens, keys, or secrets are exposed.

use std::collections::HashSet;
use std::fs;

/// Maximum number of events retained in the bounded history.
pub const MAX_EVENTS: usize = 200;

/// Maximum number of lines read per log file per collection cycle.
const MAX_LINES_PER_FILE: usize = 4096;

/// Security event categories.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EventCategory {
    Auth,
    Ssh,
    Sudo,
    Firewall,
    Kernel,
    Session,
    Other,
}

impl EventCategory {
    pub fn label(self) -> &'static str {
        match self {
            Self::Auth => "Auth",
            Self::Ssh => "SSH",
            Self::Sudo => "Sudo",
            Self::Firewall => "Firewall",
            Self::Kernel => "Kernel",
            Self::Session => "Session",
            Self::Other => "Other",
        }
    }

    pub const ALL: [EventCategory; 7] = [
        EventCategory::Auth,
        EventCategory::Ssh,
        EventCategory::Sudo,
        EventCategory::Firewall,
        EventCategory::Kernel,
        EventCategory::Session,
        EventCategory::Other,
    ];
}

/// Severity level for a security event.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Severity {
    Info,
    Warning,
    Error,
}

impl Severity {
    pub fn label(self) -> &'static str {
        match self {
            Self::Info => "Info",
            Self::Warning => "Warning",
            Self::Error => "Error",
        }
    }

    pub const ALL: [Severity; 3] = [Severity::Info, Severity::Warning, Severity::Error];
}

/// A structured, sanitized security event.
#[derive(Debug, Clone)]
pub struct SecurityEvent {
    pub timestamp: String,
    pub severity: Severity,
    pub category: EventCategory,
    pub source: String,
    pub summary: String,
}

/// Event filter for search and category/severity selection.
#[derive(Debug, Clone, Default)]
pub struct EventFilter {
    pub search: String,
    pub severity: Option<Severity>,
    pub category: Option<EventCategory>,
}

impl EventFilter {
    pub fn matches(&self, event: &SecurityEvent) -> bool {
        // Severity filter
        if let Some(sev) = self.severity {
            if event.severity != sev {
                return false;
            }
        }
        // Category filter
        if let Some(cat) = self.category {
            if event.category != cat {
                return false;
            }
        }
        // Text search
        let search_lower = self.search.to_lowercase();
        if !search_lower.is_empty() {
            let fields = [
                event.category.label().to_lowercase(),
                event.source.to_lowercase(),
                event.summary.to_lowercase(),
            ];
            if !fields.iter().any(|f| f.contains(&search_lower)) {
                return false;
            }
        }
        true
    }
}

/// Bounded collection of security events with deduplication.
#[derive(Debug, Clone)]
pub struct EventLog {
    events: Vec<SecurityEvent>,
    seen_hashes: HashSet<u64>,
}

impl Default for EventLog {
    fn default() -> Self {
        Self {
            events: Vec::with_capacity(MAX_EVENTS),
            seen_hashes: HashSet::with_capacity(MAX_EVENTS),
        }
    }
}

impl EventLog {
    /// Add an event, deduplicating by content hash.
    pub fn push(&mut self, event: SecurityEvent) {
        let hash = hash_event(&event);
        if self.seen_hashes.insert(hash) {
            self.events.push(event);
            // Evict oldest if over bound
            if self.events.len() > MAX_EVENTS {
                let excess = self.events.len() - MAX_EVENTS;
                self.events.drain(..excess);
            }
        }
    }

    /// Filter events by the given filter, returning indices into the events vec.
    pub fn filter(&self, filter: &EventFilter) -> Vec<usize> {
        self.events
            .iter()
            .enumerate()
            .filter(|(_, e)| filter.matches(e))
            .map(|(i, _)| i)
            .collect()
    }

    /// Number of events.
    pub fn len(&self) -> usize {
        self.events.len()
    }

    /// Whether the log is empty.
    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
    }

    /// Reference to events.
    pub fn events(&self) -> &[SecurityEvent] {
        &self.events
    }

    /// Get event by index.
    pub fn get(&self, index: usize) -> Option<&SecurityEvent> {
        self.events.get(index)
    }
}

/// Simple hash for deduplication (timestamp + category + summary).
fn hash_event(event: &SecurityEvent) -> u64 {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut hasher = DefaultHasher::new();
    event.timestamp.hash(&mut hasher);
    event.category.label().hash(&mut hasher);
    event.source.hash(&mut hasher);
    event.summary.hash(&mut hasher);
    hasher.finish()
}

// ---------------------------------------------------------------------------
// Log parsing
// ---------------------------------------------------------------------------

/// Parse security events from auth.log or secure log content.
pub fn parse_auth_events(content: &str) -> Vec<SecurityEvent> {
    let mut events = Vec::new();
    let mut lines_read = 0usize;

    for line in content.lines() {
        lines_read += 1;
        if lines_read > MAX_LINES_PER_FILE {
            break;
        }

        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        if let Some(event) = parse_auth_line(line) {
            events.push(event);
        }
    }

    events
}

/// Parse a single auth log line into a security event.
fn parse_auth_line(line: &str) -> Option<SecurityEvent> {
    let timestamp = extract_timestamp(line);
    let lower = line.to_lowercase();

    // SSH authentication events
    if lower.contains("sshd") {
        if lower.contains("failed") || lower.contains("failure") {
            let summary = sanitize_summary(line, "SSH authentication failed");
            return Some(SecurityEvent {
                timestamp,
                severity: Severity::Error,
                category: EventCategory::Ssh,
                source: "sshd".into(),
                summary,
            });
        }
        if lower.contains("accepted") || lower.contains("preauth") {
            let summary = sanitize_summary(line, "SSH authentication accepted");
            return Some(SecurityEvent {
                timestamp,
                severity: Severity::Info,
                category: EventCategory::Ssh,
                source: "sshd".into(),
                summary,
            });
        }
        if lower.contains("invalid user") {
            let summary = sanitize_summary(line, "SSH invalid user attempt");
            return Some(SecurityEvent {
                timestamp,
                severity: Severity::Warning,
                category: EventCategory::Ssh,
                source: "sshd".into(),
                summary,
            });
        }
    }

    // Sudo events
    if lower.contains("sudo") {
        if lower.contains("incorrect password") || lower.contains("authentication failure") {
            let summary = sanitize_summary(line, "Sudo authentication failure");
            return Some(SecurityEvent {
                timestamp,
                severity: Severity::Error,
                category: EventCategory::Sudo,
                source: "sudo".into(),
                summary,
            });
        }
        if lower.contains("command executed") || lower.contains(": command") {
            let summary = sanitize_summary(line, "Sudo command executed");
            return Some(SecurityEvent {
                timestamp,
                severity: Severity::Info,
                category: EventCategory::Sudo,
                source: "sudo".into(),
                summary,
            });
        }
    }

    // Session events
    if lower.contains("session opened") || lower.contains("session closed") {
        let action = if lower.contains("opened") {
            "Session opened"
        } else {
            "Session closed"
        };
        let summary = sanitize_summary(line, action);
        return Some(SecurityEvent {
            timestamp,
            severity: Severity::Info,
            category: EventCategory::Session,
            source: extract_process(line),
            summary,
        });
    }

    // Generic failed authentication
    if lower.contains("failed password") || lower.contains("authentication failure") {
        let summary = sanitize_summary(line, "Authentication failed");
        return Some(SecurityEvent {
            timestamp,
            severity: Severity::Error,
            category: EventCategory::Auth,
            source: extract_process(line),
            summary,
        });
    }

    // Generic successful authentication
    if lower.contains("accepted") && (lower.contains("password") || lower.contains("publickey")) {
        let summary = sanitize_summary(line, "Authentication accepted");
        return Some(SecurityEvent {
            timestamp,
            severity: Severity::Info,
            category: EventCategory::Auth,
            source: extract_process(line),
            summary,
        });
    }

    // Invalid user
    if lower.contains("invalid user") {
        let summary = sanitize_summary(line, "Invalid user login attempt");
        return Some(SecurityEvent {
            timestamp,
            severity: Severity::Warning,
            category: EventCategory::Auth,
            source: extract_process(line),
            summary,
        });
    }

    None
}

/// Parse security events from kern.log or messages.
pub fn parse_kernel_events(content: &str) -> Vec<SecurityEvent> {
    let mut events = Vec::new();
    let mut lines_read = 0usize;

    for line in content.lines() {
        lines_read += 1;
        if lines_read > MAX_LINES_PER_FILE {
            break;
        }

        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        if let Some(event) = parse_kernel_line(line) {
            events.push(event);
        }
    }

    events
}

/// Parse a single kernel log line into a security event.
fn parse_kernel_line(line: &str) -> Option<SecurityEvent> {
    let timestamp = extract_timestamp(line);
    let lower = line.to_lowercase();

    // Permission denied / SELinux / AppArmor
    if lower.contains("denied") || lower.contains("apparmor") && lower.contains("denied") {
        let summary = sanitize_summary(line, "Kernel security denial");
        return Some(SecurityEvent {
            timestamp,
            severity: Severity::Warning,
            category: EventCategory::Kernel,
            source: "kernel".into(),
            summary,
        });
    }

    // Firewall drop/block
    if lower.contains("iptables") || lower.contains("nftables") {
        if lower.contains("drop") || lower.contains("block") || lower.contains("reject") {
            let summary = sanitize_summary(line, "Firewall action detected");
            return Some(SecurityEvent {
                timestamp,
                severity: Severity::Info,
                category: EventCategory::Firewall,
                source: "kernel".into(),
                summary,
            });
        }
    }

    // UFW blocked
    if lower.contains("ufw") && (lower.contains("block") || lower.contains("deny")) {
        let summary = sanitize_summary(line, "UFW firewall block");
        return Some(SecurityEvent {
            timestamp,
            severity: Severity::Info,
            category: EventCategory::Firewall,
            source: "ufw".into(),
            summary,
        });
    }

    // Segfault / oops (security-relevant kernel events)
    if lower.contains("segfault") || lower.contains("kernel oops") || lower.contains("panic") {
        let summary = sanitize_summary(line, "Kernel critical event");
        return Some(SecurityEvent {
            timestamp,
            severity: Severity::Error,
            category: EventCategory::Kernel,
            source: "kernel".into(),
            summary,
        });
    }

    None
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Extract a timestamp from a log line (first 15 chars: "MMM DD HH:MM:SS").
fn extract_timestamp(line: &str) -> String {
    if line.len() >= 15 {
        let candidate = line[..15].trim();
        if candidate.len() >= 6
            && candidate
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == ' ' || c == ':')
        {
            return candidate.to_string();
        }
    }
    "unknown".into()
}

/// Extract the process name from a log line (word after timestamp and hostname).
fn extract_process(line: &str) -> String {
    // Typical: "Aug 26 10:00:01 hostname process[pid]: message"
    let after_ts = if line.len() > 15 { &line[15..] } else { line };
    let trimmed = after_ts.trim_start();
    // Skip hostname (first word), then take process name
    let parts: Vec<&str> = trimmed.split_whitespace().collect();
    if parts.len() >= 2 {
        let process_part = parts[1]
            .split(|c: char| c == '[' || c == ':')
            .next()
            .unwrap_or("system")
            .trim();
        if process_part.is_empty() {
            "system".into()
        } else {
            process_part.to_string()
        }
    } else {
        "system".into()
    }
}

/// Sanitize a log line for safe display.
/// Removes potential sensitive info while keeping useful context.
fn sanitize_summary(line: &str, fallback: &str) -> String {
    // If line is short enough and doesn't contain suspicious patterns, show it
    if line.len() <= 120 && !contains_sensitive_pattern(line) {
        return truncate_str(line, 120);
    }
    // Otherwise return the fallback description
    fallback.into()
}

/// Check if a line likely contains sensitive information.
fn contains_sensitive_pattern(line: &str) -> bool {
    let lower = line.to_lowercase();
    lower.contains("password")
        || lower.contains("token")
        || lower.contains("key")
        || lower.contains("secret")
        || lower.contains("credential")
}

/// Truncate string with ellipsis.
fn truncate_str(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else {
        format!("{}…", &s[..max_len.saturating_sub(1)])
    }
}

// ---------------------------------------------------------------------------
// Collection
// ---------------------------------------------------------------------------

/// Collect security events from available log files.
pub fn collect_events(existing: &EventLog) -> EventLog {
    let mut log = existing.clone();

    // Auth logs
    if let Some(content) = read_file("/var/log/auth.log").or_else(|| read_file("/var/log/secure")) {
        let events = parse_auth_events(&content);
        for event in events {
            log.push(event);
        }
    }

    // Kernel logs
    if let Some(content) = read_file("/var/log/kern.log").or_else(|| read_file("/var/log/messages"))
    {
        let events = parse_kernel_events(&content);
        for event in events {
            log.push(event);
        }
    }

    log
}

fn read_file(path: &str) -> Option<String> {
    fs::read_to_string(path).ok()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // --- Timestamp extraction ---

    #[test]
    fn extract_timestamp_valid() {
        let line = "Aug 26 10:00:01 host sshd[1234]: message";
        assert_eq!(extract_timestamp(line), "Aug 26 10:00:01");
    }

    #[test]
    fn extract_timestamp_short_line() {
        assert_eq!(extract_timestamp("short"), "unknown");
    }

    #[test]
    fn extract_timestamp_empty() {
        assert_eq!(extract_timestamp(""), "unknown");
    }

    // --- Process extraction ---

    #[test]
    fn extract_process_from_auth_line() {
        let line = "Aug 26 10:00:01 host sshd[1234]: Accepted password for user";
        assert_eq!(extract_process(line), "sshd");
    }

    #[test]
    fn extract_process_from_sudo_line() {
        let line = "Aug 26 10:00:01 host sudo: user : command executed";
        assert_eq!(extract_process(line), "sudo");
    }

    // --- Auth event parsing ---

    #[test]
    fn parse_ssh_failed_auth() {
        let line = "Aug 26 10:00:01 host sshd[1234]: Failed password for root from 1.2.3.4 port 22";
        let events = parse_auth_events(line);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].category, EventCategory::Ssh);
        assert_eq!(events[0].severity, Severity::Error);
        assert!(events[0].summary.contains("SSH"));
    }

    #[test]
    fn parse_ssh_accepted_auth() {
        let line = "Aug 26 10:00:01 host sshd[1234]: Accepted publickey for user from 1.2.3.4";
        let events = parse_auth_events(line);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].category, EventCategory::Ssh);
        assert_eq!(events[0].severity, Severity::Info);
    }

    #[test]
    fn parse_ssh_invalid_user() {
        let line = "Aug 26 10:00:01 host sshd[1234]: Invalid user admin from 1.2.3.4";
        let events = parse_auth_events(line);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].category, EventCategory::Ssh);
        assert_eq!(events[0].severity, Severity::Warning);
    }

    #[test]
    fn parse_sudo_failure() {
        let line = "Aug 26 10:00:01 host sudo: user : incorrect password ; TTY=pts/0";
        let events = parse_auth_events(line);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].category, EventCategory::Sudo);
        assert_eq!(events[0].severity, Severity::Error);
    }

    #[test]
    fn parse_sudo_command() {
        let line = "Aug 26 10:00:01 host sudo: user : command executed = /usr/bin/apt update";
        let events = parse_auth_events(line);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].category, EventCategory::Sudo);
        assert_eq!(events[0].severity, Severity::Info);
    }

    #[test]
    fn parse_session_opened() {
        let line = "Aug 26 10:00:01 host sshd[1234]: session opened for user by (uid=0)";
        let events = parse_auth_events(line);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].category, EventCategory::Session);
        assert_eq!(events[0].severity, Severity::Info);
    }

    #[test]
    fn parse_session_closed() {
        let line = "Aug 26 10:00:01 host sshd[1234]: session closed for user";
        let events = parse_auth_events(line);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].category, EventCategory::Session);
    }

    #[test]
    fn parse_generic_failed_auth() {
        let line = "Aug 26 10:00:01 host login[1234]: Failed password for invalid user test";
        let events = parse_auth_events(line);
        assert!(!events.is_empty());
        assert_eq!(events[0].severity, Severity::Error);
    }

    #[test]
    fn parse_generic_accepted_auth() {
        let line = "Aug 26 10:00:01 host login[1234]: Accepted password for user from 1.2.3.4";
        let events = parse_auth_events(line);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].severity, Severity::Info);
    }

    #[test]
    fn parse_invalid_user_generic() {
        let line = "Aug 26 10:00:01 host sshd[1234]: Invalid user testuser from 1.2.3.4";
        let events = parse_auth_events(line);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].severity, Severity::Warning);
    }

    // --- Kernel event parsing ---

    #[test]
    fn parse_kernel_denied() {
        let line = "Aug 26 10:00:01 host kernel: [12345.678] audit: denied comm=\"test\" exe=\"/bin/test\"";
        let events = parse_kernel_events(line);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].category, EventCategory::Kernel);
        assert_eq!(events[0].severity, Severity::Warning);
    }

    #[test]
    fn parse_kernel_firewall() {
        let line = "Aug 26 10:00:01 host kernel: [12345.678] iptables: DROP IN=eth0 SRC=1.2.3.4";
        let events = parse_kernel_events(line);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].category, EventCategory::Firewall);
    }

    #[test]
    fn parse_ufw_block() {
        let line = "Aug 26 10:00:01 host kernel: [12345.678] UFW BLOCK IN=eth0 SRC=1.2.3.4";
        let events = parse_kernel_events(line);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].category, EventCategory::Firewall);
    }

    #[test]
    fn parse_kernel_segfault() {
        let line = "Aug 26 10:00:01 host kernel: [12345.678] prog[1234]: segfault at 0 ip 0 sp 0";
        let events = parse_kernel_events(line);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].category, EventCategory::Kernel);
        assert_eq!(events[0].severity, Severity::Error);
    }

    // --- Severity classification ---

    #[test]
    fn severity_labels() {
        assert_eq!(Severity::Info.label(), "Info");
        assert_eq!(Severity::Warning.label(), "Warning");
        assert_eq!(Severity::Error.label(), "Error");
    }

    // --- Category classification ---

    #[test]
    fn category_labels() {
        assert_eq!(EventCategory::Auth.label(), "Auth");
        assert_eq!(EventCategory::Ssh.label(), "SSH");
        assert_eq!(EventCategory::Sudo.label(), "Sudo");
        assert_eq!(EventCategory::Firewall.label(), "Firewall");
        assert_eq!(EventCategory::Kernel.label(), "Kernel");
        assert_eq!(EventCategory::Session.label(), "Session");
        assert_eq!(EventCategory::Other.label(), "Other");
    }

    // --- Malformed input ---

    #[test]
    fn parse_empty_auth_log() {
        let events = parse_auth_events("");
        assert!(events.is_empty());
    }

    #[test]
    fn parse_malformed_auth_line() {
        let events = parse_auth_events("not a valid log line");
        assert!(events.is_empty());
    }

    #[test]
    fn parse_empty_kernel_log() {
        let events = parse_kernel_events("");
        assert!(events.is_empty());
    }

    #[test]
    fn parse_malformed_kernel_line() {
        let events = parse_kernel_events("random text with no structure");
        assert!(events.is_empty());
    }

    // --- EventLog ---

    #[test]
    fn event_log_push_and_dedup() {
        let mut log = EventLog::default();
        let event = SecurityEvent {
            timestamp: "Aug 26 10:00:01".into(),
            severity: Severity::Info,
            category: EventCategory::Auth,
            source: "sshd".into(),
            summary: "test".into(),
        };
        log.push(event.clone());
        assert_eq!(log.len(), 1);
        // Duplicate should be suppressed
        log.push(event);
        assert_eq!(log.len(), 1);
    }

    #[test]
    fn event_log_bounded_at_max() {
        let mut log = EventLog::default();
        for i in 0..MAX_EVENTS + 10 {
            log.push(SecurityEvent {
                timestamp: format!("ts{i}"),
                severity: Severity::Info,
                category: EventCategory::Auth,
                source: "test".into(),
                summary: format!("event {i}"),
            });
        }
        assert_eq!(log.len(), MAX_EVENTS);
    }

    #[test]
    fn event_log_filter_severity() {
        let mut log = EventLog::default();
        log.push(SecurityEvent {
            timestamp: "ts1".into(),
            severity: Severity::Info,
            category: EventCategory::Auth,
            source: "s".into(),
            summary: "test".into(),
        });
        log.push(SecurityEvent {
            timestamp: "ts2".into(),
            severity: Severity::Error,
            category: EventCategory::Auth,
            source: "s".into(),
            summary: "test2".into(),
        });
        let filter = EventFilter {
            severity: Some(Severity::Error),
            ..Default::default()
        };
        let indices = log.filter(&filter);
        assert_eq!(indices.len(), 1);
        assert_eq!(log.get(indices[0]).unwrap().severity, Severity::Error);
    }

    #[test]
    fn event_log_filter_category() {
        let mut log = EventLog::default();
        log.push(SecurityEvent {
            timestamp: "ts1".into(),
            severity: Severity::Info,
            category: EventCategory::Ssh,
            source: "s".into(),
            summary: "ssh event".into(),
        });
        log.push(SecurityEvent {
            timestamp: "ts2".into(),
            severity: Severity::Info,
            category: EventCategory::Sudo,
            source: "s".into(),
            summary: "sudo event".into(),
        });
        let filter = EventFilter {
            category: Some(EventCategory::Ssh),
            ..Default::default()
        };
        let indices = log.filter(&filter);
        assert_eq!(indices.len(), 1);
        assert_eq!(log.get(indices[0]).unwrap().category, EventCategory::Ssh);
    }

    #[test]
    fn event_log_filter_search() {
        let mut log = EventLog::default();
        log.push(SecurityEvent {
            timestamp: "ts1".into(),
            severity: Severity::Info,
            category: EventCategory::Auth,
            source: "sshd".into(),
            summary: "password accepted".into(),
        });
        log.push(SecurityEvent {
            timestamp: "ts2".into(),
            severity: Severity::Info,
            category: EventCategory::Auth,
            source: "login".into(),
            summary: "session opened".into(),
        });
        let filter = EventFilter {
            search: "password".into(),
            ..Default::default()
        };
        let indices = log.filter(&filter);
        assert_eq!(indices.len(), 1);
    }

    #[test]
    fn event_log_filter_case_insensitive() {
        let mut log = EventLog::default();
        log.push(SecurityEvent {
            timestamp: "ts1".into(),
            severity: Severity::Info,
            category: EventCategory::Ssh,
            source: "SSHD".into(),
            summary: "Accepted".into(),
        });
        let filter = EventFilter {
            search: "sshd".into(),
            ..Default::default()
        };
        let indices = log.filter(&filter);
        assert_eq!(indices.len(), 1);
    }

    #[test]
    fn event_log_filter_combined() {
        let mut log = EventLog::default();
        log.push(SecurityEvent {
            timestamp: "ts1".into(),
            severity: Severity::Error,
            category: EventCategory::Ssh,
            source: "sshd".into(),
            summary: "failed password".into(),
        });
        log.push(SecurityEvent {
            timestamp: "ts2".into(),
            severity: Severity::Info,
            category: EventCategory::Ssh,
            source: "sshd".into(),
            summary: "accepted key".into(),
        });
        log.push(SecurityEvent {
            timestamp: "ts3".into(),
            severity: Severity::Error,
            category: EventCategory::Auth,
            source: "login".into(),
            summary: "failed password".into(),
        });
        let filter = EventFilter {
            severity: Some(Severity::Error),
            category: Some(EventCategory::Ssh),
            search: "password".into(),
        };
        let indices = log.filter(&filter);
        assert_eq!(indices.len(), 1);
        assert_eq!(log.get(indices[0]).unwrap().category, EventCategory::Ssh);
    }

    // --- Sanitization ---

    #[test]
    fn sanitize_summary_short_no_sensitive() {
        let result = sanitize_summary("Aug 26 host: simple message", "fallback");
        assert_eq!(result, "Aug 26 host: simple message");
    }

    #[test]
    fn sanitize_summary_with_password() {
        let result = sanitize_summary("user password was incorrect", "Auth failed");
        assert_eq!(result, "Auth failed");
    }

    #[test]
    fn sanitize_summary_long_line() {
        let long = "a".repeat(200);
        let result = sanitize_summary(&long, "fallback");
        assert_eq!(result, "fallback");
    }

    // --- Truncation ---

    #[test]
    fn truncate_short() {
        assert_eq!(truncate_str("hello", 10), "hello");
    }

    #[test]
    fn truncate_long() {
        let result = truncate_str("hello world", 5);
        assert_eq!(result, "hell…");
    }

    // --- Hash ---

    #[test]
    fn hash_events_same_are_equal() {
        let e1 = SecurityEvent {
            timestamp: "ts".into(),
            severity: Severity::Info,
            category: EventCategory::Auth,
            source: "s".into(),
            summary: "test".into(),
        };
        let e2 = e1.clone();
        assert_eq!(hash_event(&e1), hash_event(&e2));
    }

    #[test]
    fn hash_events_different_are_not_equal() {
        let e1 = SecurityEvent {
            timestamp: "ts1".into(),
            severity: Severity::Info,
            category: EventCategory::Auth,
            source: "s".into(),
            summary: "test".into(),
        };
        let e2 = SecurityEvent {
            timestamp: "ts2".into(),
            severity: Severity::Info,
            category: EventCategory::Auth,
            source: "s".into(),
            summary: "test".into(),
        };
        assert_ne!(hash_event(&e1), hash_event(&e2));
    }

    // --- Filter defaults ---

    #[test]
    fn filter_default_matches_all() {
        let log = EventLog::default();
        let filter = EventFilter::default();
        assert!(log.filter(&filter).is_empty());
    }

    // --- EventLog defaults ---

    #[test]
    fn event_log_default() {
        let log = EventLog::default();
        assert!(log.is_empty());
        assert_eq!(log.len(), 0);
        assert!(log.events().is_empty());
    }

    // --- Multiple events from auth log ---

    #[test]
    fn parse_multiple_auth_events() {
        let content = "\
Aug 26 10:00:01 host sshd[1234]: Failed password for root from 1.2.3.4
Aug 26 10:00:02 host sshd[1235]: Accepted publickey for user from 5.6.7.8
Aug 26 10:00:03 host sudo: user : incorrect password
Aug 26 10:00:04 host sshd[1236]: session opened for user
Aug 26 10:00:05 host sshd[1237]: Invalid user admin from 9.9.9.9";
        let events = parse_auth_events(content);
        assert_eq!(events.len(), 5);
        // Check categories
        let categories: Vec<EventCategory> = events.iter().map(|e| e.category).collect();
        assert!(categories.contains(&EventCategory::Ssh));
        assert!(categories.contains(&EventCategory::Sudo));
        assert!(categories.contains(&EventCategory::Session));
    }
}
