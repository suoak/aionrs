//! Built-in tool names that external tool sources (MCP servers, plugins)
//! must never claim.
//!
//! [`crate::registry::ToolRegistry::get`] resolves the *first* tool with a
//! given name, and bootstrap connects MCP servers before it registers the
//! agent-level tools (`Skill`, `Spawn`, `ToolSearch`, plan tools). Without a
//! single reserved table an MCP server exposing a tool called `Skill` or
//! `Spawn` would shadow the built-in. Every name here is reserved whether or
//! not the built-in is registered in a particular runtime.

/// Unified table of reserved built-in tool names.
pub const RESERVED_TOOL_NAMES: &[&str] = &[
    // File / shell built-ins.
    "Read",
    "Write",
    "Edit",
    "ExecCommand",
    "Grep",
    "Glob",
    "ViewImage",
    // Agent control-plane tools.
    "Skill",
    "Spawn",
    "ToolSearch",
    "EnterPlanMode",
    "ExitPlanMode",
];

/// Whether `name` is a reserved built-in tool name.
///
/// Matching is case-insensitive so `skill` / `SPAWN` cannot be used to
/// impersonate a built-in towards case-insensitive models or hosts.
pub fn is_reserved_tool_name(name: &str) -> bool {
    RESERVED_TOOL_NAMES
        .iter()
        .any(|reserved| reserved.eq_ignore_ascii_case(name.trim()))
}

#[cfg(test)]
#[path = "reserved_test.rs"]
mod reserved_test;
