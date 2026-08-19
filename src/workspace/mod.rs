use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

/// Axis along which a split pane divides its parent.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SplitAxis {
    Horizontal,
    Vertical,
}

/// The preset a workspace was created from (or `Blank` when the user created
/// an empty workspace). Presets are data-driven via [`PRESET_DEFINITIONS`],
/// so future presets can be added without touching the workspace manager.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkspacePreset {
    Blank,
    Coding,
    Networking,
    Cybersecurity,
    DevOps,
    System,
    #[serde(other)]
    Unknown,
}

impl WorkspacePreset {
    /// Presets the user can create a workspace from.
    pub const CREATABLE: [WorkspacePreset; 6] = [
        WorkspacePreset::Blank,
        WorkspacePreset::Coding,
        WorkspacePreset::Networking,
        WorkspacePreset::Cybersecurity,
        WorkspacePreset::DevOps,
        WorkspacePreset::System,
    ];

    pub fn label(self) -> &'static str {
        preset_definition(self)
            .map(|def| def.name)
            .unwrap_or("Workspace")
    }

    pub fn purpose(self) -> &'static str {
        preset_definition(self).map(|def| def.purpose).unwrap_or("")
    }
}

/// Data-driven description of a workspace preset.
pub struct PresetDefinition {
    pub preset: WorkspacePreset,
    pub name: &'static str,
    pub icon: &'static str,
    pub purpose: &'static str,
    /// Optional tilde path hint for the initial working directory (e.g.
    /// `~/projects`). Only used when the directory actually exists.
    pub dir_hint: Option<&'static str>,
    pub tab_titles: &'static [&'static str],
    pub pane_titles: &'static [&'static str],
    /// Split layout used when there is more than one pane.
    pub split: Option<SplitAxis>,
}

pub const PRESET_DEFINITIONS: &[PresetDefinition] = &[
    PresetDefinition {
        preset: WorkspacePreset::Coding,
        name: "Coding",
        icon: "</>",
        purpose: "software development",
        dir_hint: Some("~/projects"),
        tab_titles: &["project"],
        pane_titles: &["project"],
        split: None,
    },
    PresetDefinition {
        preset: WorkspacePreset::Networking,
        name: "Networking",
        icon: "net",
        purpose: "network diagnostics and learning",
        dir_hint: None,
        tab_titles: &["network"],
        pane_titles: &["commands", "monitor"],
        split: Some(SplitAxis::Vertical),
    },
    PresetDefinition {
        preset: WorkspacePreset::Cybersecurity,
        name: "Cybersecurity",
        icon: "sec",
        purpose: "security learning and diagnostics",
        dir_hint: None,
        tab_titles: &["security"],
        pane_titles: &["terminal", "tools"],
        split: Some(SplitAxis::Vertical),
    },
    PresetDefinition {
        preset: WorkspacePreset::DevOps,
        name: "DevOps",
        icon: "ops",
        purpose: "containers and infrastructure",
        dir_hint: None,
        tab_titles: &["infra"],
        pane_titles: &["containers", "workflow"],
        split: Some(SplitAxis::Horizontal),
    },
    PresetDefinition {
        preset: WorkspacePreset::System,
        name: "System",
        icon: "sys",
        purpose: "Linux and system administration",
        dir_hint: None,
        tab_titles: &["system"],
        pane_titles: &["terminal", "monitor"],
        split: Some(SplitAxis::Horizontal),
    },
    PresetDefinition {
        preset: WorkspacePreset::Blank,
        name: "Blank",
        icon: "new",
        purpose: "empty workspace",
        dir_hint: None,
        tab_titles: &["shell 1"],
        pane_titles: &["shell 1"],
        split: None,
    },
];

fn preset_definition(preset: WorkspacePreset) -> Option<&'static PresetDefinition> {
    PRESET_DEFINITIONS.iter().find(|def| def.preset == preset)
}

