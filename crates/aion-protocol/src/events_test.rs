use super::*;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_ready_event_serialization() {
        let event = ProtocolEvent::Ready {
            version: "0.1.0".to_string(),
            session_id: Some("abc123".to_string()),
            capabilities: Capabilities {
                inject: true,
                tool_approval: true,
                image_input: ImageInputCapability::Supported,
                thinking: true,
                effort: false,
                effort_levels: vec![],
                modes: vec!["default".into(), "auto_edit".into(), "yolo".into()],
                current_mode: "default".into(),
                mcp: false,
            },
        };
        let json = serde_json::to_value(&event).unwrap();
        assert_eq!(json["type"], "ready");
        assert_eq!(json["version"], "0.1.0");
        assert_eq!(json["session_id"], "abc123");
        assert_eq!(json["capabilities"]["tool_approval"], true);

        // session_id omitted when None
        let event_no_sid = ProtocolEvent::Ready {
            version: "0.1.0".to_string(),
            session_id: None,
            capabilities: Capabilities {
                inject: true,
                tool_approval: true,
                image_input: ImageInputCapability::Unknown,
                thinking: true,
                effort: false,
                effort_levels: vec![],
                modes: vec!["default".into(), "auto_edit".into(), "yolo".into()],
                current_mode: "default".into(),
                mcp: false,
            },
        };
        let json2 = serde_json::to_value(&event_no_sid).unwrap();
        assert!(json2.get("session_id").is_none());
    }

    #[test]
    fn test_text_delta_event_serialization() {
        let event = ProtocolEvent::TextDelta {
            text: "hello".to_string(),
            msg_id: "m1".to_string(),
        };
        let json = serde_json::to_value(&event).unwrap();
        assert_eq!(json["type"], "text_delta");
        assert_eq!(json["text"], "hello");
        assert_eq!(json["msg_id"], "m1");
    }

    #[test]
    fn input_lifecycle_events_are_structured() {
        let accepted = serde_json::to_value(ProtocolEvent::InputAccepted {
            input_id: "input-1".into(),
        })
        .unwrap();
        assert_eq!(accepted, json!({"type": "input_accepted", "input_id": "input-1"}));

        let applied = serde_json::to_value(ProtocolEvent::InputApplied {
            input_id: "input-1".into(),
            turn_id: Some("turn-1".into()),
        })
        .unwrap();
        assert_eq!(applied["type"], "input_applied");
        assert_eq!(applied["turn_id"], "turn-1");

        let rejected = serde_json::to_value(ProtocolEvent::InputRejected {
            input_id: "input-2".into(),
            error_code: "too_late".into(),
        })
        .unwrap();
        assert_eq!(rejected["error_code"], "too_late");
    }

    #[test]
    fn test_tool_request_event_serialization() {
        let event = ProtocolEvent::ToolRequest {
            msg_id: "m1".to_string(),
            call_id: "c1".to_string(),
            execution_id: "exec-1".to_string(),
            tool: ToolInfo {
                name: "ExecCommand".to_string(),
                category: ToolCategory::Exec,
                args: json!({"cmd": "ls"}),
                description: "Execute: ls".to_string(),
            },
        };
        let json = serde_json::to_value(&event).unwrap();
        assert_eq!(json["type"], "tool_request");
        assert_eq!(json["execution_id"], "exec-1");
        assert_eq!(json["tool"]["category"], "exec");
    }

    #[test]
    fn test_tool_result_event_serialization() {
        let event = ProtocolEvent::ToolResult {
            msg_id: "m1".to_string(),
            call_id: "c1".to_string(),
            execution_id: "exec-1".to_string(),
            tool_name: "Read".to_string(),
            status: ToolStatus::Success,
            output: "file content".to_string(),
            output_type: OutputType::Text,
            metadata: None,
            content_blocks: None,
            structured_content: None,
            error_code: None,
            truncation: None,
        };
        let json = serde_json::to_value(&event).unwrap();
        assert_eq!(json["type"], "tool_result");
        assert_eq!(json["status"], "success");
        assert!(json.get("metadata").is_none());
    }

    #[test]
    fn tool_result_serializes_optional_structured_fields() {
        let event = ProtocolEvent::ToolResult {
            msg_id: "m1".to_string(),
            call_id: "c1".to_string(),
            execution_id: "exec-1".to_string(),
            tool_name: "mcp_tool".to_string(),
            status: ToolStatus::Error,
            output: "projection".to_string(),
            output_type: OutputType::Text,
            metadata: None,
            content_blocks: Some(vec![json!({"type": "image", "data": "AA==", "mimeType": "image/png"})]),
            structured_content: Some(json!({"rows": [1, 2]})),
            error_code: Some(aion_types::tool::ToolExecutionErrorCode::ExecutionFailed),
            truncation: Some(aion_types::tool::ToolResultTruncation {
                original_bytes: 20,
                output_bytes: 10,
                limit_bytes: 10,
            }),
        };

        let value = serde_json::to_value(event).unwrap();
        assert_eq!(value["content_blocks"][0]["type"], "image");
        assert_eq!(value["structured_content"]["rows"], json!([1, 2]));
        assert_eq!(value["error_code"], "execution_failed");
        assert_eq!(value["truncation"]["original_bytes"], 20);
    }

    #[test]
    fn tool_cancelled_serializes_stable_error_code() {
        let value = serde_json::to_value(ProtocolEvent::ToolCancelled {
            msg_id: "m1".to_owned(),
            call_id: "c1".to_owned(),
            execution_id: "exec-1".to_owned(),
            reason: "canceled".to_owned(),
            error_code: Some(aion_types::tool::ToolExecutionErrorCode::Canceled),
        })
        .unwrap();

        assert_eq!(value["error_code"], "canceled");
    }

    #[test]
    fn test_error_event_serialization() {
        let event = ProtocolEvent::Error {
            msg_id: None,
            error: ErrorInfo {
                code: "rate_limit".to_string(),
                message: "Too many requests".to_string(),
                retryable: true,
            },
        };
        let json = serde_json::to_value(&event).unwrap();
        assert_eq!(json["type"], "error");
        assert!(json.get("msg_id").is_none());
        assert_eq!(json["error"]["retryable"], true);
    }

    #[test]
    fn test_stream_end_with_usage() {
        let event = ProtocolEvent::StreamEnd {
            msg_id: "m1".to_string(),
            usage: Some(Usage {
                input_tokens: 100,
                output_tokens: 50,
                cache_read_tokens: Some(20),
                cache_write_tokens: None,
            }),
        };
        let json = serde_json::to_value(&event).unwrap();
        assert_eq!(json["type"], "stream_end");
        assert_eq!(json["usage"]["input_tokens"], 100);
        assert!(json["usage"].get("cache_write_tokens").is_none());
    }

    #[test]
    fn test_tool_category_display() {
        assert_eq!(ToolCategory::Info.to_string(), "info");
        assert_eq!(ToolCategory::Edit.to_string(), "edit");
        assert_eq!(ToolCategory::Exec.to_string(), "exec");
        assert_eq!(ToolCategory::Mcp.to_string(), "mcp");
    }

    #[test]
    fn test_ready_event_with_expanded_capabilities() {
        let event = ProtocolEvent::Ready {
            version: "0.2.0".to_string(),
            session_id: Some("abc".to_string()),
            capabilities: Capabilities {
                inject: true,
                tool_approval: true,
                image_input: ImageInputCapability::Supported,
                thinking: true,
                effort: true,
                effort_levels: vec!["low".into(), "medium".into(), "high".into()],
                modes: vec!["default".into(), "auto_edit".into(), "yolo".into()],
                current_mode: "default".into(),
                mcp: false,
            },
        };
        let json = serde_json::to_value(&event).unwrap();
        assert_eq!(json["capabilities"]["thinking"], true);
        assert_eq!(json["capabilities"]["image_input"], "supported");
        assert_eq!(json["capabilities"]["effort"], true);
        assert_eq!(json["capabilities"]["effort_levels"][0], "low");
        assert_eq!(json["capabilities"]["modes"][2], "yolo");
    }

    #[test]
    fn test_mcp_ready_event_serialization() {
        let event = ProtocolEvent::McpReady {
            name: "team-tools".to_string(),
            tools: vec!["team_send_message".into(), "team_task_create".into()],
        };
        let json = serde_json::to_value(&event).unwrap();
        assert_eq!(json["type"], "mcp_ready");
        assert_eq!(json["name"], "team-tools");
        assert_eq!(json["tools"][0], "team_send_message");
        assert_eq!(json["tools"][1], "team_task_create");
        assert_eq!(json["tools"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn test_pong_event_serialization() {
        let event = ProtocolEvent::Pong;
        let json = serde_json::to_value(&event).unwrap();
        assert_eq!(json["type"], "pong");
        assert_eq!(json.as_object().unwrap().len(), 1);
    }

    #[test]
    fn test_config_changed_event_serialization() {
        let event = ProtocolEvent::ConfigChanged {
            capabilities: Capabilities {
                inject: true,
                tool_approval: true,
                image_input: ImageInputCapability::Unsupported,
                thinking: false,
                effort: true,
                effort_levels: vec!["low".into(), "medium".into(), "high".into()],
                modes: vec!["default".into(), "auto_edit".into(), "yolo".into()],
                current_mode: "default".into(),
                mcp: true,
            },
        };
        let json = serde_json::to_value(&event).unwrap();
        assert_eq!(json["type"], "config_changed");
        assert_eq!(json["capabilities"]["thinking"], false);
        assert_eq!(json["capabilities"]["effort"], true);
        assert_eq!(json["capabilities"]["image_input"], "unsupported");
    }
}
