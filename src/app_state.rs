//! Concurrent state shared between the computation worker thread and UI frontends.
use crate::converter::DecimalConverter;
use crate::lock::{update_lock_state, LockState};
use crate::record::{CoreState, Record, UiSnapshot};
use std::sync::{
    atomic::{AtomicBool, AtomicU64, Ordering},
    Arc, Mutex, MutexGuard,
};
use std::time::{Duration, Instant};

const STATUS_LIFETIME: Duration = Duration::from_secs(3);
const MAX_RECENT_LOGS: usize = 21;

#[derive(Clone)]
pub struct LogEntry {
    pub index: u64,
    pub hex: char,
    pub decimal: String,
    pub duration: f64,
}

pub struct SharedState {
    core: Mutex<CoreState>,
    recent_logs: Mutex<Vec<LogEntry>>,
    status: Mutex<(String, Instant)>,
    fatal_error: Mutex<Option<String>>,
    target: AtomicU64,
    running: AtomicBool,
    paused: AtomicBool,
    save_request: AtomicBool,
    pub record_path: String,
}
pub type Shared = Arc<SharedState>;

/// Log and status data are read on a best-effort basis; poisoned locks are recovered.
fn lock_ui<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poison| poison.into_inner())
}

impl SharedState {
    pub fn new(record: Record, converter: DecimalConverter, record_path: String) -> Shared {
        Arc::new(Self {
            core: Mutex::new(CoreState { record, converter }),
            recent_logs: Mutex::new(Vec::new()),
            status: Mutex::new((String::new(), Instant::now())),
            fatal_error: Mutex::new(None),
            target: AtomicU64::new(0),
            running: AtomicBool::new(true),
            paused: AtomicBool::new(false),
            save_request: AtomicBool::new(false),
            record_path,
        })
    }

    pub fn set_target(&self, target: Option<u64>) {
        self.target.store(target.unwrap_or(0), Ordering::Release);
    }

    pub fn target(&self) -> Option<u64> {
        match self.target.load(Ordering::Acquire) {
            0 => None,
            n => Some(n),
        }
    }

    pub fn fatal_error(&self) -> Option<String> {
        lock_ui(&self.fatal_error).clone()
    }

    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::Acquire)
    }

    pub fn is_paused(&self) -> bool {
        self.paused.load(Ordering::Acquire)
    }

    pub fn request_stop(&self) {
        self.running.store(false, Ordering::Release);
    }

    pub fn request_save(&self) {
        self.save_request.store(true, Ordering::Release);
    }

    pub fn take_save_request(&self) -> bool {
        self.save_request.swap(false, Ordering::AcqRel)
    }

    pub fn set_paused(&self, paused: bool) -> std::io::Result<()> {
        let previous = self.paused.swap(paused, Ordering::AcqRel);
        let lock_state = if paused {
            LockState::Paused
        } else {
            LockState::Running
        };
        update_lock_state(&self.record_path, lock_state).inspect_err(|_| {
            self.paused.store(previous, Ordering::Release);
        })
    }

    pub fn toggle_pause(&self) -> std::io::Result<()> {
        self.set_paused(!self.is_paused())
    }

    /// Core computation state. A poisoned lock is never recovered: the application
    /// stops safely rather than continuing with suspect digits.
    pub fn core(&self) -> Option<MutexGuard<'_, CoreState>> {
        match self.core.lock() {
            Ok(guard) => Some(guard),
            Err(_) => {
                self.fail("Critical error: core state poisoned; stopping");
                None
            }
        }
    }

    pub fn snapshot(&self) -> Option<UiSnapshot> {
        self.core().map(|core| core.ui_snapshot())
    }

    pub fn snapshot_with_limits(
        &self,
        hex_limit: usize,
        decimal_limit: usize,
    ) -> Option<UiSnapshot> {
        self.core()
            .map(|core| core.ui_snapshot_with_limits(hex_limit, decimal_limit))
    }

    pub fn fail(&self, message: impl Into<String>) {
        let message = message.into();
        self.request_stop();
        *lock_ui(&self.fatal_error) = Some(message.clone());
        self.set_status(message);
    }

    pub fn set_status(&self, message: impl Into<String>) {
        *lock_ui(&self.status) = (message.into(), Instant::now());
    }

    /// Returns the current status message if its lifetime has not expired, or an empty string otherwise.
    pub fn status_text(&self) -> String {
        let status = lock_ui(&self.status);
        if status.1.elapsed() < STATUS_LIFETIME {
            status.0.clone()
        } else {
            String::new()
        }
    }

    /// Most recently generated digits (newest first).
    pub fn recent_logs(&self) -> Vec<LogEntry> {
        lock_ui(&self.recent_logs).clone()
    }

    /// Prepends a batch of digit entries ordered from oldest to newest into the stream.
    pub fn push_logs(&self, entries: Vec<LogEntry>) {
        let mut logs = lock_ui(&self.recent_logs);
        logs.splice(0..0, entries.into_iter().rev());
        logs.truncate(MAX_RECENT_LOGS);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state(path: String) -> Shared {
        SharedState::new(Record::default(), DecimalConverter::default(), path)
    }

    fn entry(index: u64) -> LogEntry {
        LogEntry {
            index,
            hex: '0',
            decimal: String::new(),
            duration: 0.0,
        }
    }

    #[test]
    fn logs_stay_newest_first_and_bounded() {
        let state = state(String::new());
        state.push_logs((1..=15).map(entry).collect());
        state.push_logs((16..=30).map(entry).collect());
        let logs = state.recent_logs();
        assert_eq!(logs.len(), MAX_RECENT_LOGS);
        assert_eq!(logs[0].index, 30);
        assert_eq!(logs[20].index, 10);
    }

    #[test]
    fn failed_pause_update_restores_previous_state() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("no-lock.json");
        let state = state(path.to_str().unwrap().to_string());
        assert!(state.toggle_pause().is_err());
        assert!(!state.is_paused());
    }

    #[test]
    fn failure_stops_and_reports() {
        let state = state(String::new());
        state.fail("boom");
        assert!(!state.is_running());
        assert_eq!(state.status_text(), "boom");
        assert_eq!(state.fatal_error().as_deref(), Some("boom"));
    }

    #[test]
    fn save_request_is_consumed_once() {
        let state = state(String::new());
        state.request_save();
        assert!(state.take_save_request());
        assert!(!state.take_save_request());
    }
}
