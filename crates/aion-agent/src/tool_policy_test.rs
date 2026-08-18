use super::{ToolGateDecision, ToolGateDenial, ToolPolicy};

#[test]
fn unrestricted_policy_allows_every_tool() {
    assert!(ToolPolicy::Unrestricted.allows("ExecCommand"));
}

#[test]
fn denied_gate_cannot_be_reallowed() {
    let denied = ToolGateDecision::Deny(ToolGateDenial::Policy);

    assert_eq!(denied.and(ToolGateDecision::Allow), denied);
}

#[test]
fn later_denial_tightens_an_allowed_gate() {
    let capability_denied = ToolGateDecision::Deny(ToolGateDenial::Capability);

    assert_eq!(ToolGateDecision::Allow.and(capability_denied), capability_denied);
    assert!(capability_denied.is_denied());
}

#[test]
fn allow_only_policy_matches_exact_tool_names() {
    let policy = ToolPolicy::allow_only(["Read", "team_send_message"]);

    assert!(policy.allows("Read"));
    assert!(policy.allows("team_send_message"));
    assert!(!policy.allows("Write"));
    assert!(!policy.allows("read"));
}
