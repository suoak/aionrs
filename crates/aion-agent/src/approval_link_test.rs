use std::sync::{Arc, Mutex};

use aion_protocol::ToolApprovalManager;

use super::ApprovalLink;
use crate::confirm::ToolConfirmer;

#[test]
fn empty_link_is_not_attached() {
    let link = ApprovalLink::new();
    let snapshot = link.snapshot();
    assert!(!link.is_attached());
    assert!(snapshot.confirmer.is_none());
    assert!(snapshot.approval_manager.is_none());
    assert!(snapshot.protocol_writer.is_none());
    assert!(snapshot.msg_id.is_empty());
}

#[test]
fn clones_observe_later_updates() {
    let link = ApprovalLink::new();
    let spawner_side = link.clone();

    let confirmer = Arc::new(Mutex::new(ToolConfirmer::new(false, vec![])));
    link.set_confirmer(Arc::clone(&confirmer));
    link.set_approval_manager(Some(Arc::new(ToolApprovalManager::new())));
    link.set_msg_id("msg-42");

    let snapshot = spawner_side.snapshot();
    assert!(spawner_side.is_attached());
    assert!(Arc::ptr_eq(snapshot.confirmer.as_ref().unwrap(), &confirmer));
    assert!(snapshot.approval_manager.is_some());
    assert_eq!(snapshot.msg_id, "msg-42");
}

#[test]
fn shared_confirmer_reflects_runtime_auto_approve_flip() {
    let link = ApprovalLink::new();
    let confirmer = Arc::new(Mutex::new(ToolConfirmer::new(false, vec![])));
    link.set_confirmer(Arc::clone(&confirmer));

    let child_view = link.snapshot().confirmer.unwrap();
    assert!(!child_view.lock().unwrap().is_auto_approve());

    confirmer.lock().unwrap().set_auto_approve(true);
    assert!(child_view.lock().unwrap().is_auto_approve());
}
