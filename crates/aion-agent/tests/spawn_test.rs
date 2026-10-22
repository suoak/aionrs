mod common;

use std::sync::{Arc, Mutex};

use aion_agent::spawner::{AgentSpawner, SubAgentConfig};
use aion_agent::tool_policy::ToolPolicy;
use aion_providers::{LlmProvider, ProviderError};
use aion_types::llm::{LlmEvent, LlmRequest};
use aion_types::message::{StopReason, TokenUsage};
use async_trait::async_trait;
use common::{MockLlmProvider, test_config};
use tokio::sync::mpsc;

struct ToolRecordingProvider {
    tool_names: Arc<Mutex<Vec<String>>>,
}

#[async_trait]
impl LlmProvider for ToolRecordingProvider {
    async fn stream(&self, request: &LlmRequest) -> Result<mpsc::Receiver<LlmEvent>, ProviderError> {
        let mut tool_names: Vec<_> = request.tools.iter().map(|tool| tool.name.clone()).collect();
        tool_names.sort();
        *self.tool_names.lock().unwrap() = tool_names;

        let (tx, rx) = mpsc::channel(2);
        tx.try_send(LlmEvent::TextDelta("done".to_string())).unwrap();
        tx.try_send(LlmEvent::Done {
            stop_reason: StopReason::EndTurn,
            usage: TokenUsage::default(),
        })
        .unwrap();
        Ok(rx)
    }
}

// ---------------------------------------------------------------------------
// Helper: build a minimal SubAgentConfig for testing
// ---------------------------------------------------------------------------

