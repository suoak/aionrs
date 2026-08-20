use std::sync::{Arc, Mutex};

use crate::confirm::{ConfirmResult, ToolConfirmer};
use crate::tool_policy::ToolPolicy;
use aion_config::compact::CompactConfig;
use aion_config::hooks::HookEngine;
use aion_protocol::events::{OutputType, ProtocolEvent, ToolCategory, ToolInfo, ToolStatus};
use aion_protocol::writer::ProtocolEmitter;
use aion_protocol::{ToolApprovalManager, ToolApprovalResult};
use aion_types::message::ContentBlock;
use aion_types::skill_types::ContextModifier;
use aion_types::tool::{ToolExecutionErrorCode, ToolResult, ToolResultTruncation};

use aion_tools::{ToolCallContext, registry::ToolRegistry, truncate_utf8};
use tokio_util::sync::CancellationToken;

/// The combined output of a tool execution batch: protocol content blocks
/// paired with per-call context modifiers (None for non-skill tools).
pub struct ToolCallOutcome {
    pub results: Vec<ContentBlock>,
    pub modifiers: Vec<Option<ContextModifier>>,
    pub follow_up_blocks: Vec<ContentBlock>,
}

#[derive(Debug, Clone, Copy)]
enum ToolExecutionMode {
    Terminal,
    Protocol,
}

impl ToolExecutionMode {
    fn as_str(self) -> &'static str {
        match self {
            Self::Terminal => "terminal",
            Self::Protocol => "protocol",
        }
    }
}

#[derive(Debug, Clone)]
struct ToolExecutionContext {
    execution_id: String,
    session_id: Option<String>,
    turn_id: Option<String>,
    step: usize,
    call_id: String,
    message_id: Option<String>,
    capability_snapshot: ToolCapabilitySnapshot,
    cancellation: CancellationToken,
    policy_source: &'static str,
    approval_source: &'static str,
    mode: ToolExecutionMode,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct ToolExecutionScope {
    pub session_id: Option<String>,
    pub turn_id: Option<String>,
    pub step: usize,
    pub image_input_supported: bool,
    pub cancellation: CancellationToken,
}

#[derive(Debug, Clone)]
struct ToolCapabilitySnapshot {
    image_input_supported: bool,
}

impl ToolExecutionContext {
    fn new(
        call_id: &str,
        message_id: Option<&str>,
        mode: ToolExecutionMode,
        scope: &ToolExecutionScope,
        approval_source: &'static str,
    ) -> Self {
        Self {
            execution_id: format!("tool_exec_{}", uuid::Uuid::now_v7().simple()),
            session_id: scope.session_id.clone(),
            turn_id: scope.turn_id.clone(),
            step: scope.step,
            call_id: call_id.to_owned(),
            message_id: message_id.map(str::to_owned),
            capability_snapshot: ToolCapabilitySnapshot {
                image_input_supported: scope.image_input_supported,
            },
            cancellation: scope.cancellation.clone(),
            policy_source: "runtime_tool_policy",
            approval_source,
            mode,
        }
    }
}

enum ApprovalProvider<'a> {
    Terminal {
        confirmer: &'a Arc<Mutex<ToolConfirmer>>,
    },
    Protocol {
        manager: &'a Arc<ToolApprovalManager>,
        writer: &'a Arc<dyn ProtocolEmitter>,
        msg_id: &'a str,
        auto_approve: bool,
        allow_list: &'a [String],
    },
}

enum ApprovalDecision {
    Approved,
    Denied(String),
}

