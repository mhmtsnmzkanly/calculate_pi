//! Runs without any user interface for servers and long unattended computations.
use crate::app_state::Shared;
use crate::record::UiSnapshot;
use crate::view::Stats;
use std::thread;
use std::time::{Duration, Instant};

const PROGRESS_INTERVAL: Duration = Duration::from_secs(10);
const POLL: Duration = Duration::from_millis(100);

fn progress_line(snapshot: &UiSnapshot, session_start: Instant) -> String {
    let stats = Stats::new(snapshot, session_start);
    format!(
        "hex={} decimal={} last10_ms={:.3} session_s={:.0}",
        snapshot.hex_count,
        snapshot.decimal_count,
        stats.last10_average_s * 1_000.0,
        stats.session_s,
    )
}

/// Prints progress until the worker stops. Failures reach the caller through
/// the worker's `join`.
pub fn run(state: Shared) {
    let session_start = Instant::now();
    let mut last_report = session_start;
    while state.is_running() {
        thread::sleep(POLL);
        if last_report.elapsed() < PROGRESS_INTERVAL {
            continue;
        }
        last_report = Instant::now();
        if let Some(snapshot) = state.snapshot_with_limits(0, 0) {
            println!("{}", progress_line(&snapshot, session_start));
        }
    }
    if let Some(snapshot) = state.snapshot_with_limits(0, 0) {
        println!("{}", progress_line(&snapshot, session_start));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn progress_line_is_machine_readable() {
        let snapshot = UiSnapshot {
            hex_count: 40,
            decimal_count: 48,
            total_elapsed_s: 1.0,
            fastest_s: 0.0,
            slowest_s: 0.0,
            hex_display: String::new(),
            decimal_display: String::new(),
            digit_times: vec![0.002],
        };
        let line = progress_line(&snapshot, Instant::now());
        assert!(
            line.starts_with("hex=40 decimal=48 last10_ms=2.000 "),
            "{line}"
        );
    }
}
