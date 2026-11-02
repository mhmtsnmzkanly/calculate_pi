//! Presentation and statistics logic shared across terminal and desktop interfaces.
use crate::record::UiSnapshot;
use std::time::Instant;

pub struct Stats {
    pub average_s: f64,
    pub last10_average_s: f64,
    pub session_s: f64,
}

impl Stats {
    pub fn new(snapshot: &UiSnapshot, session_start: Instant) -> Self {
        let average_s = if snapshot.hex_count > 0 {
            snapshot.total_elapsed_s / snapshot.hex_count as f64
        } else {
            0.0
        };
        let last10_average_s = if snapshot.digit_times.is_empty() {
            0.0
        } else {
            snapshot.digit_times.iter().sum::<f64>() / snapshot.digit_times.len() as f64
        };
        Self {
            average_s,
            last10_average_s,
            session_s: session_start.elapsed().as_secs_f64(),
        }
    }
}

/// Selects up to `limit` trailing digits and returns whether they start from the
/// very first digit of π (`true`) or represent a truncated tail (`false`).
pub fn pi_tail(digits: &str, total: u64, limit: usize) -> (bool, &str) {
    let shown = digits
        .get(digits.len().saturating_sub(limit)..)
        .unwrap_or(digits);
    (shown.len() as u64 >= total, shown)
}

/// Prefixes `"3."` only when `digits` actually begins at the first digit of π.
pub fn pi_preview(digits: &str, total: u64) -> String {
    let (is_start, shown) = pi_tail(digits, total, digits.len());
    if is_start {
        format!("3.{shown}")
    } else {
        format!("…{shown}")
    }
}

pub struct ProgressInfo {
    pub target: u64,
    pub is_explicit_target: bool,
    pub fraction: f32,
    pub eta_s: Option<f64>,
}

impl ProgressInfo {
    pub fn new(hex_count: u64, explicit_target: Option<u64>, stats: &Stats) -> Self {
        let (target, is_explicit_target) = match explicit_target {
            Some(t) => (t.max(1), true),
            None => (next_milestone(hex_count), false),
        };
        let fraction = (hex_count as f64 / target as f64).clamp(0.0, 1.0) as f32;
        let remaining = target.saturating_sub(hex_count);
        let pace = if stats.last10_average_s > 0.0 {
            stats.last10_average_s
        } else {
            stats.average_s
        };
        let eta_s = if remaining == 0 {
            Some(0.0)
        } else if pace > 0.0 {
            Some(remaining as f64 * pace)
        } else {
            None
        };
        Self {
            target,
            is_explicit_target,
            fraction,
            eta_s,
        }
    }
}

pub fn next_milestone(hex_count: u64) -> u64 {
    if hex_count < 1_000 {
        1_000
    } else if hex_count < 10_000 {
        10_000
    } else if hex_count < 50_000 {
        50_000
    } else {
        ((hex_count / 50_000) + 1) * 50_000
    }
}

pub fn format_count(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (idx, ch) in digits.chars().enumerate() {
        if idx > 0 && (digits.len() - idx).is_multiple_of(3) {
            out.push(',');
        }
        out.push(ch);
    }
    out
}

pub fn format_duration(seconds: f64) -> String {
    if !seconds.is_finite() || seconds < 0.0 {
        return "—".to_string();
    }
    if seconds < 60.0 {
        return format!("{seconds:.1}s");
    }
    let total_secs = seconds.round() as u64;
    let mins = (total_secs / 60) % 60;
    let secs = total_secs % 60;
    let hours = total_secs / 3600;
    if hours == 0 {
        format!("{mins}m {secs:02}s")
    } else {
        format!("{hours}h {mins:02}m {secs:02}s")
    }
}

pub fn accumulate_hex_frequencies(counts: &mut [u64; 16], digits: &str) {
    for b in digits.bytes() {
        let idx = match b {
            b'0'..=b'9' => (b - b'0') as usize,
            b'a'..=b'f' => (b - b'a' + 10) as usize,
            b'A'..=b'F' => (b - b'A' + 10) as usize,
            _ => continue,
        };
        counts[idx] += 1;
    }
}

pub fn accumulate_decimal_frequencies(counts: &mut [u64; 10], digits: &str) {
    for b in digits.bytes() {
        if b.is_ascii_digit() {
            counts[(b - b'0') as usize] += 1;
        }
    }
}

pub const MAX_SEARCH_MATCHES: usize = 10_000;

