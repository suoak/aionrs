use super::*;

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::fs;
    use std::sync::Arc;

    use aion_config::config::{CliArgs, McpServerConfig, TransportType};
    use aion_protocol::events::ToolCategory;
    use aion_tools::Tool;
    use aion_types::tool::ToolResult;
    use async_trait::async_trait;
    use serde_json::{Value, json};

    use crate::output::OutputSink;
    use crate::output::null_sink::NullSink;
    use crate::tool_policy::ToolPolicy;

    use super::*;

    struct DeferredTestTool(&'static str);

    #[async_trait]
    impl Tool for DeferredTestTool {
        fn name(&self) -> &str {
            self.0
        }

        fn description(&self) -> &str {
            "deferred test tool"
        }

        fn input_schema(&self) -> Value {
            json!({"type": "object"})
        }

        fn is_concurrency_safe(&self, _input: &Value) -> bool {
            true
        }

        async fn execute(&self, _input: Value) -> ToolResult {
            ToolResult {
                content: "unused".to_string(),
                is_error: false,
            }
        }

        fn category(&self) -> ToolCategory {
            ToolCategory::Info
        }

        fn is_deferred(&self) -> bool {
            true
        }
    }

    fn test_config() -> Config {
        Config::resolve(&CliArgs {
            provider: Some("anthropic".to_string()),
            api_key: Some("sk-test".to_string()),
            base_url: None,
            model: Some("claude-sonnet-4-20250514".to_string()),
            max_tokens: Some(4096),
            thinking: None,
            thinking_budget: None,
            max_turns: None,
            max_tool_call_malformed_turns: None,
            max_tool_call_failure_turns: None,
            system_prompt: None,
            profile: None,
            auto_approve: false,
            project_dir: None,
        })
        .unwrap()
    }

    fn write_skill(project_root: &std::path::Path, name: &str) {
        let skill_dir = project_root.join(".csbu-workmate").join("skills").join(name);
        fs::create_dir_all(&skill_dir).unwrap();
        fs::write(skill_dir.join("SKILL.md"), "---\ndescription: test skill\n---\n").unwrap();
    }

    #[tokio::test]
    async fn isolated_skill_dirs_excludes_workspace_skills() {
        let workspace = tempfile::TempDir::new().unwrap();
        let isolated_root = tempfile::TempDir::new().unwrap();
        write_skill(workspace.path(), "workspace-only");
        write_skill(isolated_root.path(), "isolated-only");

        let output: Arc<dyn OutputSink> = Arc::new(NullSink);
        let bootstrap = AgentBootstrap::new(test_config(), workspace.path().to_string_lossy(), output)
            .isolated_skill_dirs(vec![isolated_root.path().to_path_buf()]);

        let names = bootstrap
            .load_skills(workspace.path(), None)
            .await
            .into_iter()
            .map(|skill| skill.name)
            .collect::<Vec<_>>();

        assert!(names.iter().any(|name| name == "isolated-only"));
        assert!(!names.iter().any(|name| name == "workspace-only"));
    }

    #[test]
    fn mcp_servers_with_runtime_env_uses_server_env_as_override() {
        let mut config = test_config();
        config.mcp.servers.insert(
            "stdio".to_string(),
            McpServerConfig {
                transport: TransportType::Stdio,
                command: Some("server".to_string()),
                args: None,
                env: Some(HashMap::from([
                    ("OVERRIDE".to_string(), "server".to_string()),
                    ("SERVER_ONLY".to_string(), "1".to_string()),
                ])),
                url: None,
                headers: None,
                deferred: None,
                startup_timeout_ms: None,
            },
        );

        let output: Arc<dyn OutputSink> = Arc::new(NullSink);
        let bootstrap = AgentBootstrap::new(config, "/tmp", output).runtime_env(vec![
            ("OVERRIDE".to_string(), "runtime".to_string()),
            ("RUNTIME_ONLY".to_string(), "1".to_string()),
        ]);

        let servers = bootstrap.mcp_servers_with_runtime_env();
        let env = servers
            .get("stdio")
            .and_then(|server| server.env.as_ref())
            .expect("stdio server env should exist");

        assert_eq!(env.get("OVERRIDE").map(String::as_str), Some("server"));
        assert_eq!(env.get("SERVER_ONLY").map(String::as_str), Some("1"));
        assert_eq!(env.get("RUNTIME_ONLY").map(String::as_str), Some("1"));
    }

    #[tokio::test]
    async fn tool_search_snapshot_excludes_policy_denied_tools() {
        let output: Arc<dyn OutputSink> = Arc::new(NullSink);
        let bootstrap = AgentBootstrap::new(test_config(), "/tmp", output)
            .tool_policy(ToolPolicy::allow_only(["ToolSearch", "AllowedDeferred"]));
        let mut registry = ToolRegistry::new();
        registry.register(Box::new(DeferredTestTool("AllowedDeferred")));
        registry.register(Box::new(DeferredTestTool("DeniedDeferred")));

        bootstrap.register_tool_search(&mut registry);

        let tool_search = registry.get("ToolSearch").expect("ToolSearch should be registered");
        let allowed = tool_search.execute(json!({"query": "AllowedDeferred"})).await;
        let denied = tool_search.execute(json!({"query": "DeniedDeferred"})).await;

        assert!(allowed.content.contains("AllowedDeferred"));
        assert!(denied.content.starts_with("No deferred tools matching"));
        assert!(!denied.content.contains("\"name\": \"DeniedDeferred\""));
    }

    #[tokio::test]
    async fn auto_approve_handle_is_shared_and_revocable() {
        let mut config = test_config();
        config.tools.auto_approve = true;
        let workspace = tempfile::TempDir::new().unwrap();
        let output: Arc<dyn OutputSink> = Arc::new(NullSink);
        let result = AgentBootstrap::new(config, workspace.path().to_string_lossy(), output)
            .build()
            .await
            .unwrap();
        let handle = result.engine.auto_approve_handle();
        assert!(handle.load(std::sync::atomic::Ordering::SeqCst));

        handle.store(false, std::sync::atomic::Ordering::SeqCst);
        assert!(
            !result
                .engine
                .auto_approve_handle()
                .load(std::sync::atomic::Ordering::SeqCst),
            "engine confirmer must observe the host-owned flag"
        );
    }

    #[test]
    fn mcp_server_env_strips_aionui_runtime_vars() {
        let mut config = test_config();
        config.mcp.servers.insert(
            "third-party".to_string(),
            McpServerConfig {
                transport: TransportType::Stdio,
                command: Some("server".to_string()),
                args: None,
                env: None,
                url: None,
                headers: None,
                deferred: None,
                startup_timeout_ms: None,
            },
        );

        let output: Arc<dyn OutputSink> = Arc::new(NullSink);
        let bootstrap = AgentBootstrap::new(config, "/tmp", output).runtime_env(vec![
            ("AIONUI_RUNTIME_TOKEN".to_string(), "secret".to_string()),
            ("AIONUI_BASE_URL".to_string(), "http://127.0.0.1:1".to_string()),
            ("aionui_lowercase".to_string(), "x".to_string()),
            ("PATH_HINT".to_string(), "keep".to_string()),
        ]);

        let servers = bootstrap.mcp_servers_with_runtime_env();
        let env = servers["third-party"].env.as_ref().unwrap();
        assert!(!env.contains_key("AIONUI_RUNTIME_TOKEN"), "runtime token leaked to MCP");
        assert!(!env.contains_key("AIONUI_BASE_URL"));
        assert!(!env.contains_key("aionui_lowercase"));
        assert_eq!(env.get("PATH_HINT").map(String::as_str), Some("keep"));
    }

    #[test]
    fn mcp_server_env_passthrough_and_explicit_bridge_env_survive() {
        let runtime_env = vec![
            ("AIONUI_RUNTIME_TOKEN".to_string(), "secret".to_string()),
            ("AIONUI_CONVERSATION_ID".to_string(), "conv-1".to_string()),
        ];
        let bridge_env = HashMap::from([
            ("TEAM_MCP_PORT".to_string(), "4321".to_string()),
            ("TEAM_MCP_TOKEN".to_string(), "bridge".to_string()),
        ]);

        let env = build_mcp_server_env(&runtime_env, Some(bridge_env), &["AIONUI_CONVERSATION_ID".to_string()]);

        assert!(!env.contains_key("AIONUI_RUNTIME_TOKEN"));
        assert_eq!(env.get("AIONUI_CONVERSATION_ID").map(String::as_str), Some("conv-1"));
        assert_eq!(env.get("TEAM_MCP_PORT").map(String::as_str), Some("4321"));
        assert_eq!(env.get("TEAM_MCP_TOKEN").map(String::as_str), Some("bridge"));
        assert!(is_mcp_private_env("AIONUI_RUNTIME_TOKEN"));
        assert!(!is_mcp_private_env("TEAM_MCP_TOKEN"));
    }

    #[tokio::test]
    async fn tool_search_not_registered_without_deferred_tools() {
        let output: Arc<dyn OutputSink> = Arc::new(NullSink);
        let bootstrap = AgentBootstrap::new(test_config(), "/tmp", output);
        let mut registry = bootstrap.build_builtin_registry(std::path::Path::new("/tmp"));

        bootstrap.register_tool_search(&mut registry);

        assert!(
            registry.get("ToolSearch").is_none(),
            "ToolSearch must not exist without deferred tools"
        );
    }

    #[tokio::test]
    async fn tool_search_not_registered_when_only_policy_denied_tools_are_deferred() {
        let output: Arc<dyn OutputSink> = Arc::new(NullSink);
        let bootstrap = AgentBootstrap::new(test_config(), "/tmp", output)
            .tool_policy(ToolPolicy::allow_only(["ToolSearch", "Read"]));
        let mut registry = ToolRegistry::new();
        registry.register(Box::new(DeferredTestTool("DeniedDeferred")));

        bootstrap.register_tool_search(&mut registry);

        assert!(registry.get("ToolSearch").is_none());
    }

    /// Policy without any deferred built-in (Spawn and the plan tools are
    /// deferred, so they are left out).
    fn no_deferred_policy(extra: &[&str]) -> ToolPolicy {
        let mut names = vec![
            "Read",
            "Write",
            "Edit",
            "ExecCommand",
            "Grep",
            "Glob",
            "Skill",
            "ToolSearch",
        ];
        names.extend_from_slice(extra);
        ToolPolicy::allow_only(names)
    }

    #[tokio::test]
    async fn built_engine_without_deferred_tools_has_no_tool_search() {
        let workspace = tempfile::TempDir::new().unwrap();
        let output: Arc<dyn OutputSink> = Arc::new(NullSink);
        let result = AgentBootstrap::new(test_config(), workspace.path().to_string_lossy(), output)
            .tool_policy(no_deferred_policy(&[]))
            .build()
            .await
            .unwrap();

        let names = result.engine.tool_names();
        assert!(!names.iter().any(|n| n == "ToolSearch"), "{names:?}");
        assert!(names.iter().any(|n| n == "Skill"));
        assert!(
            result.engine.approval_link().is_attached(),
            "spawner must see the parent's approval wiring"
        );
    }

    #[tokio::test]
    async fn built_engine_keeps_tool_search_when_deferred_spawn_is_available() {
        let workspace = tempfile::TempDir::new().unwrap();
        let output: Arc<dyn OutputSink> = Arc::new(NullSink);
        let result = AgentBootstrap::new(test_config(), workspace.path().to_string_lossy(), output)
            .build()
            .await
            .unwrap();

        let names = result.engine.tool_names();
        assert!(names.iter().any(|n| n == "Spawn"));
        assert!(names.iter().any(|n| n == "ToolSearch"), "Spawn is deferred: {names:?}");
    }

    #[tokio::test]
    async fn refresh_tool_search_tracks_runtime_deferred_tools() {
        let workspace = tempfile::TempDir::new().unwrap();
        let output: Arc<dyn OutputSink> = Arc::new(NullSink);
        let mut engine = AgentBootstrap::new(test_config(), workspace.path().to_string_lossy(), output)
            .tool_policy(no_deferred_policy(&["LateDeferred"]))
            .build()
            .await
            .unwrap()
            .engine;
        assert!(!engine.tool_names().iter().any(|n| n == "ToolSearch"));

        engine
            .registry_mut()
            .register(Box::new(DeferredTestTool("LateDeferred")));
        engine.refresh_tool_search();
        assert_eq!(
            engine
                .tool_names()
                .iter()
                .filter(|n| n.as_str() == "ToolSearch")
                .count(),
            1
        );

        engine.refresh_tool_search();
        assert_eq!(
            engine
                .tool_names()
                .iter()
                .filter(|n| n.as_str() == "ToolSearch")
                .count(),
            1,
            "refresh must not duplicate ToolSearch"
        );
    }
}
