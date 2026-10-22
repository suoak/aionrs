use aion_types::tool::ToolDef;

use crate::Tool;
use crate::reserved::is_reserved_tool_name;

/// Why an external tool was refused by [`ToolRegistry::register_external`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RegisterError {
    /// The name is a reserved built-in tool name.
    ReservedName(String),
    /// A tool with this name is already registered.
    Duplicate(String),
}

impl std::fmt::Display for RegisterError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ReservedName(name) => write!(f, "tool name '{name}' is reserved for a built-in tool"),
            Self::Duplicate(name) => write!(f, "a tool named '{name}' is already registered"),
        }
    }
}

impl std::error::Error for RegisterError {}

pub struct ToolRegistry {
    tools: Vec<Box<dyn Tool>>,
}

impl Default for ToolRegistry {
    fn default() -> Self {
        Self::new()
    }
}
impl ToolRegistry {
    pub fn new() -> Self {
        Self { tools: Vec::new() }
    }

    /// Register a built-in tool.
    ///
    /// External sources (MCP servers, plugins) must use
    /// [`Self::register_external`] so they cannot claim built-in names.
    pub fn register(&mut self, tool: Box<dyn Tool>) {
        self.tools.push(tool);
    }

    /// Register a tool from an external source (MCP server, plugin).
    ///
    /// Refuses reserved built-in names (see [`crate::reserved`]) and names
    /// that are already registered, so an external tool can never shadow or
    /// replace a built-in such as `Skill` or `Spawn`.
    pub fn register_external(&mut self, tool: Box<dyn Tool>) -> Result<(), RegisterError> {
        let name = tool.name().to_string();
        if is_reserved_tool_name(&name) {
            return Err(RegisterError::ReservedName(name));
        }
        if self.get(&name).is_some() {
            return Err(RegisterError::Duplicate(name));
        }
        self.tools.push(tool);
        Ok(())
    }

    /// Whether any registered tool is deferred (schema loaded on demand via
    /// `ToolSearch`).
    pub fn has_deferred_tools(&self) -> bool {
        self.tools.iter().any(|t| t.is_deferred())
    }

    /// Find a tool by name
    pub fn get(&self, name: &str) -> Option<&dyn Tool> {
        self.tools.iter().find(|t| t.name() == name).map(|t| t.as_ref())
    }

    /// Get all registered tool names
    pub fn tool_names(&self) -> Vec<String> {
        self.tools.iter().map(|t| t.name().to_string()).collect()
    }

    /// Generate API tool definitions for all registered tools
    pub fn to_tool_defs(&self) -> Vec<ToolDef> {
        self.tools
            .iter()
            .map(|t| ToolDef {
                name: t.name().to_string(),
                description: t.description().to_string(),
                input_schema: t.input_schema(),
                deferred: t.is_deferred(),
            })
            .collect()
    }

    /// Generate API tool definitions for tools matching a predicate.
    ///
    /// Used by plan mode to restrict the tool set sent to the LLM.
    pub fn to_tool_defs_filtered<F>(&self, filter: F) -> Vec<ToolDef>
    where
        F: Fn(&dyn Tool) -> bool,
    {
        self.tools
            .iter()
            .filter(|t| filter(t.as_ref()))
            .map(|t| ToolDef {
                name: t.name().to_string(),
                description: t.description().to_string(),
                input_schema: t.input_schema(),
                deferred: t.is_deferred(),
            })
            .collect()
    }
}

#[cfg(test)]
#[path = "registry_test.rs"]
mod registry_test;