impl PresetDefinition {
    /// The default tab layout for this preset. Pane ids are 1-based per tab,
    /// matching the ids the runtime assigns on first use.
    fn build_tabs(&self, working_dir: &Path) -> Vec<SavedTab> {
        self.tab_titles
            .iter()
            .enumerate()
            .map(|(index, title)| {
                let id = index + 1;
                let panes = if self.pane_titles.len() <= 1 {
                    SavedPaneLayout::Single(SavedPane {
                        id: 1,
                        title: self.pane_titles[0].to_owned(),
                        working_dir: working_dir.to_path_buf(),
                    })
                } else {
                    SavedPaneLayout::Split {
                        axis: self.split.unwrap_or(SplitAxis::Vertical),
                        first: SavedPane {
                            id: 1,
                            title: self.pane_titles[0].to_owned(),
                            working_dir: working_dir.to_path_buf(),
                        },
                        second: SavedPane {
                            id: 2,
                            title: self.pane_titles[1].to_owned(),
                            working_dir: working_dir.to_path_buf(),
                        },
                    }
                };
                SavedTab {
                    id,
                    title: (*title).to_owned(),
                    active_pane: 1,
                    panes,
                }
            })
            .collect()
    }
}

/// Serialized description of one terminal pane.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct SavedPane {
    pub id: usize,
    pub title: String,
    /// Working directory the pane's shell was started in.
    pub working_dir: PathBuf,
}

impl Default for SavedPane {
    fn default() -> Self {
        Self {
            id: 1,
            title: String::new(),
            working_dir: PathBuf::new(),
        }
    }
}

/// Serialized description of a tab's pane layout (single or one split level,
/// mirroring the runtime `PaneLayout`).
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub enum SavedPaneLayout {
    Single(SavedPane),
    Split {
        axis: SplitAxis,
        first: SavedPane,
        second: SavedPane,
    },
}

/// Serialized description of one terminal tab inside a workspace.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct SavedTab {
    pub id: usize,
    pub title: String,
    pub active_pane: usize,
    pub panes: SavedPaneLayout,
}

impl Default for SavedTab {
    fn default() -> Self {
        Self {
            id: 1,
            title: "shell 1".to_owned(),
            active_pane: 1,
            panes: SavedPaneLayout::Single(SavedPane::default()),
        }
    }
}

/// A saved workspace: stable identity plus the metadata and layout needed to
/// restore it. Kept independent from the runtime so it can be persisted with
/// the existing config architecture.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Workspace {
    /// Stable identity. Never changes across renames.
    pub id: String,
    pub name: String,
    pub preset: WorkspacePreset,
    pub icon: String,
    /// Default working directory for new tabs/panes in this workspace.
    pub working_dir: PathBuf,
    pub tabs: Vec<SavedTab>,
    pub active_tab: usize,
    /// Free-form metadata (e.g. `duplicate_of`).
    pub metadata: HashMap<String, String>,
}

impl Default for Workspace {
    fn default() -> Self {
        Self {
            id: String::new(),
            name: "Workspace".to_owned(),
            preset: WorkspacePreset::Blank,
            icon: "new".to_owned(),
            working_dir: home_dir(),
            tabs: Vec::new(),
            active_tab: 0,
            metadata: HashMap::new(),
        }
    }
}

impl Workspace {
    pub fn from_preset(preset: WorkspacePreset, working_dir: PathBuf, id: String) -> Self {
        let Some(def) = preset_definition(preset) else {
            return Self::blank(preset.label().to_owned(), working_dir, id);
        };
        let tabs = def.build_tabs(&working_dir);
        Self {
            id,
            name: def.name.to_owned(),
            preset,
            icon: def.icon.to_owned(),
            working_dir,
            tabs,
            active_tab: 0,
            metadata: HashMap::new(),
        }
    }

    pub fn blank(name: String, working_dir: PathBuf, id: String) -> Self {
        Self::from_preset(WorkspacePreset::Blank, working_dir, id).with_name(name)
    }

    pub fn with_name(mut self, name: String) -> Self {
        self.name = name;
        self
    }
}

/// The five built-in preset workspaces, used to seed a fresh install.
pub fn default_workspaces(fallback_dir: &Path) -> Vec<Workspace> {
    PRESET_DEFINITIONS
        .iter()
        .filter(|def| def.preset != WorkspacePreset::Blank)
        .map(|def| {
            let dir = def
                .dir_hint
                .map(expand_hint)
                .filter(|path| path.is_dir())
                .unwrap_or_else(|| fallback_dir.to_path_buf());
            Workspace::from_preset(def.preset, dir, new_id("ws"))
        })
        .collect()
}

/// Lightweight unique id: monotonic counter plus timestamp. Stable across
/// renames by design (renames never touch `id`).
pub fn new_id(prefix: &str) -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    let counter = COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("{prefix}-{stamp:x}-{counter}")
}

pub fn home_dir() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/"))
}

