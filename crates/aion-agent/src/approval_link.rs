//! Shared approval wiring between a parent engine and the sub-agents it spawns.
//!
//! Sub-agents used to run with `auto_approve = true` hard-coded, so a parent
//! session in the default (ask) permission mode could launder `ExecCommand`
//! or file writes through `Spawn`. The [`ApprovalLink`] lets the spawner hand
//! each child the parent's live approval channel instead:
//!
//! - the parent's [`ToolConfirmer`] (and with it the session-wide runtime
//!   auto-approve flag that hosts flip when leaving / entering YOLO);
//! - the host [`ToolApprovalManager`] + [`ProtocolEmitter`] once the host
//!   installs them, so child tool calls surface as `tool_request` events;
//! - the parent's current message id, so the host can attach the request to
//!   the message that issued `Spawn`.
//!
//! The link is filled lazily: bootstrap creates the spawner before the host
//! calls `set_approval_manager` / `set_protocol_writer`, so the spawner reads
//! a snapshot at spawn time rather than at construction.

use std::sync::{Arc, Mutex, RwLock};

use aion_protocol::ToolApprovalManager;
use aion_protocol::writer::ProtocolEmitter;

use crate::confirm::ToolConfirmer;

#[derive(Default)]
struct ApprovalLinkState {
    confirmer: Option<Arc<Mutex<ToolConfirmer>>>,
    approval_manager: Option<Arc<ToolApprovalManager>>,
    protocol_writer: Option<Arc<dyn ProtocolEmitter>>,
    msg_id: String,
}

/// Cloneable handle shared by a parent engine and its spawner.
#[derive(Clone, Default)]
pub struct ApprovalLink {
    inner: Arc<RwLock<ApprovalLinkState>>,
}

/// Point-in-time view of the parent's approval wiring, taken when a child is
/// spawned.
#[derive(Clone, Default)]
pub struct ApprovalSnapshot {
    /// Parent confirmer. Children share it so the runtime auto-approve flag
    /// and interactive "always allow" decisions stay in sync with the parent.
    pub confirmer: Option<Arc<Mutex<ToolConfirmer>>>,
    /// Host approval manager (JSON-stream / embedded hosts).
    pub approval_manager: Option<Arc<ToolApprovalManager>>,
    /// Host protocol emitter paired with `approval_manager`.
    pub protocol_writer: Option<Arc<dyn ProtocolEmitter>>,
    /// Message id of the parent turn that is spawning the child.
    pub msg_id: String,
}

impl ApprovalLink {
    pub fn new() -> Self {
        Self::default()
    }

    fn write(&self) -> std::sync::RwLockWriteGuard<'_, ApprovalLinkState> {
        self.inner.write().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    pub(crate) fn set_confirmer(&self, confirmer: Arc<Mutex<ToolConfirmer>>) {
        self.write().confirmer = Some(confirmer);
    }

    pub(crate) fn set_approval_manager(&self, manager: Option<Arc<ToolApprovalManager>>) {
        self.write().approval_manager = manager;
    }

    pub(crate) fn set_protocol_writer(&self, writer: Option<Arc<dyn ProtocolEmitter>>) {
        self.write().protocol_writer = writer;
    }

    pub(crate) fn set_msg_id(&self, msg_id: &str) {
        self.write().msg_id = msg_id.to_owned();
    }

    /// Whether a parent engine has attached itself to this link.
    pub fn is_attached(&self) -> bool {
        self.snapshot().confirmer.is_some()
    }

    /// Read the parent's current approval wiring.
    pub fn snapshot(&self) -> ApprovalSnapshot {
        let state = self.inner.read().unwrap_or_else(|poisoned| poisoned.into_inner());
        ApprovalSnapshot {
            confirmer: state.confirmer.clone(),
            approval_manager: state.approval_manager.clone(),
            protocol_writer: state.protocol_writer.clone(),
            msg_id: state.msg_id.clone(),
        }
    }
}

#[cfg(test)]
#[path = "approval_link_test.rs"]
mod approval_link_test;