/// Performs a case-insensitive ASCII substring search over digit strings,
/// returning up to [`MAX_SEARCH_MATCHES`] starting indices.
pub fn find_digit_matches(haystack: &str, needle: &str) -> Vec<usize> {
    let needle = needle.trim();
    if needle.is_empty() || needle.len() > haystack.len() {
        return Vec::new();
    }
    let hay_bytes = haystack.as_bytes();
    let needle_bytes: Vec<u8> = needle.bytes().map(|b| b.to_ascii_uppercase()).collect();
    let first = needle_bytes[0];
    let len = needle_bytes.len();
    let limit = hay_bytes.len() - len;
    let mut matches = Vec::new();
    for i in 0..=limit {
        if hay_bytes[i].to_ascii_uppercase() == first
            && hay_bytes[i + 1..i + len]
                .iter()
                .zip(&needle_bytes[1..])
                .all(|(h, n)| h.to_ascii_uppercase() == *n)
        {
            matches.push(i);
            if matches.len() >= MAX_SEARCH_MATCHES {
                break;
            }
        }
    }
    matches
}

/// Returns the index of the match in the sorted `matches` slice that covers character position `pos`.
pub fn match_covering_pos(matches: &[usize], needle_len: usize, pos: usize) -> Option<usize> {
    if matches.is_empty() || needle_len == 0 {
        return None;
    }
    let idx = matches.partition_point(|&start| start <= pos);
    if idx == 0 {
        return None;
    }
    let candidate = idx - 1;
    if pos < matches[candidate] + needle_len {
        Some(candidate)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot(hex_count: u64, total: f64, times: Vec<f64>) -> UiSnapshot {
        UiSnapshot {
            hex_count,
            decimal_count: 0,
            total_elapsed_s: total,
            fastest_s: 0.0,
            slowest_s: 0.0,
            hex_display: String::new(),
            decimal_display: String::new(),
            digit_times: times,
        }
    }

    #[test]
    fn preview_marks_only_the_real_start_as_three_point() {
        assert_eq!(pi_preview("1415", 4), "3.1415");
        assert_eq!(pi_preview("1415", 5), "…1415");
        assert_eq!(pi_tail("14159", 5, 4), (false, "4159"));
        assert_eq!(pi_tail("4159", 5, 4), (false, "4159"));
        assert_eq!(pi_tail("1415", 4, 4), (true, "1415"));
    }

    #[test]
    fn stats_handle_an_empty_record() {
        let stats = Stats::new(&snapshot(0, 0.0, vec![]), Instant::now());
        assert_eq!(stats.average_s, 0.0);
        assert_eq!(stats.last10_average_s, 0.0);
    }

    #[test]
    fn stats_average_total_and_recent_digits() {
        let stats = Stats::new(&snapshot(4, 8.0, vec![1.0, 3.0]), Instant::now());
        assert_eq!(stats.average_s, 2.0);
        assert_eq!(stats.last10_average_s, 2.0);
    }

    #[test]
    fn frequencies_and_search_helpers_are_accurate() {
        let mut hex_counts = [0u64; 16];
        accumulate_hex_frequencies(&mut hex_counts, "243f6A88");
        assert_eq!(hex_counts[2], 1);
        assert_eq!(hex_counts[8], 2);
        assert_eq!(hex_counts[10], 1);
        assert_eq!(hex_counts[15], 1);

        let mut dec_counts = [0u64; 10];
        accumulate_decimal_frequencies(&mut dec_counts, "1415926535");
        assert_eq!(dec_counts[1], 2);
        assert_eq!(dec_counts[5], 3);

        let matches = find_digit_matches("14159265358979", "59");
        assert_eq!(matches, vec![3]);
        assert_eq!(match_covering_pos(&matches, 2, 2), None);
        assert_eq!(match_covering_pos(&matches, 2, 3), Some(0));
        assert_eq!(match_covering_pos(&matches, 2, 4), Some(0));
        assert_eq!(match_covering_pos(&matches, 2, 5), None);
    }

    #[test]
    fn progress_and_formatting_helpers_work() {
        let stats = Stats::new(&snapshot(500, 100.0, vec![0.1]), Instant::now());
        let progress = ProgressInfo::new(500, None, &stats);
        assert!(!progress.is_explicit_target);
        assert_eq!(progress.target, 1_000);
        assert!((progress.fraction - 0.5).abs() < 1e-6);
        assert!((progress.eta_s.unwrap() - 50.0).abs() < 1e-6);

        assert_eq!(format_count(138_167), "138,167");
        assert_eq!(format_duration(12.34), "12.3s");
        assert_eq!(format_duration(125.0), "2m 05s");
        assert_eq!(format_duration(3661.0), "1h 01m 01s");
    }
}
