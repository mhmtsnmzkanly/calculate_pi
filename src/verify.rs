//! Independent verification of recorded digits and migration of legacy v1 records.
use crate::bellard::bellard_hex_digits;
use crate::converter::DecimalConverter;
use crate::lock::OutputLock;
use crate::math::{certified_hex_digits, Cancel, MathError, MAX_BATCH_DIGITS};
use crate::record::{CoreState, Record};
use crate::storage::save_record_snapshot;
use std::fs;
use std::io;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};
use thiserror::Error;

/// Number of random windows spot-checked in addition to the tail when resuming a record.
const RESUME_SAMPLES: usize = 2;
pub const DEFAULT_SAMPLES: usize = 32;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Coverage {
    Samples(usize),
    Full,
}

#[derive(Debug, Error)]
pub enum VerifyError {
    #[error("hex digit mismatch at index {index}: record has {found:X}, Bellard formula gives {expected:X}")]
    Mismatch {
        index: usize,
        expected: u8,
        found: u8,
    },
    #[error("record contains a non-hex character at position {0}")]
    InvalidDigit(usize),
    #[error("verification cancelled")]
    Cancelled,
    #[error("verification failed: {0}")]
    Compute(MathError),
}

/// Number of hexadecimal digits compared by a completed verification run.
#[derive(Debug, PartialEq, Eq)]
pub struct VerifyReport {
    pub checked: usize,
    pub total: usize,
}

fn splitmix64(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// Sorted, disjoint windows that always include the newest digits plus `samples`
/// random positions. Each run probes fresh positions via a different `seed`.
fn sample_windows(len: usize, samples: usize, seed: u64) -> Vec<(usize, usize)> {
    if len == 0 {
        return Vec::new();
    }
    let mut state = seed;
    let mut starts: Vec<usize> = (0..samples)
        .map(|_| (splitmix64(&mut state) % len as u64) as usize)
        .collect();
    starts.push(len.saturating_sub(MAX_BATCH_DIGITS));
    starts.sort_unstable();
    let mut windows: Vec<(usize, usize)> = Vec::with_capacity(starts.len());
    for start in starts {
        let start = windows.last().map_or(start, |w| start.max(w.1));
        let end = (start + MAX_BATCH_DIGITS).min(len);
        if start < end {
            windows.push((start, end));
        }
    }
    windows
}

fn parse_digits(hex: &str) -> Result<Vec<u8>, VerifyError> {
    hex.chars()
        .enumerate()
        .map(|(i, c)| {
            c.to_digit(16)
                .map(|d| d as u8)
                .ok_or(VerifyError::InvalidDigit(i))
        })
        .collect()
}

fn check_range(
    digits: &[u8],
    (start, end): (usize, usize),
    cancel: &Cancel<'_>,
    on_digits: &mut dyn FnMut(usize),
) -> Result<(), VerifyError> {
    let mut index = start;
    while index < end {
        let wanted = (end - index).min(MAX_BATCH_DIGITS);
        let computed =
            bellard_hex_digits(index as u64, wanted, cancel).map_err(|error| match error {
                MathError::Cancelled { .. } => VerifyError::Cancelled,
                error => VerifyError::Compute(error),
            })?;
        for (offset, (&expected, &found)) in computed.iter().zip(&digits[index..]).enumerate() {
            if expected != found {
                return Err(VerifyError::Mismatch {
                    index: index + offset,
                    expected,
                    found,
                });
            }
        }
        index += computed.len();
        on_digits(computed.len());
    }
    Ok(())
}

fn verify_with_seed(
    hex: &str,
    coverage: Coverage,
    seed: u64,
    cancel: &Cancel<'_>,
    on_progress: &mut dyn FnMut(usize, usize),
) -> Result<VerifyReport, VerifyError> {
    let digits = parse_digits(hex)?;
    let windows = match coverage {
        Coverage::Full if digits.is_empty() => Vec::new(),
        Coverage::Full => vec![(0, digits.len())],
        Coverage::Samples(samples) => sample_windows(digits.len(), samples, seed),
    };
    let total: usize = windows.iter().map(|(start, end)| end - start).sum();
    let mut checked = 0;
    for window in windows {
        check_range(&digits, window, cancel, &mut |count| {
            checked += count;
            on_progress(checked, total);
        })?;
    }
    Ok(VerifyReport { checked, total })
}

fn time_seed() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_nanos() as u64)
}

