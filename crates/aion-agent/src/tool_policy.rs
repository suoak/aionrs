use std::collections::BTreeSet;

/// Runtime authorization policy for tools registered with an agent engine.
///
/// The policy is enforced both when tool definitions are sent to the model and
/// immediately before a requested tool is executed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum ToolPolicy {
    /// Every registered tool is available.
    #[default]
    Unrestricted,
    /// Only tools whose exact names are present in the set are available.
    AllowOnly(BTreeSet<String>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ToolGateDenial {
    Policy,
    Capability,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ToolGateDecision {
    Allow,
    Deny(ToolGateDenial),
}

impl ToolGateDecision {
    /// Combine gates monotonically: once denied, no later gate may re-allow execution.
    pub(crate) fn and(self, next: Self) -> Self {
        match self {
            Self::Deny(_) => self,
            Self::Allow => next,
        }
    }

    pub(crate) fn is_denied(self) -> bool {
        matches!(self, Self::Deny(_))
    }
}

impl ToolPolicy {
    pub fn allow_only<I, S>(tool_names: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Self::AllowOnly(tool_names.into_iter().map(Into::into).collect())
    }

    pub fn allows(&self, tool_name: &str) -> bool {
        match self {
            Self::Unrestricted => true,
            Self::AllowOnly(tool_names) => tool_names.contains(tool_name),
        }
    }

    pub(crate) fn decision(&self, tool_name: &str) -> ToolGateDecision {
        if self.allows(tool_name) {
            ToolGateDecision::Allow
        } else {
            ToolGateDecision::Deny(ToolGateDenial::Policy)
        }
    }
}

#[cfg(test)]
#[path = "tool_policy_test.rs"]
mod tool_policy_test;
