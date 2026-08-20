use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;

use super::config::McpServerConfig;
use super::manager::McpManager;
use aion_protocol::events::ToolCategory;
use aion_tools::{Tool, ToolExecutionOutput};
use aion_types::message::{ContentBlock, ImageUrl};
use aion_types::tool::{JsonSchema, ToolResult};

/// Wraps an MCP server tool as a local Tool trait implementation.
/// Uses naming convention "mcp__{server}__{tool}" when collisions exist,
/// otherwise uses the tool's original name.
pub struct McpToolProxy {
    /// Display name used for registration (may be prefixed)
    display_name: String,
    /// Original tool name on the MCP server
    tool_name: String,
    /// Server this tool belongs to
    server_name: String,
    description: String,
    input_schema: JsonSchema,
    manager: Arc<McpManager>,
    /// Whether this tool's schema should be deferred (sent as name-only stub).
    deferred: bool,
}

impl McpToolProxy {
    pub fn new(
        display_name: String,
        tool_name: String,
        server_name: String,
        description: String,
        input_schema: JsonSchema,
        manager: Arc<McpManager>,
        deferred: bool,
    ) -> Self {
        Self {
            display_name,
            tool_name,
            server_name,
            description,
            input_schema,
            manager,
            deferred,
        }
    }
}

#[async_trait]
impl Tool for McpToolProxy {
    fn name(&self) -> &str {
        &self.display_name
    }

    fn description(&self) -> &str {
        &self.description
    }

    fn input_schema(&self) -> JsonSchema {
        self.input_schema.clone()
    }

    fn is_concurrency_safe(&self, _input: &Value) -> bool {
        // MCP tools are assumed not concurrency-safe
        false
    }

    fn is_deferred(&self) -> bool {
        self.deferred
    }

    async fn execute(&self, input: Value) -> ToolResult {
        let output = self.execute_with_follow_up(input).await;
        output.result
    }

    async fn execute_with_follow_up(&self, input: Value) -> ToolExecutionOutput {
        match self
            .manager
            .call_tool_result(&self.server_name, &self.tool_name, input)
            .await
        {
            Ok(result) => {
                let is_error = result.is_error;
                let mut text = Vec::new();
                let mut follow_up_blocks = Vec::new();
                let content_blocks = result
                    .content
                    .iter()
                    .filter_map(|content| serde_json::to_value(content).ok())
                    .collect();
                for content in result.content {
                    match content {
                        super::protocol::McpContent::Text { text: value } => text.push(value),
                        super::protocol::McpContent::Image { data, mime_type } => {
                            follow_up_blocks.push(ContentBlock::Image {
                                image_url: ImageUrl {
                                    url: format!("data:{mime_type};base64,{data}"),
                                },
                            });
                        }
                        super::protocol::McpContent::Resource { resource } => {
                            text.push(serde_json::to_string(&resource).unwrap_or_else(|_| "[resource]".into()));
                        }
                    }
                }
                ToolExecutionOutput {
                    result: ToolResult {
                        content: text.join("\n"),
                        is_error,
                    },
                    follow_up_blocks: if is_error { Vec::new() } else { follow_up_blocks },
                    content_blocks: Some(content_blocks),
                    structured_content: result.structured_content,
                    error_code: is_error.then_some(aion_types::tool::ToolExecutionErrorCode::ExecutionFailed),
                    truncation: None,
                }
            }
            Err(error) => ToolExecutionOutput {
                result: ToolResult {
                    content: format!("MCP tool error: {error}"),
                    is_error: true,
                },
                follow_up_blocks: Vec::new(),
                content_blocks: None,
                structured_content: None,
                error_code: Some(aion_types::tool::ToolExecutionErrorCode::ExecutionFailed),
                truncation: None,
            },
        }
    }

    fn category(&self) -> ToolCategory {
        ToolCategory::Mcp
    }

    fn describe(&self, input: &Value) -> String {
        format!(
            "MCP {}/{}: {}",
            self.server_name,
            self.tool_name,
            serde_json::to_string(input).unwrap_or_default()
        )
    }
}

/// Register all MCP tools into the tool registry, handling name collisions.
///
/// Strategy:
/// - If tool name doesn't collide with built-in or other MCP tools → use as-is
/// - If collision detected → prefix with "mcp__{server_name}__"
///
/// Each tool's deferred flag is read from the server's config:
/// `McpServerConfig::deferred` — defaults to `true` when absent.
pub fn register_mcp_tools(
    registry: &mut aion_tools::registry::ToolRegistry,
    manager: &Arc<McpManager>,
    builtin_names: &[String],
    server_configs: &HashMap<String, McpServerConfig>,
) {
    let all_tools = manager.all_tools();

    // Determine which names need prefixing
    for (server_name, tool_def) in &all_tools {
        let original_name = &tool_def.name;

        // Check collision with built-in tools
        let collides_builtin = builtin_names.iter().any(|n| n == original_name);

        // Check collision with other MCP servers' tools
        let cross_server_collision = manager.tool_name_count(original_name) > 1;

        let display_name = if collides_builtin || cross_server_collision {
            format!("mcp__{}_{}", server_name, original_name)
        } else {
            original_name.clone()
        };

        // MCP tools are deferred by default; server config can override.
        let deferred = server_configs
            .get(*server_name)
            .and_then(|c| c.deferred)
            .unwrap_or(true);

        let proxy = McpToolProxy::new(
            display_name,
            original_name.clone(),
            server_name.to_string(),
            tool_def.description.clone().unwrap_or_default(),
            tool_def.input_schema.clone(),
            Arc::clone(manager),
            deferred,
        );

        registry.register(Box::new(proxy));
    }
}

/// Register tools from a single newly-connected MCP server.
/// Uses the same collision-detection logic as `register_mcp_tools`.
pub fn register_single_server_tools(
    registry: &mut aion_tools::registry::ToolRegistry,
    manager: &Arc<McpManager>,
    server_name: &str,
    builtin_names: &[String],
    deferred: bool,
) {
    let all_tools = manager.all_tools();
    let server_tools: Vec<_> = all_tools.iter().filter(|(sn, _)| *sn == server_name).collect();

    for (_, tool_def) in &server_tools {
        let original_name = &tool_def.name;
        let collides_builtin = builtin_names.iter().any(|n| n == original_name);
        let cross_server_collision = manager.tool_name_count(original_name) > 1;

        let display_name = if collides_builtin || cross_server_collision {
            format!("mcp__{}_{}", server_name, original_name)
        } else {
            original_name.clone()
        };

        let proxy = McpToolProxy::new(
            display_name,
            original_name.clone(),
            server_name.to_string(),
            tool_def.description.clone().unwrap_or_default(),
            tool_def.input_schema.clone(),
            Arc::clone(manager),
            deferred,
        );

        registry.register(Box::new(proxy));
    }
}

#[cfg(test)]
#[path = "tool_proxy_test.rs"]
mod tool_proxy_test;