impl ApprovalProvider<'_> {
    fn source_for(&self, registry: &ToolRegistry, call: &ContentBlock) -> &'static str {
        match self {
            Self::Terminal { .. } => "interactive",
            Self::Protocol {
                manager,
                auto_approve,
                allow_list,
                ..
            } => {
                let ContentBlock::ToolUse { name, .. } = call else {
                    return "automatic";
                };
                let Some(tool) = registry.get(name) else {
                    return "automatic";
                };
                let category = tool.category();
                if !*auto_approve && !allow_list.contains(name) && !manager.is_auto_approved(&category.to_string()) {
                    "host"
                } else {
                    "automatic"
                }
            }
        }
    }

    async fn approve(
        &self,
        registry: &ToolRegistry,
        call: &ContentBlock,
        context: &ToolExecutionContext,
    ) -> Result<ApprovalDecision, ExecutionControl> {
        let ContentBlock::ToolUse { id, name, input, .. } = call else {
            return Ok(ApprovalDecision::Approved);
        };
        match self {
            Self::Terminal { confirmer } => {
                let input_display = serde_json::to_string(input).unwrap_or_default();
                match confirmer
                    .lock()
                    .unwrap()
                    .check(name, &truncate_display(&input_display, 200))
                {
                    ConfirmResult::Approved => Ok(ApprovalDecision::Approved),
                    ConfirmResult::Denied => Ok(ApprovalDecision::Denied("Tool execution denied by user".to_owned())),
                    ConfirmResult::Quit => Err(ExecutionControl::Quit),
                }
            }
            Self::Protocol {
                manager,
                writer,
                msg_id,
                ..
            } => {
                if context.approval_source != "host" {
                    return Ok(ApprovalDecision::Approved);
                }
                let tool = registry.get(name);
                let category = tool.map(|value| value.category()).unwrap_or(ToolCategory::Exec);
                let description = tool.map(|value| value.describe(input)).unwrap_or_default();
                let _ = writer.emit(&ProtocolEvent::ToolRequest {
                    msg_id: (*msg_id).to_owned(),
                    call_id: id.clone(),
                    execution_id: context.execution_id.clone(),
                    tool: ToolInfo {
                        name: name.clone(),
                        category,
                        args: input.clone(),
                        description,
                    },
                });
                match manager.request_approval(id, &category).await {
                    Ok(ToolApprovalResult::Approved) => Ok(ApprovalDecision::Approved),
                    Ok(ToolApprovalResult::Denied { reason }) => {
                        let _ = writer.emit(&ProtocolEvent::ToolCancelled {
                            msg_id: (*msg_id).to_owned(),
                            call_id: id.clone(),
                            execution_id: context.execution_id.clone(),
                            reason: reason.clone(),
                            error_code: Some(ToolExecutionErrorCode::UserDenied),
                        });
                        Ok(ApprovalDecision::Denied(reason))
                    }
                    Err(_) => Err(ExecutionControl::Quit),
                }
            }
        }
    }
}

#[derive(Debug, Clone, Default)]
struct ToolExecutionDetails {
    content_blocks: Option<Vec<serde_json::Value>>,
    structured_content: Option<serde_json::Value>,
    error_code: Option<ToolExecutionErrorCode>,
    truncation: Option<ToolResultTruncation>,
}

fn tool_error(code: &str, message: impl AsRef<str>) -> String {
    format!("[tool_error:{code}] {}", message.as_ref())
}

fn tool_call_id(call: &ContentBlock) -> &str {
    match call {
        ContentBlock::ToolUse { id, .. } => id,
        _ => unreachable!("tool execution received a non-tool-use block"),
    }
}

impl std::ops::Deref for ToolCallOutcome {
    type Target = Vec<ContentBlock>;
    fn deref(&self) -> &Self::Target {
        &self.results
    }
}

impl std::ops::DerefMut for ToolCallOutcome {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.results
    }
}

