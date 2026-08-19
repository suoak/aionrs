use std::sync::{Arc, Mutex};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PendingInjection {
    pub input_id: String,
    pub content: String,
}

/// Cloneable host handle for queuing input while an agent run owns the engine.
#[derive(Debug, Clone, Default)]
pub struct InjectionHandle {
    pending: Arc<Mutex<Vec<PendingInjection>>>,
}

impl InjectionHandle {
    pub fn enqueue(&self, input_id: String, content: String) {
        self.pending
            .lock()
            .unwrap()
            .push(PendingInjection { input_id, content });
    }

    pub(crate) fn drain(&self) -> Vec<PendingInjection> {
        std::mem::take(&mut *self.pending.lock().unwrap())
    }
}

#[cfg(test)]
#[path = "injection_test.rs"]
mod injection_test;
