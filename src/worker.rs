//! Background computation worker thread, autosaving, and lifecycle management.
use crate::{
    app_state::{LogEntry, Shared},
    math::{certified_hex_digits, MathError, MAX_BATCH_DIGITS},
    storage::save_record_snapshot,
};
use std::{
    sync::Arc,
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};
use thiserror::Error;

const AUTOSAVE_INTERVAL: Duration = Duration::from_secs(30);
const PAUSE_POLL: Duration = Duration::from_millis(50);

#[derive(Debug, Error)]
pub enum WorkerError {
    #[error("worker thread panicked")]
    Panic,
    #[error("core state lock poisoned")]
    CorePoisoned,
    #[error("certified computation failed: {0}")]
    Compute(#[from] MathError),
    #[error("save failed: {0}")]
    Save(#[source] std::io::Error),
    #[error("base converter error: {0}")]
    Converter(String),
    #[error("{computation}; additionally, final save failed: {save}")]
    ComputeAndSave {
        computation: Box<WorkerError>,
        save: Box<WorkerError>,
    },
}

pub struct WorkerHandle {
    state: Shared,
    join: Option<JoinHandle<Result<(), WorkerError>>>,
}

impl WorkerHandle {
    /// Starts the computation worker. When `target` is provided, the worker stops
    /// automatically once the record reaches that many hex digits.
    pub fn start(state: Shared, target: Option<u64>) -> Self {
        state.set_target(target);
        let thread_state = Arc::clone(&state);
        let join = thread::spawn(move || run(thread_state, target));
        Self {
            state,
            join: Some(join),
        }
    }

    pub fn stop(&self) {
        self.state.request_stop();
    }

    pub fn join(mut self) -> Result<(), WorkerError> {
        match self.join.take().expect("worker is joined only once").join() {
            Ok(result) => result,
            Err(_) => Err(WorkerError::Panic),
        }
    }
}

fn run(state: Shared, target: Option<u64>) -> Result<(), WorkerError> {
    let computation = run_loop(&state, target);
    let final_save = save(&state);
    state.request_stop();
    let result = combine_results(computation, final_save);
    if let Err(error) = &result {
        state.fail(format!("Critical worker error: {error}"));
    }
    result
}

fn combine_results(
    computation: Result<(), WorkerError>,
    final_save: Result<(), WorkerError>,
) -> Result<(), WorkerError> {
    match (computation, final_save) {
        (Err(computation), Err(save)) => Err(WorkerError::ComputeAndSave {
            computation: Box::new(computation),
            save: Box::new(save),
        }),
        (Err(e), Ok(())) | (Ok(()), Err(e)) => Err(e),
        (Ok(()), Ok(())) => Ok(()),
    }
}

fn run_loop(state: &Shared, target: Option<u64>) -> Result<(), WorkerError> {
    let mut last_save = Instant::now();
    while state.is_running() {
        if state.take_save_request() || last_save.elapsed() >= AUTOSAVE_INTERVAL {
            save(state)?;
            last_save = Instant::now();
        }
        if state.is_paused() {
            thread::sleep(PAUSE_POLL);
            continue;
        }
        let n = state
            .core()
            .ok_or(WorkerError::CorePoisoned)?
            .record
            .next_hex_index;
        let wanted = match target.map(|target| target.saturating_sub(n)) {
            Some(0) => break,
            Some(remaining) => remaining.min(MAX_BATCH_DIGITS as u64) as usize,
            None => MAX_BATCH_DIGITS,
        };
        let start = Instant::now();
        let stopped = || !state.is_running();
        let digits = match certified_hex_digits(n, wanted, &stopped) {
            Ok(digits) => digits,
            Err(MathError::Cancelled { .. }) if stopped() => break,
            Err(e) => return Err(WorkerError::Compute(e)),
        };
        if stopped() {
            break;
        }
        let elapsed = start.elapsed().as_secs_f64() / digits.len() as f64;
        state.push_logs(commit_batch(state, n, digits, elapsed)?);
    }
    Ok(())
}

fn commit_batch(
    state: &Shared,
    first_index: u64,
    digits: Vec<u8>,
    elapsed: f64,
) -> Result<Vec<LogEntry>, WorkerError> {
    let mut core = state.core().ok_or(WorkerError::CorePoisoned)?;
    let decimals = core
        .commit_batch(&digits, elapsed)
        .map_err(WorkerError::Converter)?;
    Ok(digits
        .into_iter()
        .zip(decimals)
        .zip(first_index + 1..)
        .map(|((digit, decimal), index)| {
            let hex =
                char::from_digit(u32::from(digit), 16).map_or('?', |c| c.to_ascii_uppercase());
            LogEntry {
                index,
                hex,
                decimal,
                duration: elapsed,
            }
        })
        .collect())
}

fn save(state: &Shared) -> Result<(), WorkerError> {
    let snapshot = state
        .core()
        .ok_or(WorkerError::CorePoisoned)?
        .snapshot_record();
    save_record_snapshot(snapshot, &state.record_path).map_err(WorkerError::Save)?;
    state.set_status("Saved successfully");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{app_state::SharedState, converter::DecimalConverter, record::Record};
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn cancellation_performs_successful_final_save() {
        let dir = tempdir().unwrap();
        let path = dir
            .path()
            .join("worker-final-save.json")
            .to_str()
            .unwrap()
            .to_string();
        let state = SharedState::new(Record::default(), DecimalConverter::default(), path.clone());
        let worker = WorkerHandle::start(state, None);
        worker.stop();
        worker.join().unwrap();
        assert!(fs::metadata(&path).is_ok());
    }

    #[test]
    fn final_save_failure_reaches_join() {
        let dir = tempdir().unwrap();
        let path = dir
            .path()
            .join("missing-dir/record.json")
            .to_str()
            .unwrap()
            .to_string();
        let state = SharedState::new(Record::default(), DecimalConverter::default(), path);
        let worker = WorkerHandle::start(state, None);
        worker.stop();
        assert!(matches!(worker.join(), Err(WorkerError::Save(_))));
    }

    #[test]
    fn target_stops_exactly_and_saves() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("target.json").to_str().unwrap().to_string();
        let state = SharedState::new(Record::default(), DecimalConverter::default(), path.clone());
        WorkerHandle::start(state.clone(), Some(33)).join().unwrap();
        assert!(!state.is_running());
        let (record, _) = crate::storage::load_record(&path).unwrap();
        assert_eq!(record.hex_digits.len(), 33);
    }

    #[test]
    fn compute_and_save_failures_are_both_reported() {
        let compute = Err(WorkerError::Converter("bad digit".into()));
        let save = Err(WorkerError::Save(std::io::Error::other("disk full")));
        let message = combine_results(compute, save).unwrap_err().to_string();
        assert!(message.contains("bad digit"));
        assert!(message.contains("disk full"));
    }
}