/// Partition tool calls and execute them with optional confirmation and hooks
pub async fn execute_tool_calls(
    registry: &ToolRegistry,
    tool_calls: &[ContentBlock],
    confirmer: &Arc<Mutex<ToolConfirmer>>,
    hooks: Option<&mut HookEngine>,
    compaction_level: aion_compact::CompactLevel,
    toon_enabled: bool,
) -> Result<ToolCallOutcome, ExecutionControl> {
    execute_tool_calls_with_output_limit(
        registry,
        tool_calls,
        confirmer,
        hooks,
        compaction_level,
        toon_enabled,
        CompactConfig::default().tool_output_max_bytes,
        ToolExecutionScope::default(),
        &ToolPolicy::Unrestricted,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn execute_tool_calls_with_output_limit(
    registry: &ToolRegistry,
    tool_calls: &[ContentBlock],
    confirmer: &Arc<Mutex<ToolConfirmer>>,
    mut hooks: Option<&mut HookEngine>,
    compaction_level: aion_compact::CompactLevel,
    toon_enabled: bool,
    tool_output_max_bytes: usize,
    scope: ToolExecutionScope,
    tool_policy: &ToolPolicy,
) -> Result<ToolCallOutcome, ExecutionControl> {
    let mut results = Vec::new();
    let mut modifiers = Vec::new();
    let mut follow_up_blocks = Vec::new();
    let approval = ApprovalProvider::Terminal { confirmer };

    for batch in partition(registry, tool_calls) {
        if batch.is_concurrent {
            // For concurrent batch, confirm all first, then execute approved ones.
            // Concurrent tools are never SkillTool (is_concurrency_safe=false for Skill),
            // so no skill hooks merging is needed here.
            let mut approved = Vec::new();
            for call in &batch.calls {
                let context = ToolExecutionContext::new(
                    tool_call_id(call),
                    None,
                    ToolExecutionMode::Terminal,
                    &scope,
                    approval.source_for(registry, call),
                );
                if let Some(denied) = policy_denial_result(registry, call, &context, &scope, tool_policy) {
                    results.push(denied);
                    modifiers.push(None);
                    continue;
                }
                match approval.approve(registry, call, &context).await? {
                    ApprovalDecision::Denied(reason) => {
                        results.push(approval_denial_result(call, &reason));
                        modifiers.push(None);
                    }
                    ApprovalDecision::Approved => approved.push((*call, context)),
                }
            }
            // Reborrow as shared for concurrent execution.
            let hooks_shared: Option<&HookEngine> = hooks.as_deref();
            let futures: Vec<_> = approved
                .iter()
                .map(|(call, context)| {
                    execute_single(
                        registry,
                        call,
                        context.clone(),
                        hooks_shared,
                        compaction_level,
                        toon_enabled,
                        tool_output_max_bytes,
                    )
                })
                .collect();
            let batch_results = futures::future::join_all(futures).await;
            for (block, modifier, blocks, _) in batch_results {
                results.push(block);
                modifiers.push(modifier);
                follow_up_blocks.extend(blocks);
            }
        } else {
            for call in &batch.calls {
                let context = ToolExecutionContext::new(
                    tool_call_id(call),
                    None,
                    ToolExecutionMode::Terminal,
                    &scope,
                    approval.source_for(registry, call),
                );
                if let Some(denied) = policy_denial_result(registry, call, &context, &scope, tool_policy) {
                    results.push(denied);
                    modifiers.push(None);
                    continue;
                }
                match approval.approve(registry, call, &context).await? {
                    ApprovalDecision::Denied(reason) => {
                        results.push(approval_denial_result(call, &reason));
                        modifiers.push(None);
                    }
                    ApprovalDecision::Approved => {
                        // Reborrow as shared for execute_single, then reclaim mut for merge.
                        let block;
                        let modifier;
                        let blocks;
                        {
                            let hooks_shared: Option<&HookEngine> = hooks.as_deref();
                            (block, modifier, blocks, _) = execute_single(
                                registry,
                                call,
                                context,
                                hooks_shared,
                                compaction_level,
                                toon_enabled,
                                tool_output_max_bytes,
                            )
                            .await;
                        }
                        // Merge skill hooks after a successful sequential execution.
                        if !block_is_error(&block) {
                            maybe_merge_skill_hooks(registry, call, hooks.as_deref_mut());
                        }
                        results.push(block);
                        modifiers.push(modifier);
                        follow_up_blocks.extend(blocks);
                    }
                }
            }
        }
    }

    truncate_tool_result_blocks(&mut results, tool_output_max_bytes);

    Ok(ToolCallOutcome {
        results,
        modifiers,
        follow_up_blocks,
    })
}

/// Signal that the user wants to abort
#[derive(Debug)]
pub enum ExecutionControl {
    Quit,
}

fn approval_denial_result(call: &ContentBlock, reason: &str) -> ContentBlock {
    let ContentBlock::ToolUse { id, .. } = call else {
        unreachable!("approval received a non-tool-use block")
    };
    ContentBlock::ToolResult {
        tool_use_id: id.clone(),
        content: tool_error("user_denied", format!("Tool denied: {reason}")),
        is_error: true,
    }
}

async fn execute_single(
    registry: &ToolRegistry,
    call: &ContentBlock,
    context: ToolExecutionContext,
    hooks: Option<&HookEngine>,
    compaction_level: aion_compact::CompactLevel,
    toon_enabled: bool,
    tool_output_max_bytes: usize,
) -> (
    ContentBlock,
    Option<ContextModifier>,
    Vec<ContentBlock>,
    ToolExecutionDetails,
) {
    let ContentBlock::ToolUse { id, name, input, .. } = call else {
        unreachable!("execute_single called with non-ToolUse block")
    };

    let start = std::time::Instant::now();
    tracing::info!(
        target: "aion_agent",
        tool = %name,
        call_id = %context.call_id,
        execution_id = %context.execution_id,
        execution_mode = context.mode.as_str(),
        message_id = context.message_id.as_deref().unwrap_or(""),
        session_id = context.session_id.as_deref().unwrap_or(""),
        turn_id = context.turn_id.as_deref().unwrap_or(""),
        step = context.step,
        image_input_supported = context.capability_snapshot.image_input_supported,
        policy_source = context.policy_source,
        approval_source = context.approval_source,
        cancelled = context.cancellation.is_cancelled(),
        "tool execution started"
    );

    // Run pre-tool-use hooks
    if let Some(hook_engine) = hooks
        && let Err(e) = hook_engine.run_pre_tool_use(name, input).await
    {
        return (
            ContentBlock::ToolResult {
                tool_use_id: id.clone(),
                content: tool_error("hook_failed", format!("Blocked by hook: {e}")),
                is_error: true,
            },
            None,
            Vec::new(),
            ToolExecutionDetails {
                error_code: Some(ToolExecutionErrorCode::HookFailed),
                ..ToolExecutionDetails::default()
            },
        );
    }

    let (result, modifier, follow_up_blocks, details) = match registry.get(name) {
        Some(tool) => {
            let max_size = tool.max_result_size();
            let execution = tool
                .execute_with_context(
                    input.clone(),
                    &ToolCallContext {
                        execution_id: context.execution_id.clone(),
                        cancellation: context.cancellation.clone(),
                    },
                )
                .await;
            let mut details = ToolExecutionDetails {
                content_blocks: execution.content_blocks,
                structured_content: execution.structured_content,
                error_code: execution.error_code,
                truncation: execution.truncation,
            };
            let r = execution.result;
            let modifier = if r.is_error {
                None
            } else {
                tool.context_modifier_for(input)
            };
            let follow_up_blocks = if r.is_error {
                Vec::new()
            } else {
                execution.follow_up_blocks
            };
            let error_content = if r.is_error && tool.is_deferred() {
                maybe_append_deferred_hint(&r.content, tool.input_schema(), input)
            } else {
                r.content.clone()
            };
            let content = aion_compact::compact_output(&error_content, compaction_level);
            let content = if toon_enabled {
                aion_compact::compact_output_toon(&content)
            } else {
                content
            };
            let original_bytes = content.len();
            let output_limit = max_size.min(tool_output_max_bytes);
            let content = truncate_result(&content, output_limit);
            if content.len() < original_bytes {
                details.truncation = Some(ToolResultTruncation {
                    original_bytes,
                    output_bytes: content.len(),
                    limit_bytes: output_limit,
                });
                tracing::debug!(
                    target: "aion_agent",
                    tool = %name,
                    original_bytes,
                    output_bytes = content.len(),
                    output_limit,
                    "tool result truncated for model context"
                );
            }
            if r.is_error && details.error_code.is_none() {
                details.error_code = Some(ToolExecutionErrorCode::ExecutionFailed);
            }
            (
                ToolResult {
                    content,
                    is_error: r.is_error,
                },
                modifier,
                follow_up_blocks,
                details,
            )
        }
        None => (
            ToolResult {
                content: tool_error("unknown_tool", format!("Unknown tool: {name}")),
                is_error: true,
            },
            None,
            Vec::new(),
            ToolExecutionDetails {
                error_code: Some(ToolExecutionErrorCode::UnknownTool),
                ..ToolExecutionDetails::default()
            },
        ),
    };

    // Run post-tool-use hooks
    if let Some(hook_engine) = hooks {
        let messages = hook_engine.run_post_tool_use(name, input, &result.content).await;
        for msg in messages {
            tracing::info!(target: "aion_agent", hook_message = %msg, "post-tool-use hook output");
        }
    }

    let duration_ms = start.elapsed().as_millis() as u64;
    tracing::info!(
        target: "aion_agent",
        execution_id = %context.execution_id,
        call_id = %context.call_id,
        duration_ms,
        success = !result.is_error,
        "tool execution completed"
    );

    (
        ContentBlock::ToolResult {
            tool_use_id: id.clone(),
            content: result.content,
            is_error: result.is_error,
        },
        modifier,
        follow_up_blocks,
        details,
    )
}

fn policy_denial_result(
    registry: &ToolRegistry,
    call: &ContentBlock,
    context: &ToolExecutionContext,
    scope: &ToolExecutionScope,
    tool_policy: &ToolPolicy,
) -> Option<ContentBlock> {
    let ContentBlock::ToolUse { id, name, .. } = call else {
        return None;
    };
    let tool = registry.get(name)?;
    let policy_denied = !tool_policy.allows(name);
    let capability_denied = tool.requires_image_input() && !scope.image_input_supported;
    if !policy_denied && !capability_denied {
        return None;
    }

    tracing::warn!(
        target: "aion_agent",
        event = "agent.tool_policy.denied",
        tool_call_id = %id,
        execution_id = %context.execution_id,
        tool = %name,
        policy_denied,
        capability_denied,
        "rejected tool call before approval"
    );
    Some(ContentBlock::ToolResult {
        tool_use_id: id.clone(),
        content: tool_error(
            "policy_denied",
            format!("Tool '{name}' is not available in this runtime. Use an available tool or answer in text."),
        ),
        is_error: true,
    })
}

/// Execute tool calls with JSON stream protocol approval flow
#[allow(clippy::too_many_arguments)]
pub async fn execute_tool_calls_with_approval(
    registry: &ToolRegistry,
    tool_calls: &[ContentBlock],
    approval_manager: &Arc<ToolApprovalManager>,
    writer: &Arc<dyn ProtocolEmitter>,
    msg_id: &str,
    auto_approve: bool,
    allow_list: &[String],
    hooks: Option<&mut HookEngine>,
    compaction_level: aion_compact::CompactLevel,
    toon_enabled: bool,
) -> Result<ToolCallOutcome, ExecutionControl> {
    execute_tool_calls_with_approval_and_output_limit(
        registry,
        tool_calls,
        approval_manager,
        writer,
        msg_id,
        auto_approve,
        allow_list,
        hooks,
        compaction_level,
        toon_enabled,
        CompactConfig::default().tool_output_max_bytes,
        ToolExecutionScope::default(),
        &ToolPolicy::Unrestricted,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn execute_tool_calls_with_approval_and_output_limit(
    registry: &ToolRegistry,
    tool_calls: &[ContentBlock],
    approval_manager: &Arc<ToolApprovalManager>,
    writer: &Arc<dyn ProtocolEmitter>,
    msg_id: &str,
    auto_approve: bool,
    allow_list: &[String],
    mut hooks: Option<&mut HookEngine>,
    compaction_level: aion_compact::CompactLevel,
    toon_enabled: bool,
    tool_output_max_bytes: usize,
    scope: ToolExecutionScope,
    tool_policy: &ToolPolicy,
) -> Result<ToolCallOutcome, ExecutionControl> {
    let mut results = Vec::new();
    let mut modifiers = Vec::new();
    let mut follow_up_blocks = Vec::new();
    let approval = ApprovalProvider::Protocol {
        manager: approval_manager,
        writer,
        msg_id,
        auto_approve,
        allow_list,
    };

    for call in tool_calls {
        let ContentBlock::ToolUse { id, name, .. } = call else {
            continue;
        };

        let context = ToolExecutionContext::new(
            id,
            Some(msg_id),
            ToolExecutionMode::Protocol,
            &scope,
            approval.source_for(registry, call),
        );

        if let Some(denied) = policy_denial_result(registry, call, &context, &scope, tool_policy) {
            if let ContentBlock::ToolResult { content, .. } = &denied {
                let _ = writer.emit(&ProtocolEvent::ToolResult {
                    msg_id: msg_id.to_string(),
                    call_id: id.clone(),
                    execution_id: context.execution_id.clone(),
                    tool_name: name.clone(),
                    status: ToolStatus::Error,
                    output: content.clone(),
                    output_type: OutputType::Text,
                    metadata: None,
                    content_blocks: None,
                    structured_content: None,
                    error_code: Some(ToolExecutionErrorCode::PolicyDenied),
                    truncation: None,
                });
            }
            results.push(denied);
            modifiers.push(None);
            continue;
        }

        if let ApprovalDecision::Denied(reason) = approval.approve(registry, call, &context).await? {
            results.push(approval_denial_result(call, &reason));
            modifiers.push(None);
            continue;
        }

        // Emit tool_running
        let _ = writer.emit(&ProtocolEvent::ToolRunning {
            msg_id: msg_id.to_string(),
            call_id: id.clone(),
            execution_id: context.execution_id.clone(),
            tool_name: name.clone(),
        });

        // Execute the tool (reborrow as shared for execute_single, then reclaim mut for merge).
        let result;
        let modifier;
        let blocks;
        let details;
        {
            let hooks_shared: Option<&HookEngine> = hooks.as_deref();
            (result, modifier, blocks, details) = execute_single(
                registry,
                call,
                context.clone(),
                hooks_shared,
                compaction_level,
                toon_enabled,
                tool_output_max_bytes,
            )
            .await;
        }

        // Emit tool_result event
        if let ContentBlock::ToolResult { content, is_error, .. } = &result {
            if details.error_code == Some(ToolExecutionErrorCode::Canceled) {
                let _ = writer.emit(&ProtocolEvent::ToolCancelled {
                    msg_id: msg_id.to_string(),
                    call_id: id.clone(),
                    execution_id: context.execution_id.clone(),
                    reason: "Tool execution canceled".to_owned(),
                    error_code: Some(ToolExecutionErrorCode::Canceled),
                });
            } else {
                let status = if *is_error {
                    ToolStatus::Error
                } else {
                    ToolStatus::Success
                };
                let output_type = if details.content_blocks.as_ref().is_some_and(|blocks| {
                    blocks
                        .iter()
                        .any(|block| block.get("type").and_then(serde_json::Value::as_str) == Some("image"))
                }) {
                    OutputType::Image
                } else {
                    OutputType::Text
                };
                let _ = writer.emit(&ProtocolEvent::ToolResult {
                    msg_id: msg_id.to_string(),
                    call_id: id.clone(),
                    execution_id: context.execution_id.clone(),
                    tool_name: name.clone(),
                    status,
                    output: content.clone(),
                    output_type,
                    metadata: None,
                    content_blocks: details.content_blocks,
                    structured_content: details.structured_content,
                    error_code: details.error_code,
                    truncation: details.truncation,
                });
            }
        }

        // Merge skill hooks after a successful execution.
        if !block_is_error(&result) {
            maybe_merge_skill_hooks(registry, call, hooks.as_deref_mut());
        }

        results.push(result);
        modifiers.push(modifier);
        follow_up_blocks.extend(blocks);
    }

    truncate_tool_result_blocks(&mut results, tool_output_max_bytes);

    Ok(ToolCallOutcome {
        results,
        modifiers,
        follow_up_blocks,
    })
}

/// If `call` is a Skill tool call that returned successfully, merge skill hooks into the engine.
fn merge_skill_hooks_into(engine: &mut HookEngine, registry: &ToolRegistry, call: &ContentBlock) {
    let ContentBlock::ToolUse { name, input, .. } = call else {
        return;
    };
    if name != "Skill" {
        return;
    }
    let Some(tool) = registry.get(name) else {
        return;
    };
    if let Some(skill_hooks) = tool.skill_hooks_for(input) {
        engine.merge_hooks(skill_hooks);
    }
}

fn maybe_merge_skill_hooks(registry: &ToolRegistry, call: &ContentBlock, hooks: Option<&mut HookEngine>) {
    if let Some(engine) = hooks {
        merge_skill_hooks_into(engine, registry, call);
    }
}

/// Returns true when a ContentBlock::ToolResult has is_error=true.
fn block_is_error(block: &ContentBlock) -> bool {
    matches!(block, ContentBlock::ToolResult { is_error: true, .. })
}

/// When a deferred tool fails AND the input is missing required fields from
/// its full schema, append a hint telling the LLM to call ToolSearch first.
/// If required fields are all present (or the schema has none), the original
/// error is returned unchanged — the failure is a runtime issue, not a
/// missing-schema problem.
fn maybe_append_deferred_hint(original_error: &str, schema: serde_json::Value, input: &serde_json::Value) -> String {
    let missing: Vec<&str> = schema["required"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str())
                .filter(|key| input.get(key).is_none())
                .collect()
        })
        .unwrap_or_default();

    if missing.is_empty() {
        return original_error.to_string();
    }

    format!(
        "{}\n\nThis is a deferred tool — its full parameter schema was not loaded. \
         Call ToolSearch to load the schema, then retry.",
        original_error
    )
}

fn truncate_tool_result_blocks(results: &mut [ContentBlock], max_bytes: usize) {
    for result in results {
        if let ContentBlock::ToolResult { content, .. } = result
            && content.len() > max_bytes
        {
            *content = truncate_result(content, max_bytes);
        }
    }
}

fn truncate_result(content: &str, max_bytes: usize) -> String {
    if content.len() <= max_bytes {
        return content.to_string();
    }

    let marker = format!("\n\n... [truncated output: original {} bytes] ...\n\n", content.len());
    if marker.len() >= max_bytes {
        return truncate_utf8(&marker, max_bytes).to_string();
    }

    let content_budget = max_bytes - marker.len();
    let head_budget = content_budget.div_ceil(2);
    let tail_budget = content_budget / 2;

    let mut head_end = head_budget;
    while head_end > 0 && !content.is_char_boundary(head_end) {
        head_end -= 1;
    }

    let mut tail_start = content.len() - tail_budget;
    while tail_start < content.len() && !content.is_char_boundary(tail_start) {
        tail_start += 1;
    }

    let head = &content[..head_end];
    let tail = &content[tail_start..];
    format!("{head}{marker}{tail}")
}

fn truncate_display(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        // Find a char boundary to avoid panicking on multi-byte characters
        let end = s.char_indices().nth(max).map(|(i, _)| i).unwrap_or(s.len());
        format!("{}...", &s[..end])
    }
}

struct Batch<'a> {
    is_concurrent: bool,
    calls: Vec<&'a ContentBlock>,
}

fn partition<'a>(registry: &ToolRegistry, calls: &'a [ContentBlock]) -> Vec<Batch<'a>> {
    let mut batches: Vec<Batch<'a>> = Vec::new();

    for call in calls {
        let ContentBlock::ToolUse { name, input, .. } = call else {
            continue;
        };
        let is_safe = registry
            .get(name)
            .map(|t| t.is_concurrency_safe(input))
            .unwrap_or(false);

        match batches.last_mut() {
            Some(last) if last.is_concurrent && is_safe => {
                last.calls.push(call);
            }
            _ => {
                batches.push(Batch {
                    is_concurrent: is_safe,
                    calls: vec![call],
                });
            }
        }
    }

    batches
}

#[cfg(test)]
#[path = "orchestration_test.rs"]
mod orchestration_test;
