use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

#[derive(Clone, Default)]
pub struct NotificationQueue {
    messages: Arc<Mutex<VecDeque<String>>>,
}

impl NotificationQueue {
    pub fn push(&self, message: impl Into<String>) {
        if let Ok(mut messages) = self.messages.lock() {
            messages.push_back(message.into());
        }
    }

    pub fn drain(&self) -> Vec<String> {
        let Ok(mut messages) = self.messages.lock() else {
            return Vec::new();
        };
        messages.drain(..).collect()
    }
}
