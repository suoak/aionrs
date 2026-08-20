use super::*;

#[cfg(test)]
mod tests {
    use super::*;
    use aion_protocol::events::ToolCategory;
    use aion_types::tool::ToolExecutionErrorCode;
    use serde_json::json;

    #[test]
    fn truncate_utf8_ascii_within_limit() {
        assert_eq!(truncate_utf8("hello", 80), "hello");
    }

    #[test]
    fn truncate_utf8_ascii_at_boundary() {
        assert_eq!(truncate_utf8("abcde", 3), "abc");
    }

    #[test]
    fn truncate_utf8_multibyte_snaps_back() {
        // '些' is 3 bytes (E4 BA 9B) starting at index 79 would span 79..82
        let s = "# 用 script 模拟 TTY 交互来添加 DeepSeek 提供商\n# 首先看看有哪些";
        let result = truncate_utf8(s, 80);
        assert!(result.len() <= 80);
        assert!(result.is_char_boundary(result.len()));
    }

    #[test]
    fn truncate_utf8_empty() {
        assert_eq!(truncate_utf8("", 80), "");
    }

    #[test]
    fn truncate_utf8_zero_limit() {
        assert_eq!(truncate_utf8("hello", 0), "");
    }

    #[test]
    fn truncate_utf8_emoji() {
        // 🦀 is 4 bytes
        let s = "aaa🦀bbb";
        assert_eq!(truncate_utf8(s, 4), "aaa");
        assert_eq!(truncate_utf8(s, 7), "aaa🦀");
    }

    struct BlockingTool;

    #[async_trait::async_trait]
    impl Tool for BlockingTool {
        fn name(&self) -> &str {
            "Blocking"
        }

        fn description(&self) -> &str {
            "Blocks until canceled"
        }

        fn input_schema(&self) -> JsonSchema {
            json!({"type": "object"})
        }

        fn is_concurrency_safe(&self, _input: &Value) -> bool {
            false
        }

        async fn execute(&self, _input: Value) -> ToolResult {
            std::future::pending().await
        }

        fn category(&self) -> ToolCategory {
            ToolCategory::Exec
        }
    }

    #[tokio::test]
    async fn context_cancellation_stops_the_tool_future() {
        let cancellation = CancellationToken::new();
        let context = ToolCallContext {
            execution_id: "execution-1".to_owned(),
            cancellation: cancellation.clone(),
        };
        cancellation.cancel();

        let output = BlockingTool.execute_with_context(json!({}), &context).await;

        assert!(output.result.is_error);
        assert_eq!(output.error_code, Some(ToolExecutionErrorCode::Canceled));
    }
}
