use std::collections::HashSet;
use std::io::{self, BufRead, Write};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

pub struct ToolConfirmer {
    /// Shared session-wide auto-approval flag. Embedding hosts flip it when the
    /// user changes permission mode mid-session (YOLO ⇄ default) so the change
    /// applies to the next tool call instead of being frozen at bootstrap.
    auto_approve: Arc<AtomicBool>,
    allow_list: HashSet<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfirmResult {
    Approved,
    Denied,
    Quit,
}

impl ToolConfirmer {
    pub fn new(auto_approve: bool, allow_list: Vec<String>) -> Self {
        Self {
            auto_approve: Arc::new(AtomicBool::new(auto_approve)),
            allow_list: allow_list.into_iter().collect(),
        }
    }

    /// Replace the auto-approve flag with a host-shared one.
    pub fn share_auto_approve(&mut self, flag: Arc<AtomicBool>) {
        self.auto_approve = flag;
    }

    /// The shared auto-approve flag (clone of the `Arc`).
    pub fn auto_approve_flag(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.auto_approve)
    }

    /// Set session-wide auto-approval at runtime.
    pub fn set_auto_approve(&self, enabled: bool) {
        self.auto_approve.store(enabled, Ordering::SeqCst);
    }

    /// Returns whether auto-approve is enabled
    pub fn is_auto_approve(&self) -> bool {
        self.auto_approve.load(Ordering::SeqCst)
    }

    /// Add a tool name to the allow list at runtime.
    /// Used by skill context modifiers to grant auto-approval for specified tools.
    pub fn add_to_allow_list(&mut self, name: &str) {
        self.allow_list.insert(name.to_string());
    }

    /// Check if the tool needs confirmation. Returns the user's decision.
    pub fn check(&mut self, tool_name: &str, tool_input_display: &str) -> ConfirmResult {
        if self.is_auto_approve() || self.allow_list.contains(tool_name) {
            return ConfirmResult::Approved;
        }

        eprint!(
            "\n[tool] {}({})\nAllow? [y]es / [n]o / [a]lways / [q]uit > ",
            tool_name, tool_input_display
        );
        io::stderr().flush().unwrap();

        let mut input = String::new();
        if io::stdin().lock().read_line(&mut input).is_err() {
            return ConfirmResult::Denied;
        }

        match input.trim().to_lowercase().as_str() {
            "y" | "yes" | "" => ConfirmResult::Approved,
            "a" | "always" => {
                self.allow_list.insert(tool_name.to_string());
                ConfirmResult::Approved
            }
            "q" | "quit" => ConfirmResult::Quit,
            _ => ConfirmResult::Denied,
        }
    }
}

#[cfg(test)]
#[path = "confirm_test.rs"]
mod confirm_test;