fn expand_hint(hint: &str) -> PathBuf {
    if let Some(rest) = hint.strip_prefix("~/") {
        home_dir().join(rest)
    } else {
        PathBuf::from(hint)
    }
}

/// First candidate that exists and is a directory; otherwise `fallback`.
pub fn resolve_working_dir(ws_dir: &Path, config_dir: &Path) -> PathBuf {
    let home = home_dir();
    [ws_dir, config_dir, &home]
        .iter()
        .copied()
        .find(|path| path.is_dir())
        .map(|path| path.to_path_buf())
        .unwrap_or(home)
}

/// Resolves a user-typed directory (supports `~` and `~/...`). Invalid or
/// missing directories fall back safely; nothing is ever created.
pub fn resolve_dir_input(input: &str, fallback: &Path) -> PathBuf {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return fallback.to_path_buf();
    }
    let expanded = if trimmed == "~" {
        home_dir()
    } else if let Some(rest) = trimmed.strip_prefix("~/") {
        home_dir().join(rest)
    } else if let Some(rest) = trimmed.strip_prefix('~') {
        home_dir().join(rest)
    } else {
        PathBuf::from(trimmed)
    };
    if expanded.is_dir() {
        expanded
    } else {
        fallback.to_path_buf()
    }
}

/// A live workspace: persisted metadata plus the runtime tab list.
#[derive(Clone, Debug)]
pub struct WorkspaceRuntime<T> {
    pub data: Workspace,
    pub tabs: Vec<T>,
    pub active_tab: usize,
    pub next_tab_id: usize,
    pub next_pane_id: usize,
}

impl<T> WorkspaceRuntime<T> {
    pub fn new(data: Workspace, tabs: Vec<T>, next_tab_id: usize, next_pane_id: usize) -> Self {
        Self {
            data,
            tabs,
            active_tab: 0,
            next_tab_id,
            next_pane_id,
        }
    }
}

/// Owns the workspace list and implements all workspace operations. Generic
/// over the tab type so the manager stays independent of the terminal UI.
pub struct WorkspaceManager<T> {
    pub workspaces: Vec<WorkspaceRuntime<T>>,
    pub active: usize,
    pub default_id: String,
}

impl<T> WorkspaceManager<T> {
    pub fn new(workspaces: Vec<WorkspaceRuntime<T>>, active_id: &str, default_id: &str) -> Self {
        let default_id = if workspaces.iter().any(|ws| ws.data.id == default_id) {
            default_id.to_owned()
        } else {
            workspaces
                .first()
                .map(|ws| ws.data.id.clone())
                .unwrap_or_default()
        };
        let active = workspaces
            .iter()
            .position(|ws| ws.data.id == active_id)
            .or_else(|| workspaces.iter().position(|ws| ws.data.id == default_id))
            .unwrap_or(0);
        Self {
            workspaces,
            active,
            default_id,
        }
    }

    pub fn active(&self) -> Option<&WorkspaceRuntime<T>> {
        self.workspaces.get(self.active)
    }

    pub fn active_mut(&mut self) -> Option<&mut WorkspaceRuntime<T>> {
        self.workspaces.get_mut(self.active)
    }

    pub fn active_id(&self) -> Option<&str> {
        self.workspaces
            .get(self.active)
            .map(|ws| ws.data.id.as_str())
    }

    pub fn by_id(&self, id: &str) -> Option<usize> {
        self.workspaces.iter().position(|ws| ws.data.id == id)
    }

    pub fn switch_to(&mut self, index: usize) {
        if index < self.workspaces.len() {
            self.active = index;
        }
    }

    pub fn switch_next(&mut self) {
        if !self.workspaces.is_empty() {
            self.active = (self.active + 1) % self.workspaces.len();
        }
    }

    pub fn switch_previous(&mut self) {
        if self.workspaces.is_empty() {
            return;
        }
        self.active = if self.active == 0 {
            self.workspaces.len() - 1
        } else {
            self.active - 1
        };
    }

    /// Adds a workspace and makes it active.
    pub fn create(&mut self, runtime: WorkspaceRuntime<T>) -> usize {
        self.workspaces.push(runtime);
        self.active = self.workspaces.len() - 1;
        self.active
    }

    pub fn rename(&mut self, id: &str, name: &str) -> bool {
        let name = name.trim();
        if name.is_empty() {
            return false;
        }
        let Some(index) = self.by_id(id) else {
            return false;
        };
        self.workspaces[index].data.name = name.to_owned();
        true
    }

