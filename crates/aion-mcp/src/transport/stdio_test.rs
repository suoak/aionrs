use std::collections::HashMap;

use super::{StdioTransport, inherited_private_env};

#[test]
fn inherited_private_env_lists_only_unprovided_aionui_vars() {
    // SAFETY: test-only mutation of a uniquely named variable.
    unsafe {
        std::env::set_var("AIONUI_STDIO_TEST_SECRET", "s3cret");
        std::env::set_var("AIONUI_STDIO_TEST_EXPLICIT", "parent");
    }
    let explicit = HashMap::from([("AIONUI_STDIO_TEST_EXPLICIT".to_string(), "child".to_string())]);

    let removed: Vec<String> = inherited_private_env(&explicit)
        .into_iter()
        .map(|name| name.to_string_lossy().into_owned())
        .collect();

    assert!(removed.iter().any(|name| name == "AIONUI_STDIO_TEST_SECRET"));
    assert!(!removed.iter().any(|name| name == "AIONUI_STDIO_TEST_EXPLICIT"));
    assert!(
        !removed
            .iter()
            .any(|name| !name.to_ascii_uppercase().starts_with("AIONUI_"))
    );
}

/// End-to-end: a spawned MCP server process never sees `AIONUI_RUNTIME_TOKEN`
/// inherited from the host process.
#[cfg(unix)]
#[tokio::test]
async fn spawned_mcp_server_does_not_inherit_runtime_token() {
    // SAFETY: test-only mutation; the name is only read by this test's child.
    unsafe {
        std::env::set_var("AIONUI_RUNTIME_TOKEN", "leaked-token");
    }
    let out = std::env::temp_dir().join(format!("aion-mcp-env-{}.txt", std::process::id()));
    let _ = std::fs::remove_file(&out);
    let env = HashMap::from([
        ("MCP_ENV_DUMP".to_string(), out.to_string_lossy().into_owned()),
        ("TEAM_MCP_TOKEN".to_string(), "bridge".to_string()),
    ]);

    let transport = StdioTransport::spawn(
        "sh",
        &["-c".to_string(), "env > \"$MCP_ENV_DUMP\"; sleep 5".to_string()],
        &env,
    )
    .await
    .expect("spawn sh");

    let mut dumped = String::new();
    for _ in 0..200 {
        if let Ok(text) = std::fs::read_to_string(&out)
            && text.contains("TEAM_MCP_TOKEN=")
        {
            dumped = text;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    drop(transport);
    let _ = std::fs::remove_file(&out);

    assert!(
        dumped.contains("TEAM_MCP_TOKEN=bridge"),
        "explicit env must reach the server"
    );
    assert!(
        !dumped.contains("AIONUI_RUNTIME_TOKEN"),
        "MCP server inherited AIONUI_RUNTIME_TOKEN:\n{dumped}"
    );
}