/// Compares recorded digits against Bellard's formula. Unlike the record's SHA-256
/// checksum, this catches miscomputed digits in addition to post-write file corruption.
pub fn verify_digits(
    hex: &str,
    coverage: Coverage,
    cancel: &Cancel<'_>,
    on_progress: &mut dyn FnMut(usize, usize),
) -> Result<VerifyReport, VerifyError> {
    verify_with_seed(hex, coverage, time_seed(), cancel, on_progress)
}

/// Quick sampling check performed before resuming an existing record.
pub fn spot_check(hex: &str) -> Result<(), VerifyError> {
    let coverage = Coverage::Samples(RESUME_SAMPLES);
    verify_digits(hex, coverage, &|| false, &mut |_, _| {}).map(|_| ())
}

/// Re-verifies a legacy v1 record from scratch using the BBP engine, stopping if `cancel`
/// triggers. Nothing is written to `new_path` unless every digit is certified.
pub fn reverify_v1(old_path: &str, new_path: &str, cancel: &Cancel<'_>) -> io::Result<()> {
    let same_path = match (fs::canonicalize(old_path), fs::canonicalize(new_path)) {
        (Ok(old), Ok(new)) => old == new,
        _ => Path::new(old_path) == Path::new(new_path),
    };
    if same_path {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "input and output files must be different",
        ));
    }
    let _output_lock = OutputLock::acquire(new_path)?;
    if fs::symlink_metadata(new_path).is_ok() {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            format!("output file {new_path} already exists; refusing to overwrite"),
        ));
    }
    let value: serde_json::Value = serde_json::from_str(&fs::read_to_string(old_path)?)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    let hex = value
        .get("hex_digits")
        .and_then(|v| v.as_str())
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "missing v1 hex_digits field"))?;
    let invalid = |message: String| io::Error::new(io::ErrorKind::InvalidData, message);
    let digits = hex
        .chars()
        .enumerate()
        .map(|(i, c)| {
            c.to_digit(16)
                .map(|d| d as u8)
                .ok_or_else(|| invalid(format!("invalid hex character at position {i}")))
        })
        .collect::<io::Result<Vec<u8>>>()?;
    let mut core = CoreState {
        record: Record::default(),
        converter: DecimalConverter::default(),
    };
    let mut index = 0;
    while index < digits.len() {
        let wanted = (digits.len() - index).min(MAX_BATCH_DIGITS);
        let certified =
            certified_hex_digits(index as u64, wanted, cancel).map_err(|e| match e {
                MathError::Cancelled { .. } => {
                    io::Error::new(io::ErrorKind::Interrupted, "re-verification cancelled")
                }
                e => invalid(format!("re-verification error at position {index}: {e}")),
            })?;
        for (expected, actual) in certified.into_iter().zip(&digits[index..]) {
            if expected != *actual {
                return Err(invalid(format!("digit mismatch at position {index}")));
            }
            core.commit_digit(expected, 0.0).map_err(invalid)?;
            index += 1;
        }
    }
    save_record_snapshot(core.snapshot_record(), new_path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::load_record;
    use tempfile::{tempdir, TempDir};

    const PI_HEX: &str = "243F6A8885A308D313198A2E03707344A4093822299F31D0082EFA98EC4E6C89";

    fn never() -> bool {
        false
    }

    fn check(hex: &str, coverage: Coverage) -> Result<VerifyReport, VerifyError> {
        verify_with_seed(hex, coverage, 7, &never, &mut |_, _| {})
    }

    fn temp_path(name: &str) -> (TempDir, String) {
        let dir = tempdir().unwrap();
        let path = dir.path().join(name).to_str().unwrap().to_string();
        (dir, path)
    }

    #[test]
    fn windows_are_ordered_disjoint_and_include_the_tail() {
        for seed in 0..50 {
            let windows = sample_windows(1_000, 40, seed);
            assert!(windows.windows(2).all(|w| w[0].1 <= w[1].0));
            assert!(windows.iter().all(|w| w.0 < w.1 && w.1 <= 1_000));
            assert_eq!(windows.last().unwrap().1, 1_000);
        }
    }

    #[test]
    fn short_and_empty_records_have_valid_windows() {
        assert!(sample_windows(0, 5, 1).is_empty());
        assert!(sample_windows(3, 5, 1).iter().all(|w| w.1 <= 3));
    }

    #[test]
    fn correct_digits_pass_full_verification() {
        let report = check(PI_HEX, Coverage::Full).unwrap();
        assert_eq!(report.checked, PI_HEX.len());
    }

    #[test]
    fn empty_record_passes() {
        assert_eq!(check("", Coverage::Full).unwrap().checked, 0);
        spot_check("").unwrap();
    }

    #[test]
    fn full_verification_finds_the_exact_wrong_digit() {
        let wrong = format!("{}0{}", &PI_HEX[..30], &PI_HEX[31..]);
        let found = u8::from_str_radix(&PI_HEX[30..31], 16).unwrap();
        assert_ne!(found, 0, "test must actually mutate a digit");
        match check(&wrong, Coverage::Full).unwrap_err() {
            VerifyError::Mismatch { index, found, .. } => assert_eq!((index, found), (30, 0)),
            other => panic!("unexpected error: {other}"),
        }
    }

    #[test]
    fn spot_check_always_catches_a_wrong_newest_digit() {
        let wrong = format!("{}0", &PI_HEX[..PI_HEX.len() - 1]);
        assert!(matches!(
            spot_check(&wrong),
            Err(VerifyError::Mismatch { .. })
        ));
    }

    #[test]
    fn cancellation_is_reported() {
        let result = verify_with_seed(PI_HEX, Coverage::Full, 1, &|| true, &mut |_, _| {});
        assert!(matches!(result, Err(VerifyError::Cancelled)));
    }

    #[test]
    fn reverify_cancellation_writes_nothing() {
        let (dir, new) = temp_path("cancelled.json");
        let old = dir.path().join("old.json");
        fs::write(&old, r#"{"hex_digits":"243"}"#).unwrap();
        let error = reverify_v1(old.to_str().unwrap(), &new, &|| true).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::Interrupted);
        assert!(!Path::new(&new).exists());
    }

    #[test]
    fn reverify_rejects_same_input_and_output() {
        let (_dir, path) = temp_path("reverify-same.json");
        fs::write(&path, "{}").unwrap();
        assert_eq!(
            reverify_v1(&path, &path, &never).unwrap_err().kind(),
            io::ErrorKind::InvalidInput
        );
    }

    #[test]
    fn reverify_refuses_locked_output() {
        let (dir, new) = temp_path("reverify-locked.json");
        let old = dir.path().join("old.json");
        fs::write(&old, r#"{"hex_digits":""}"#).unwrap();
        let lock = OutputLock::acquire(&new).unwrap();
        assert_eq!(
            reverify_v1(old.to_str().unwrap(), &new, &never)
                .unwrap_err()
                .kind(),
            io::ErrorKind::WouldBlock
        );
        drop(lock);
    }

    #[test]
    fn reverify_never_overwrites_existing_output() {
        let (dir, new) = temp_path("existing.json");
        let old = dir.path().join("old.json");
        fs::write(&old, r#"{"hex_digits":"243"}"#).unwrap();
        fs::write(&new, "precious").unwrap();
        assert_eq!(
            reverify_v1(old.to_str().unwrap(), &new, &never)
                .unwrap_err()
                .kind(),
            io::ErrorKind::AlreadyExists
        );
        assert_eq!(fs::read_to_string(&new).unwrap(), "precious");
    }

    #[test]
    fn reverify_reports_first_wrong_digit() {
        let (dir, new) = temp_path("wrong.json");
        let old = dir.path().join("old.json");
        fs::write(&old, r#"{"hex_digits":"243F6A80"}"#).unwrap();
        let error = reverify_v1(old.to_str().unwrap(), &new, &never).unwrap_err();
        assert!(
            error.to_string().contains("digit mismatch at position 7"),
            "{error}"
        );
        assert!(!Path::new(&new).exists());
    }

    #[test]
    fn reverify_writes_new_output() {
        let (dir, new) = temp_path("fresh.json");
        let old = dir.path().join("old.json");
        fs::write(&old, r#"{"hex_digits":"243f"}"#).unwrap();
        reverify_v1(old.to_str().unwrap(), &new, &never).unwrap();
        assert_eq!(load_record(&new).unwrap().0.hex_digits, "243F");
    }
}