    /// Copies `index` (new identity, ` (copy)` suffix) and activates the copy.
    /// The copy keeps the source layout but gets fresh runtime sessions via
    /// `build`.
    pub fn duplicate(
        &mut self,
        index: usize,
        build: impl FnOnce(&Workspace) -> WorkspaceRuntime<T>,
    ) -> usize {
        let Some(source) = self.workspaces.get(index) else {
            return index;
        };
        let mut data = source.data.clone();
        data.id = new_id("ws");
        data.name = format!("{} (copy)", data.name);
        data.metadata
            .insert("duplicate_of".to_owned(), source.data.id.clone());
        let runtime = build(&data);
        let insert_at = index + 1;
        self.workspaces.insert(insert_at, runtime);
        self.active = insert_at;
        insert_at
    }

    /// Deletes a workspace. Never leaves the list empty: deleting the last
    /// workspace replaces it with a fresh blank one built via `build`.
    /// Deleting the active workspace switches to a neighbor.
    pub fn delete(&mut self, index: usize, build: impl FnOnce(&Workspace) -> WorkspaceRuntime<T>) {
        if self.workspaces.len() <= 1 {
            let data =
                Workspace::blank(format!("Workspace {}", index + 1), home_dir(), new_id("ws"));
            let runtime = build(&data);
            self.workspaces[0] = runtime;
            self.active = 0;
            self.default_id = self.workspaces[0].data.id.clone();
            return;
        }

        let removed_id = self.workspaces[index].data.id.clone();
        self.workspaces.remove(index);
        if index < self.active {
            self.active -= 1;
        }
        self.active = self.active.min(self.workspaces.len() - 1);
        if removed_id == self.default_id {
            self.default_id = self.workspaces[self.active].data.id.clone();
        }
    }

    pub fn move_left(&mut self, index: usize) {
        if index > 0 && index < self.workspaces.len() {
            self.workspaces.swap(index, index - 1);
            if self.active == index {
                self.active -= 1;
            } else if self.active == index - 1 {
                self.active += 1;
            }
        }
    }

    pub fn move_right(&mut self, index: usize) {
        if index + 1 < self.workspaces.len() {
            self.workspaces.swap(index, index + 1);
            if self.active == index {
                self.active += 1;
            } else if self.active == index + 1 {
                self.active -= 1;
            }
        }
    }

    pub fn set_default(&mut self, index: usize) {
        if let Some(ws) = self.workspaces.get(index) {
            self.default_id = ws.data.id.clone();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_workspace(name: &str) -> Workspace {
        Workspace::blank(name.to_owned(), home_dir(), new_id("ws"))
    }

    fn runtime(ws: Workspace) -> WorkspaceRuntime<()> {
        WorkspaceRuntime::new(ws, vec![(), ()], 1, 1)
    }

    fn build(ws: &Workspace) -> WorkspaceRuntime<()> {
        runtime(ws.clone())
    }

    #[test]
    fn presets_exist_and_have_layouts() {
        for preset in WorkspacePreset::CREATABLE {
            let ws = Workspace::from_preset(preset, home_dir(), new_id("ws"));
            assert!(!ws.name.is_empty(), "{preset:?} has a name");
            assert!(!ws.tabs.is_empty(), "{preset:?} has tabs");
            for tab in &ws.tabs {
                match &tab.panes {
                    SavedPaneLayout::Single(pane) => assert_eq!(pane.id, 1),
                    SavedPaneLayout::Split { first, second, .. } => {
                        assert!(first.id != second.id);
                    }
                }
            }
        }
    }

    #[test]
    fn default_workspaces_are_unique_and_preseted() {
        let dirs = default_workspaces(&home_dir());
        assert_eq!(dirs.len(), 5);
        let mut ids = dirs.iter().map(|ws| ws.id.as_str()).collect::<Vec<_>>();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), 5, "workspace ids must be unique");
        for preset in [
            WorkspacePreset::Coding,
            WorkspacePreset::Networking,
            WorkspacePreset::Cybersecurity,
            WorkspacePreset::DevOps,
            WorkspacePreset::System,
        ] {
            assert!(
                dirs.iter().any(|ws| ws.preset == preset),
                "missing preset {preset:?}"
            );
        }
    }

    #[test]
    fn rename_keeps_identity() {
        let mut manager = WorkspaceManager::new(vec![runtime(test_workspace("Coding"))], "", "");
        let id = manager.workspaces[0].data.id.clone();
        assert!(manager.rename(&id, "Rust Development"));
        assert_eq!(manager.workspaces[0].data.name, "Rust Development");
        assert_eq!(manager.workspaces[0].data.id, id);
    }

