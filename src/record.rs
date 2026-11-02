//! Persistent JSON record schema and in-memory core computation state.
use crate::converter::DecimalConverter;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::io;

pub const SCHEMA_VERSION: u32 = 2;
pub const ALGORITHM_ID: &str = "bbp-mpfr-directed-v1";

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Record {
    pub schema_version: u32,
    pub algorithm: String,
    pub next_hex_index: u64,
    pub certified_hex_count: u64,
    pub total_elapsed_s: f64,
    pub min_elapsed_s: f64,
    pub max_elapsed_s: f64,
    pub hex_digits: String,
    pub decimal_digits: String,
    pub digit_times: Vec<f64>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub decimal_y_hex: String,
    pub hex_sha256: String,
}

impl Default for Record {
    fn default() -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            algorithm: ALGORITHM_ID.into(),
            next_hex_index: 0,
            certified_hex_count: 0,
            total_elapsed_s: 0.0,
            min_elapsed_s: 0.0,
            max_elapsed_s: 0.0,
            hex_digits: String::new(),
            decimal_digits: String::new(),
            digit_times: Vec::new(),
            decimal_y_hex: String::new(),
            hex_sha256: sha256_hex(""),
        }
    }
}

pub struct CoreState {
    pub record: Record,
    pub converter: DecimalConverter,
}
#[derive(Clone)]
pub struct UiSnapshot {
    pub hex_count: u64,
    pub decimal_count: u64,
    pub total_elapsed_s: f64,
    pub fastest_s: f64,
    pub slowest_s: f64,
    pub hex_display: String,
    pub decimal_display: String,
    pub digit_times: Vec<f64>,
}

impl CoreState {
    pub fn ui_snapshot(&self) -> UiSnapshot {
        self.ui_snapshot_with_limits(128, 10_000)
    }
    pub fn ui_snapshot_with_limits(&self, hex_limit: usize, decimal_limit: usize) -> UiSnapshot {
        let suffix = |s: &str, n: usize| s.get(s.len().saturating_sub(n)..).unwrap_or(s).to_owned();
        UiSnapshot {
            hex_count: self.record.hex_digits.len() as u64,
            decimal_count: self.record.decimal_digits.len() as u64,
            total_elapsed_s: self.record.total_elapsed_s,
            fastest_s: self.record.min_elapsed_s,
            slowest_s: self.record.max_elapsed_s,
            hex_display: suffix(&self.record.hex_digits, hex_limit),
            decimal_display: suffix(&self.record.decimal_digits, decimal_limit),
            digit_times: self.record.digit_times.clone(),
        }
    }
    pub fn snapshot_record(&self) -> Record {
        let mut r = self.record.clone();
        r.decimal_y_hex.clear();
        r
    }
    pub fn add_timing(&mut self, elapsed: f64) {
        self.add_batch_timing(1, elapsed);
    }
    pub fn add_batch_timing(&mut self, count: usize, per_digit_elapsed: f64) {
        if count == 0 {
            return;
        }
        let had_timing = !self.record.digit_times.is_empty();
        self.record.total_elapsed_s += per_digit_elapsed * (count as f64);
        self.record.min_elapsed_s = if !had_timing {
            per_digit_elapsed
        } else {
            self.record.min_elapsed_s.min(per_digit_elapsed)
        };
        self.record.max_elapsed_s = self.record.max_elapsed_s.max(per_digit_elapsed);
        self.record.digit_times.push(per_digit_elapsed);
        if self.record.digit_times.len() > 10 {
            self.record.digit_times.remove(0);
        }
    }
    /// Appends a certified hexadecimal digit and returns any decimal digits finalized at this step.
    pub fn commit_digit(&mut self, digit: u8, elapsed: f64) -> Result<String, String> {
        let mut out = self.commit_batch(&[digit], elapsed)?;
        Ok(out.pop().unwrap_or_default())
    }
    /// Appends a batch of certified hexadecimal digits, recording batch timing once while
    /// returning the finalized decimal digits produced at each hexadecimal step.
    pub fn commit_batch(
        &mut self,
        digits: &[u8],
        per_digit_elapsed: f64,
    ) -> Result<Vec<String>, String> {
        let mut decimals = Vec::with_capacity(digits.len());
        for &digit in digits {
            let hex = char::from_digit(u32::from(digit), 16)
                .ok_or_else(|| format!("hex digit out of range: {digit}"))?
                .to_ascii_uppercase();
            let decimal: String = self
                .converter
                .add_hex(digit)?
                .into_iter()
                .filter_map(|d| char::from_digit(u32::from(d), 10))
                .collect();
            self.record.hex_digits.push(hex);
            self.record.decimal_digits.push_str(&decimal);
            self.record.certified_hex_count += 1;
            self.record.next_hex_index = self.record.certified_hex_count;
            decimals.push(decimal);
        }
        self.add_batch_timing(digits.len(), per_digit_elapsed);
        Ok(decimals)
    }
}

