use super::*;

#[cfg(test)]
mod phase7_tests {
    use super::{ForkOverrides, SubAgentConfig, ToolPolicy, build_tool_registry, effective_child_tool_policy};

    #[test]
    fn tc_7_1_fork_overrides_default_values() {
        let o = ForkOverrides::default();
        assert!(o.model.is_none());
        assert!(o.effort.is_none());
        assert!(o.allowed_tools.is_empty());
    }

    #[test]
    fn tc_7_40_build_tool_registry_unrestricted_registers_all() {
        let registry = build_tool_registry(&ToolPolicy::Unrestricted, &std::env::temp_dir(), &[]);
        for name in &["Read", "Write", "Edit", "ExecCommand", "Grep", "Glob"] {
            assert!(registry.get(name).is_some(), "tool '{name}' should be registered");
        }
    }

    #[test]
    fn tc_7_43_build_tool_registry_filters_to_policy() {
        let policy = ToolPolicy::allow_only(["ExecCommand", "Read"]);
        let registry = build_tool_registry(&policy, &std::env::temp_dir(), &[]);
        assert!(registry.get("ExecCommand").is_some());
        assert!(registry.get("Read").is_some());
        assert!(registry.get("Write").is_none());
    }

    #[test]
    fn fork_overrides_can_only_narrow_parent_policy() {
        let parent = ToolPolicy::allow_only(["Read", "Grep", "Spawn"]);
        let allowed_tools = vec!["Read".to_string(), "ExecCommand".to_string()];

        let child = effective_child_tool_policy(&parent, &allowed_tools);

        assert!(child.allows("Read"));
        assert!(!child.allows("Grep"));
        assert!(!child.allows("ExecCommand"));
        assert!(!child.allows("Write"));
    }

    #[test]
    fn empty_fork_override_inherits_parent_policy() {
        let parent = ToolPolicy::allow_only(["Read", "Grep", "Spawn"]);

        let child = effective_child_tool_policy(&parent, &[]);

        assert_eq!(child, parent);
    }

    #[test]
    fn tc_7_sub_agent_config_original_fields_intact() {
        let config = SubAgentConfig {
            name: "test-agent".to_string(),
            prompt: "do the task".to_string(),
            max_turns: 5,
            max_tokens: 1024,
            system_prompt: Some("you are helpful".to_string()),
        };
        assert_eq!(config.name, "test-agent");
        assert_eq!(config.max_turns, 5);
    }
}

mod approval_inheritance {
    use std::sync::Arc;

    use aion_config::config::Config;
    use aion_providers::{LlmProvider, ProviderError};
    use aion_types::llm::{LlmEvent, LlmRequest};

    use super::{AgentSpawner, ToolPolicy};
    use crate::approval_link::ApprovalLink;
    use crate::confirm::ToolConfirmer;

    struct NullProvider;

    #[async_trait::async_trait]
    impl LlmProvider for NullProvider {
        async fn stream(&self, _: &LlmRequest) -> Result<tokio::sync::mpsc::Receiver<LlmEvent>, ProviderError> {
            let (_tx, rx) = tokio::sync::mpsc::channel(1);
            Ok(rx)
        }
    }

    fn config(auto_approve: bool) -> Config {
        Config::resolve(&aion_config::config::CliArgs {
            provider: Some("anthropic".to_string()),
            api_key: Some("sk-test".to_string()),
            base_url: None,
            model: Some("test-model".to_string()),
            max_tokens: Some(1024),
            thinking: None,
            thinking_budget: None,
            max_turns: None,
            max_tool_call_malformed_turns: None,
            max_tool_call_failure_turns: None,
            system_prompt: None,
            profile: None,
            auto_approve,
            project_dir: None,
        })
        .unwrap()
    }

    #[test]
    fn child_without_link_is_not_force_approved() {
        let spawner = AgentSpawner::new(
            Arc::new(NullProvider),
            config(false),
            std::env::temp_dir(),
            ToolPolicy::Unrestricted,
        );
        let (engine, msg_id) = spawner.build_child_engine(config(false), ToolPolicy::Unrestricted);
        assert!(
            !engine.is_auto_approve(),
            "sub-agent must not hard-code auto_approve=true"
        );
        assert!(msg_id.is_empty());
    }

    #[test]
    fn child_with_link_tracks_parent_runtime_flag() {
        let link = ApprovalLink::new();
        let parent_confirmer = Arc::new(std::sync::Mutex::new(ToolConfirmer::new(false, vec![])));
        link.set_confirmer(Arc::clone(&parent_confirmer));
        link.set_msg_id("parent-msg");

        // Base config says auto-approve; the parent's live flag must win.
        let spawner = AgentSpawner::new(
            Arc::new(NullProvider),
            config(true),
            std::env::temp_dir(),
            ToolPolicy::Unrestricted,
        )
        .with_approval_link(link);
        let (engine, msg_id) = spawner.build_child_engine(config(true), ToolPolicy::Unrestricted);
        assert_eq!(msg_id, "parent-msg");
        assert!(!engine.is_auto_approve());

        parent_confirmer.lock().unwrap().set_auto_approve(true);
        assert!(engine.is_auto_approve(), "YOLO on the parent applies to the child");
    }
}
