//! Bounded command history that promotes repeated entries instead of duplicating them.

use std::collections::VecDeque;

#[derive(Debug, Default)]
pub struct CommandHistory { entries: VecDeque<String>, capacity: usize }

impl CommandHistory {
    pub fn new(capacity: usize) -> Result<Self, &'static str> {
        if capacity == 0 { Err("history capacity must be positive") } else { Ok(Self { entries: VecDeque::new(), capacity }) }
    }

    pub fn record(&mut self, command: impl Into<String>) {
        let command = command.into();
        if command.trim().is_empty() { return; }
        if let Some(index) = self.entries.iter().position(|entry| entry == &command) {
            self.entries.remove(index);
        }
        self.entries.push_front(command);
        while self.entries.len() > self.capacity { self.entries.pop_back(); }
    }

    pub fn latest(&self) -> Option<&str> { self.entries.front().map(String::as_str) }
    pub fn iter(&self) -> impl DoubleEndedIterator<Item = &str> { self.entries.iter().map(String::as_str) }
    pub fn len(&self) -> usize { self.entries.len() }
}
