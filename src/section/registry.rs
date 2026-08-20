use super::placeholder::PlaceholderSection;
use super::system::SystemSection;
use super::terminal_section::TerminalSection;
use super::{Section, SectionId};
use crate::config::TerminalConfig;

/// Central owner of every built-in ORBIT section.
///
/// The registry is the single place that knows which sections exist, in
/// which order, and which one is active. Section names and metadata are
/// never scattered through the application — UI reads them from
/// [`SectionId::descriptor`] via the live section.
pub struct SectionRegistry {
    sections: Vec<Box<dyn Section>>,
    active: usize,
}

impl SectionRegistry {
    /// Builds the registry with a real Terminal section, the placeholder
    /// sections for the remaining tool categories, and the live System
    /// section, restoring the last active section from config. Invalid
    /// persisted ids fall back to Terminal.
    pub fn new(config: &TerminalConfig) -> Self {
        Self::with_terminal_builder(config, |config| Box::new(TerminalSection::new(config)))
    }

    /// Same as [`Self::new`] but lets callers (e.g. tests) provide a
    /// terminal builder that does not spawn a PTY.
    pub fn with_terminal_builder(
        config: &TerminalConfig,
        build_terminal: impl FnOnce(&TerminalConfig) -> Box<dyn Section>,
    ) -> Self {
        let mut sections: Vec<Box<dyn Section>> = Vec::new();
        sections.push(build_terminal(config));
        for id in [
            SectionId::Coding,
            SectionId::Networking,
            SectionId::Cybersecurity,
            SectionId::DevOps,
        ] {
            sections.push(Box::new(PlaceholderSection::new(id)));
        }
        sections.push(Box::new(SystemSection::new()));

        let active = SectionId::from_config_id(&config.active_section)
            .index()
            .min(sections.len().saturating_sub(1));

        Self { sections, active }
    }

    /// Registers an additional built-in section (used by future phases).
    /// New sections are appended and never disturb existing ones.
    #[allow(dead_code)] // future extension point
    pub fn register(&mut self, section: Box<dyn Section>) -> usize {
        self.sections.push(section);
        self.sections.len() - 1
    }

    pub fn len(&self) -> usize {
        self.sections.len()
    }

    pub fn active_index(&self) -> usize {
        self.active
    }

    pub fn active_id(&self) -> SectionId {
        self.sections
            .get(self.active)
            .map(|section| section.id())
            .unwrap_or(SectionId::Terminal)
    }

    pub fn active(&self) -> &dyn Section {
        self.sections
            .get(self.active)
            .map(|section| section.as_ref())
            .expect("registry always has an active section")
    }

    pub fn active_mut(&mut self) -> &mut dyn Section {
        self.sections
            .get_mut(self.active)
            .map(|section| section.as_mut())
            .expect("registry always has an active section")
    }

    pub fn section(&self, index: usize) -> Option<&(dyn Section + '_)> {
        self.sections.get(index).map(|section| section.as_ref())
    }

    /// Switches to `id` if it is registered. Invalid ids are ignored, so a
    /// section switch can never leave the registry in a broken state.
    pub fn switch_to(&mut self, id: SectionId) -> bool {
        if let Some(position) = self.sections.iter().position(|section| section.id() == id) {
            self.active = position;
            true
        } else {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::TerminalConfig;

    fn placeholder_terminal(_config: &TerminalConfig) -> Box<dyn Section> {
        Box::new(PlaceholderSection::new(SectionId::Terminal))
    }

    fn registry(active_section: &str) -> SectionRegistry {
        let mut config = TerminalConfig::default();
        config.active_section = active_section.to_owned();
        SectionRegistry::with_terminal_builder(&config, placeholder_terminal)
    }

    #[test]
    fn builtins_exist_in_canonical_order() {
        let registry = registry("");
        assert_eq!(registry.len(), 6);
        let ids: Vec<SectionId> = (0..registry.len())
            .map(|index| registry.section(index).unwrap().id())
            .collect();
        assert_eq!(ids, SectionId::ALL.to_vec());
    }

    #[test]
    fn terminal_is_the_default_section() {
        let registry = registry("");
        assert_eq!(registry.active_id(), SectionId::Terminal);
        assert_eq!(registry.active_index(), 0);
    }

    #[test]
    fn empty_or_unknown_config_id_falls_back_to_terminal() {
        for invalid in ["", "nope", "workspace-1", " Coding"] {
            let registry = registry(invalid);
            assert_eq!(
                registry.active_id(),
                SectionId::Terminal,
                "config id {invalid:?} must fall back to Terminal"
            );
        }
    }

    #[test]
    fn persisted_section_id_is_restored() {
        for id in SectionId::ALL {
            let registry = registry(id.config_id());
            assert_eq!(registry.active_id(), id);
        }
    }

    #[test]
    fn switching_sections_round_trips() {
        let mut registry = registry("");
        assert!(registry.switch_to(SectionId::Networking));
        assert_eq!(registry.active_id(), SectionId::Networking);
        assert!(registry.switch_to(SectionId::System));
        assert_eq!(registry.active_id(), SectionId::System);
        assert!(registry.switch_to(SectionId::Terminal));
        assert_eq!(registry.active_id(), SectionId::Terminal);
    }

    #[test]
    fn switching_through_every_section_keeps_terminal_first() {
        let mut registry = registry("");
        for id in SectionId::ALL {
            assert!(registry.switch_to(id));
            assert_eq!(registry.active_id(), id);
            assert_eq!(registry.section(0).unwrap().id(), SectionId::Terminal);
        }
    }

    #[test]
    fn switch_to_restores_sections() {
        let mut registry = registry("devops");
        assert_eq!(registry.active_id(), SectionId::DevOps);
        assert!(registry.switch_to(SectionId::Networking));
        assert_eq!(registry.active_id(), SectionId::Networking);
        assert!(registry.switch_to(SectionId::Terminal));
        assert_eq!(registry.active_id(), SectionId::Terminal);
    }

    #[test]
    fn config_ids_round_trip_through_parsing() {
        for id in SectionId::ALL {
            assert_eq!(SectionId::from_config_id(id.config_id()), id);
        }
    }

    #[test]
    fn descriptors_match_section_order() {
        for id in SectionId::ALL {
            let descriptor = id.descriptor();
            assert!(!descriptor.name.is_empty());
            assert!(!descriptor.icon.is_empty());
            assert!(!descriptor.description.is_empty());
            assert!(descriptor.shortcut.starts_with("Ctrl+"));
        }
        let registry = registry("");
        for (index, id) in SectionId::ALL.iter().enumerate() {
            let section = registry.section(index).unwrap();
            assert_eq!(section.id(), *id);
        }
    }

    #[test]
    fn registering_a_new_section_does_not_disturb_builtins() {
        let mut registry = registry("");
        let index = registry.register(Box::new(PlaceholderSection::new(SectionId::System)));
        assert_eq!(index, registry.len() - 1);
        assert_eq!(registry.section(0).unwrap().id(), SectionId::Terminal);
        assert_eq!(registry.active_id(), SectionId::Terminal);
        // A duplicate built-in id is appended, never replaces the first.
        assert!(registry.switch_to(SectionId::System));
        assert_eq!(registry.active_index(), 5);
    }
}