fn make_sub_config(name: &str) -> SubAgentConfig {
    SubAgentConfig {
        name: name.to_string(),
        prompt: format!("Task for {}", name),
        max_turns: 5,
        max_tokens: 1024,
        system_prompt: None,
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// Single sub-agent executes and returns the expected text result.
#[tokio::test]
async fn test_spawn_single_agent() {
    let provider = Arc::new(MockLlmProvider::with_text_response("Sub-agent done"));
    let spawner = AgentSpawner::new(provider, test_config(), std::env::temp_dir(), ToolPolicy::Unrestricted);

    let result = spawner.spawn_one(make_sub_config("agent-1")).await;

    assert_eq!(result.text, "Sub-agent done");
    assert!(!result.is_error, "expected no error, got: {}", result.text);
    assert_eq!(result.turns, 1);
    assert_eq!(result.name, "agent-1");
}

#[tokio::test]
async fn restricted_parent_policy_limits_spawned_agent_tools() {
    let tool_names = Arc::new(Mutex::new(Vec::new()));
    let provider = Arc::new(ToolRecordingProvider {
        tool_names: Arc::clone(&tool_names),
    });
    let spawner = AgentSpawner::new(
        provider,
        test_config(),
        std::env::temp_dir(),
        ToolPolicy::allow_only(["Spawn", "Read", "Grep"]),
    );

    let result = spawner.spawn_one(make_sub_config("restricted-agent")).await;

    assert!(!result.is_error, "expected restricted sub-agent to complete");
    assert_eq!(*tool_names.lock().unwrap(), vec!["Grep", "Read"]);
}

/// Parallel sub-agents all complete successfully and return distinct results.
#[tokio::test]
async fn test_spawn_parallel_agents() {
    // Provide one turn sequence per sub-agent; each stream() call pops one entry.
    let make_turn = |text: &str| {
        vec![
            LlmEvent::TextDelta(text.to_string()),
            LlmEvent::Done {
                stop_reason: StopReason::EndTurn,
                usage: TokenUsage {
                    input_tokens: 10,
                    output_tokens: 5,
                    cache_creation_tokens: 0,
                    cache_read_tokens: 0,
                },
            },
        ]
    };

    let provider = Arc::new(MockLlmProvider::with_turns(vec![
        make_turn("result-A"),
        make_turn("result-B"),
        make_turn("result-C"),
    ]));

    let spawner = AgentSpawner::new(provider, test_config(), std::env::temp_dir(), ToolPolicy::Unrestricted);

    let sub_configs = vec![
        make_sub_config("agent-A"),
        make_sub_config("agent-B"),
        make_sub_config("agent-C"),
    ];

    let results = spawner.spawn_parallel(sub_configs).await;

    assert_eq!(results.len(), 3, "expected 3 results from 3 sub-agents");

    for result in &results {
        assert!(
            !result.is_error,
            "sub-agent '{}' returned an error: {}",
            result.name, result.text
        );
    }

    // Each result should contain one of the expected texts (order may vary due
    // to concurrent scheduling, so we just verify the full set is covered).
    let texts: std::collections::HashSet<&str> = results.iter().map(|r| r.text.as_str()).collect();
    assert!(texts.contains("result-A"), "missing result-A");
    assert!(texts.contains("result-B"), "missing result-B");
    assert!(texts.contains("result-C"), "missing result-C");
}

/// The same provider Arc is reused across sequentially spawned sub-agents.
#[tokio::test]
async fn test_spawn_shares_provider() {
    // Two turns: one for each sequential sub-agent call.
    let provider = Arc::new(MockLlmProvider::with_turns(vec![
        vec![
            LlmEvent::TextDelta("first".to_string()),
            LlmEvent::Done {
                stop_reason: StopReason::EndTurn,
                usage: TokenUsage {
                    input_tokens: 10,
                    output_tokens: 5,
                    cache_creation_tokens: 0,
                    cache_read_tokens: 0,
                },
            },
        ],
        vec![
            LlmEvent::TextDelta("second".to_string()),
            LlmEvent::Done {
                stop_reason: StopReason::EndTurn,
                usage: TokenUsage {
                    input_tokens: 10,
                    output_tokens: 5,
                    cache_creation_tokens: 0,
                    cache_read_tokens: 0,
                },
            },
        ],
    ]));

    // Both sub-agents share the same underlying provider via Arc.
    let provider_dyn: Arc<dyn aion_providers::LlmProvider> = provider;
    let spawner = AgentSpawner::new(
        Arc::clone(&provider_dyn),
        test_config(),
        std::env::temp_dir(),
        ToolPolicy::Unrestricted,
    );

    let result1 = spawner.spawn_one(make_sub_config("seq-1")).await;
    let result2 = spawner.spawn_one(make_sub_config("seq-2")).await;

    assert!(!result1.is_error, "seq-1 errored: {}", result1.text);
    assert!(!result2.is_error, "seq-2 errored: {}", result2.text);
    assert_eq!(result1.text, "first");
    assert_eq!(result2.text, "second");
}

/// An LLM error event causes the sub-agent result to be marked as an error.
#[tokio::test]
async fn test_spawn_agent_error_captured() {
    // Emit an Error event — the engine converts this to AgentError::ApiError,
    // which spawner catches and stores in SubAgentResult::is_error.
    let provider = Arc::new(MockLlmProvider::with_events(vec![LlmEvent::Error(
        "provider failed".to_string(),
    )]));

    let spawner = AgentSpawner::new(provider, test_config(), std::env::temp_dir(), ToolPolicy::Unrestricted);

    let result = spawner.spawn_one(make_sub_config("error-agent")).await;

    assert!(result.is_error, "expected is_error=true");
    assert!(
        result.text.to_lowercase().contains("error"),
        "expected error message to contain 'error', got: {}",
        result.text
    );
}

// ---------------------------------------------------------------------------
// Sub-agent approval inheritance (M7 / E2)
// ---------------------------------------------------------------------------

mod approval_inheritance {
    use std::io;
    use std::sync::{Arc, Mutex};

    use aion_agent::engine::AgentEngine;
    use aion_agent::output::null_sink::NullSink;
    use aion_agent::spawner::{AgentSpawner, SubAgentConfig};
    use aion_agent::tool_policy::ToolPolicy;
    use aion_protocol::events::ProtocolEvent;
    use aion_protocol::writer::ProtocolEmitter;
    use aion_protocol::{ToolApprovalManager, ToolApprovalResult};
    use aion_tools::registry::ToolRegistry;
    use aion_types::llm::LlmEvent;
    use aion_types::message::{StopReason, TokenUsage};
    use serde_json::json;

    use super::common::{MockLlmProvider, test_config};

    #[derive(Default)]
    struct CapturingEmitter {
        requests: Mutex<Vec<(String, String, String)>>,
    }

    impl ProtocolEmitter for CapturingEmitter {
        fn emit(&self, event: &ProtocolEvent) -> io::Result<()> {
            if let ProtocolEvent::ToolRequest {
                msg_id, call_id, tool, ..
            } = event
            {
                self.requests
                    .lock()
                    .unwrap()
                    .push((msg_id.clone(), call_id.clone(), tool.name.clone()));
            }
            Ok(())
        }
    }

    impl CapturingEmitter {
        fn tool_requests(&self) -> Vec<(String, String, String)> {
            self.requests.lock().unwrap().clone()
        }
    }

    fn exec_then_finish(call_id: &str) -> MockLlmProvider {
        MockLlmProvider::with_turns(vec![
            vec![
                LlmEvent::ToolUse {
                    id: call_id.to_string(),
                    name: "ExecCommand".to_string(),
                    input: json!({ "cmd": "echo sub-agent" }),
                    extra: None,
                },
                LlmEvent::Done {
                    stop_reason: StopReason::ToolUse,
                    usage: TokenUsage::default(),
                },
            ],
            vec![
                LlmEvent::TextDelta("child finished".to_string()),
                LlmEvent::Done {
                    stop_reason: StopReason::EndTurn,
                    usage: TokenUsage::default(),
                },
            ],
        ])
    }

    fn sub_config() -> SubAgentConfig {
        SubAgentConfig {
            name: "child".to_string(),
            prompt: "run a command".to_string(),
            max_turns: 5,
            max_tokens: 1024,
            system_prompt: None,
        }
    }

    /// Parent engine in the default (ask) mode wired to a host approval channel.
    fn parent_engine(auto_approve: bool) -> (AgentEngine, Arc<ToolApprovalManager>, Arc<CapturingEmitter>) {
        let mut config = test_config();
        config.tools.auto_approve = auto_approve;
        let mut engine = AgentEngine::new_with_provider(
            Arc::new(MockLlmProvider::with_text_response("parent")),
            config,
            ToolRegistry::new(),
            Arc::new(NullSink),
            std::env::temp_dir(),
        );
        let manager = Arc::new(ToolApprovalManager::new());
        let emitter = Arc::new(CapturingEmitter::default());
        engine.set_approval_manager(Arc::clone(&manager));
        engine.set_protocol_writer(emitter.clone());
        (engine, manager, emitter)
    }

    /// Resolve `call_id` once it is pending. `ToolRequest` is emitted just
    /// before the approval is registered, so keep retrying until stopped.
    fn spawn_resolver(
        manager: Arc<ToolApprovalManager>,
        call_id: &'static str,
        approve: bool,
        stop: Arc<std::sync::atomic::AtomicBool>,
    ) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            for _ in 0..2000 {
                if stop.load(std::sync::atomic::Ordering::SeqCst) {
                    return;
                }
                let result = if approve {
                    ToolApprovalResult::Approved
                } else {
                    ToolApprovalResult::Denied {
                        reason: "denied in test".to_string(),
                    }
                };
                manager.resolve(call_id, result);
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            }
        })
    }

    fn spawner_for(engine: &AgentEngine, provider: MockLlmProvider) -> AgentSpawner {
        // Even if the spawner's base config says auto-approve, the parent's
        // live approval state wins once the link is attached.
        AgentSpawner::new(
            Arc::new(provider),
            test_config(),
            std::env::temp_dir(),
            ToolPolicy::Unrestricted,
        )
        .with_approval_link(engine.approval_link())
    }

    #[tokio::test]
    async fn default_mode_child_exec_command_emits_tool_request() {
        let (engine, manager, emitter) = parent_engine(false);
        let spawner = spawner_for(&engine, exec_then_finish("child-call-1"));

        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let resolver = spawn_resolver(Arc::clone(&manager), "child-call-1", false, Arc::clone(&stop));
        let result = spawner.spawn_one(sub_config()).await;
        stop.store(true, std::sync::atomic::Ordering::SeqCst);
        resolver.await.unwrap();

        assert!(!result.is_error, "child should finish after denial: {}", result.text);
        let requests = emitter.tool_requests();
        assert_eq!(requests.len(), 1, "expected exactly one tool_request, got {requests:?}");
        assert_eq!(requests[0].1, "child-call-1");
        assert_eq!(requests[0].2, "ExecCommand");
    }

    #[tokio::test]
    async fn yolo_parent_auto_approves_child_without_tool_request() {
        let (engine, _manager, emitter) = parent_engine(false);
        // Host flips the session to YOLO mid-session through the shared flag.
        engine
            .auto_approve_handle()
            .store(true, std::sync::atomic::Ordering::SeqCst);
        let spawner = spawner_for(&engine, exec_then_finish("child-call-2"));

        let result = spawner.spawn_one(sub_config()).await;

        assert!(!result.is_error, "{}", result.text);
        assert_eq!(result.text, "child finished");
        assert!(emitter.tool_requests().is_empty(), "YOLO parent must not prompt");
    }

    #[tokio::test]
    async fn leaving_yolo_restores_child_approval() {
        let (engine, manager, emitter) = parent_engine(true);
        engine
            .auto_approve_handle()
            .store(false, std::sync::atomic::Ordering::SeqCst);
        let spawner = spawner_for(&engine, exec_then_finish("child-call-3"));

        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let resolver = spawn_resolver(Arc::clone(&manager), "child-call-3", true, Arc::clone(&stop));
        let result = spawner.spawn_one(sub_config()).await;
        stop.store(true, std::sync::atomic::Ordering::SeqCst);
        resolver.await.unwrap();

        assert!(!result.is_error, "{}", result.text);
        assert_eq!(emitter.tool_requests().len(), 1);
    }
}