pub fn sha256_hex(s: &str) -> String {
    let mut h = Sha256::new();
    h.update(s.as_bytes());
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

pub fn validate_record(r: &Record) -> io::Result<()> {
    if r.schema_version != SCHEMA_VERSION || r.algorithm != ALGORITHM_ID {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "unsupported record schema or algorithm",
        ));
    }

    if r.next_hex_index != r.certified_hex_count
        || r.certified_hex_count != r.hex_digits.len() as u64
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "record digit count invariant violated",
        ));
    }
    if r.certified_hex_count == 0
        && (r.min_elapsed_s != 0.0 || r.max_elapsed_s != 0.0 || r.total_elapsed_s != 0.0)
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "empty record timing invariant violated",
        ));
    }
    if r.certified_hex_count > 0
        && (r.min_elapsed_s > r.max_elapsed_s
            || r.max_elapsed_s > r.total_elapsed_s
            || r.digit_times.is_empty())
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "timing bounds invariant violated",
        ));
    }
    if r.digit_times.len() > 10 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "digit_times history may contain at most 10 entries",
        ));
    }
    if !r.hex_digits.chars().all(|c| c.is_ascii_hexdigit())
        || !r.decimal_digits.chars().all(|c| c.is_ascii_digit())
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "record contains invalid digit characters",
        ));
    }
    if r.hex_sha256 != sha256_hex(&r.hex_digits) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "hex SHA-256 checksum mismatch",
        ));
    }
    if !r.total_elapsed_s.is_finite()
        || r.total_elapsed_s < 0.0
        || !r.min_elapsed_s.is_finite()
        || r.min_elapsed_s < 0.0
        || !r.max_elapsed_s.is_finite()
        || r.max_elapsed_s < 0.0
        || r.digit_times.iter().any(|v| !v.is_finite() || *v < 0.0)
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "timing values must be finite and non-negative",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_empty_default() {
        validate_record(&Record::default()).unwrap();
    }
    #[test]
    fn rejects_corrupt_checksum() {
        let r = Record {
            hex_digits: "A".into(),
            ..Record::default()
        };
        assert!(validate_record(&r).is_err());
    }
    #[test]
    fn snapshot_limits_bound_copied_suffixes() {
        let core = CoreState {
            record: Record {
                hex_digits: "0123456789ABCDEF".into(),
                decimal_digits: "1415926535".into(),
                ..Record::default()
            },
            converter: DecimalConverter::default(),
        };
        let snap = core.ui_snapshot_with_limits(4, 0);
        assert_eq!(snap.hex_count, 16);
        assert_eq!(snap.decimal_count, 10);
        assert_eq!(snap.hex_display, "CDEF");
        assert!(snap.decimal_display.is_empty());
    }
    #[test]
    fn commit_batch_records_one_timing_sample_per_batch() {
        let mut core = CoreState {
            record: Record::default(),
            converter: DecimalConverter::default(),
        };
        core.commit_batch(&[2, 4, 3, 15], 0.05).unwrap();
        assert_eq!(core.record.certified_hex_count, 4);
        assert_eq!(core.record.digit_times, vec![0.05]);
        assert!((core.record.total_elapsed_s - 0.20).abs() < 1e-12);
        let mut snap = core.snapshot_record();
        snap.hex_sha256 = sha256_hex(&snap.hex_digits);
        validate_record(&snap).unwrap();
    }
}
