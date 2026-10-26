//! A store in memory and an adapter that records what it's given.

use std::sync::Mutex;

use harness_contracts::{Level, Notification, NotificationId, NotificationState, ThreadId};

use crate::{Adapter, Live, NotificationStore};

#[derive(Default)]
pub struct MemoryStore {
    rows: Mutex<Vec<Notification>>,
    /// Refuse writes (to show a failure changes nothing).
    pub refuse: std::sync::atomic::AtomicBool,
}

impl MemoryStore {
    fn rows(&self) -> std::sync::MutexGuard<'_, Vec<Notification>> {
        self.rows.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

impl NotificationStore for MemoryStore {
    fn open(&self) -> Result<Vec<Notification>, String> {
        Ok(self.rows().iter().filter(|n| n.state == NotificationState::Open).cloned().collect())
    }

    fn recent(&self, limit: usize) -> Result<Vec<Notification>, String> {
        Ok(self.rows().iter().rev().take(limit).cloned().collect())
    }

    fn get(&self, id: &NotificationId) -> Result<Option<Notification>, String> {
        Ok(self.rows().iter().find(|n| &n.id == id).cloned())
    }

    fn put(&self, notifications: &[Notification]) -> Result<(), String> {
        if self.refuse.load(std::sync::atomic::Ordering::Relaxed) {
            return Err("the disk is full".into());
        }
        let mut rows = self.rows();
        for n in notifications {
            match rows.iter_mut().find(|r| r.id == n.id) {
                Some(row) => *row = n.clone(),
                None => rows.push(n.clone()),
            }
        }
        Ok(())
    }

    fn count(&self, prefix: &str) -> Result<u32, String> {
        Ok(self.rows().iter().filter(|n| n.id.as_str().starts_with(prefix)).count() as u32)
    }

    fn expire_waiting(&self) -> Result<usize, String> {
        let mut rows = self.rows();
        let waiting: Vec<&mut Notification> =
            rows.iter_mut().filter(|n| n.state == NotificationState::Open && n.level == Level::Waiting).collect();
        let count = waiting.len();
        for n in waiting {
            n.state = NotificationState::Expired;
        }
        Ok(count)
    }
}

/// Records every notification it's handed.
#[derive(Default)]
pub struct Recorder(pub Mutex<Vec<Notification>>);

impl Adapter for Recorder {
    fn deliver(&self, notification: &Notification) {
        self.0.lock().unwrap_or_else(std::sync::PoisonError::into_inner).push(notification.clone());
    }
}

/// Sessions where the threads named were ended on purpose.
#[derive(Default)]
pub struct Ended(pub Mutex<Vec<ThreadId>>);

impl Live for Ended {
    fn closed_on_purpose(&self, thread: &ThreadId) -> bool {
        let mut ended = self.0.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let was = ended.contains(thread);
        ended.retain(|t| t != thread);
        was
    }
}
