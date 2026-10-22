use super::{RESERVED_TOOL_NAMES, is_reserved_tool_name};

#[test]
fn control_plane_tools_are_reserved() {
    for name in ["Skill", "Spawn", "ToolSearch", "EnterPlanMode", "ExitPlanMode"] {
        assert!(is_reserved_tool_name(name), "{name} must be reserved");
    }
}

#[test]
fn reservation_is_case_insensitive() {
    assert!(is_reserved_tool_name("skill"));
    assert!(is_reserved_tool_name(" SPAWN "));
    assert!(!is_reserved_tool_name("search_docs"));
    assert!(!is_reserved_tool_name("mcp__srv_Skill"));
}

#[test]
fn table_has_no_duplicates() {
    let mut names: Vec<_> = RESERVED_TOOL_NAMES.iter().map(|n| n.to_ascii_lowercase()).collect();
    names.sort();
    let before = names.len();
    names.dedup();
    assert_eq!(before, names.len());
}