    #[test]
    fn rename_rejects_empty_names() {
        let mut manager = WorkspaceManager::new(vec![runtime(test_workspace("Coding"))], "", "");
        let id = manager.workspaces[0].data.id.clone();
        assert!(!manager.rename(&id, "   "));
    }

    #[test]
    fn duplicate_gets_fresh_identity() {
        let mut manager = WorkspaceManager::new(vec![runtime(test_workspace("Coding"))], "", "");
        let original_id = manager.workspaces[0].data.id.clone();
        let copy_index = manager.duplicate(0, build);
        assert_eq!(copy_index, 1);
        assert_eq!(manager.active, 1);
        assert_ne!(manager.workspaces[1].data.id, original_id);
        assert!(manager.workspaces[1].data.name.contains("copy"));
        assert_eq!(
            manager.workspaces[1].data.metadata["duplicate_of"],
            original_id
        );
    }

    #[test]
    fn delete_keeps_at_least_one_workspace() {
        let mut manager = WorkspaceManager::new(vec![runtime(test_workspace("Only"))], "", "");
        manager.delete(0, build);
        assert_eq!(manager.workspaces.len(), 1);
        assert_eq!(manager.active, 0);
        assert!(!manager.workspaces[0].data.name.is_empty());
    }

    #[test]
    fn deleting_active_switches_to_neighbor() {
        let mut manager = WorkspaceManager::new(
            vec![
                runtime(test_workspace("A")),
                runtime(test_workspace("B")),
                runtime(test_workspace("C")),
            ],
            "",
            "",
        );
        manager.switch_to(2);
        manager.delete(2, build);
        assert_eq!(manager.workspaces.len(), 2);
        assert_eq!(manager.active, 1);
        assert_eq!(manager.workspaces[manager.active].data.name, "B");
    }

    #[test]
    fn switch_next_and_previous_wrap() {
        let mut manager = WorkspaceManager::new(
            vec![runtime(test_workspace("A")), runtime(test_workspace("B"))],
            "",
            "",
        );
        manager.switch_next();
        assert_eq!(manager.active, 1);
        manager.switch_next();
        assert_eq!(manager.active, 0);
        manager.switch_previous();
        assert_eq!(manager.active, 1);
    }

    #[test]
    fn reorder_moves_workspaces() {
        let mut manager = WorkspaceManager::new(
            vec![
                runtime(test_workspace("A")),
                runtime(test_workspace("B")),
                runtime(test_workspace("C")),
            ],
            "",
            "",
        );
        manager.move_right(0);
        assert_eq!(manager.workspaces[0].data.name, "B");
        assert_eq!(manager.workspaces[1].data.name, "A");
        manager.move_left(2);
        assert_eq!(manager.workspaces[1].data.name, "C");
        assert_eq!(manager.workspaces[2].data.name, "A");
    }

    #[test]
    fn default_workspace_falls_back_when_deleted() {
        let mut manager = WorkspaceManager::new(
            vec![runtime(test_workspace("A")), runtime(test_workspace("B"))],
            "",
            "",
        );
        manager.set_default(0);
        let original_default = manager.default_id.clone();
        manager.delete(0, build);
        assert_ne!(manager.default_id, original_default);
        assert!(manager.by_id(&manager.default_id).is_some());
    }

    #[test]
    fn invalid_dirs_fall_back_safely() {
        let missing = PathBuf::from("/definitely/not/a/real/dir/orbit");
        let home = home_dir();
        assert_eq!(resolve_working_dir(&missing, &home), home);
        assert_eq!(resolve_working_dir(&missing, &missing), home);
        assert_eq!(resolve_dir_input("~/does-not-exist-orbit", &home), home);
        assert_eq!(resolve_dir_input("   ", &home), home);
        assert!(resolve_dir_input("~", &home).is_dir());
    }

    #[test]
    fn active_workspace_restored_by_id() {
        let mut manager = WorkspaceManager::new(
            vec![runtime(test_workspace("A")), runtime(test_workspace("B"))],
            "",
            "",
        );
        let id = manager.workspaces[1].data.id.clone();
        manager.switch_to(1);
        let restored = WorkspaceManager::new(manager.workspaces, &id, "");
        assert_eq!(restored.workspaces[restored.active].data.id, id);
    }
}
