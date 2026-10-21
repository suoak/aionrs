use super::*;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_auto_approve_always_allows() {
        let mut confirmer = ToolConfirmer::new(true, vec![]);
        assert_eq!(confirmer.check("ExecCommand", "echo hello"), ConfirmResult::Approved);
        assert_eq!(confirmer.check("Read", "/tmp/file"), ConfirmResult::Approved);
        assert_eq!(confirmer.check("Write", "/tmp/out"), ConfirmResult::Approved);
    }

    #[test]
    fn test_allowlist_contains_tool() {
        let mut confirmer = ToolConfirmer::new(false, vec!["Read".into(), "Write".into()]);
        assert_eq!(confirmer.check("Read", "/tmp/file"), ConfirmResult::Approved);
        assert_eq!(confirmer.check("Write", "/tmp/out"), ConfirmResult::Approved);
    }

    #[test]
    fn test_allowlist_approves_even_when_auto_approve_is_false() {
        let mut confirmer = ToolConfirmer::new(false, vec!["Read".into()]);
        assert_eq!(confirmer.check("Read", "/some/path"), ConfirmResult::Approved);
    }

    // Phase 6: add_to_allow_list() grants runtime approval
    #[test]
    fn test_add_to_allow_list_grants_approval() {
        let mut confirmer = ToolConfirmer::new(false, vec![]);
        // Before: tool not in list (would prompt — skip interactive check, just verify membership)
        confirmer.add_to_allow_list("Write");
        // After: auto-approved without interactive prompt
        assert_eq!(confirmer.check("Write", "file.txt"), ConfirmResult::Approved);
    }

    // Phase 6: add_to_allow_list() is idempotent — adding twice has no bad effect
    #[test]
    fn test_add_to_allow_list_idempotent() {
        let mut confirmer = ToolConfirmer::new(false, vec![]);
        confirmer.add_to_allow_list("ExecCommand");
        confirmer.add_to_allow_list("ExecCommand"); // duplicate — HashSet, no panic
        assert_eq!(confirmer.check("ExecCommand", "echo hi"), ConfirmResult::Approved);
    }

    // Phase 6: add_to_allow_list() does not affect unrelated tools
    #[test]
    fn test_add_to_allow_list_does_not_affect_other_tools() {
        let mut confirmer = ToolConfirmer::new(false, vec![]);
        confirmer.add_to_allow_list("Read");
        // Write is not in the list — check returns non-Approved for non-interactive
        // (we cannot test interactive input; verify Read is approved and Write is not in list)
        assert_eq!(confirmer.check("Read", "file.txt"), ConfirmResult::Approved);
        // We can't test the Denied path without stdin, but we verify allow_list state:
        assert!(confirmer.allow_list.contains("Read"));
        assert!(!confirmer.allow_list.contains("Write"));
    }

    #[test]
    fn shared_auto_approve_flag_can_be_revoked_at_runtime() {
        let mut confirmer = ToolConfirmer::new(true, vec![]);
        let flag = confirmer.auto_approve_flag();
        assert!(confirmer.is_auto_approve());
        assert_eq!(confirmer.check("ExecCommand", "rm -rf x"), ConfirmResult::Approved);

        // Host leaves YOLO mode: the very next decision must stop auto-approving.
        flag.store(false, std::sync::atomic::Ordering::SeqCst);
        assert!(!confirmer.is_auto_approve());

        confirmer.set_auto_approve(true);
        assert!(flag.load(std::sync::atomic::Ordering::SeqCst));
    }

    #[test]
    fn share_auto_approve_replaces_the_flag() {
        let mut confirmer = ToolConfirmer::new(true, vec![]);
        let host_flag = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        confirmer.share_auto_approve(host_flag.clone());
        assert!(!confirmer.is_auto_approve());
        host_flag.store(true, std::sync::atomic::Ordering::SeqCst);
        assert!(confirmer.is_auto_approve());
    }
}
