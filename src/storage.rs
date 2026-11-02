//! Atomic writing and validated loading of record files.
use crate::converter::DecimalConverter;
use crate::record::{sha256_hex, validate_record, Record, ALGORITHM_ID, SCHEMA_VERSION};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::Path;

pub fn save_record_snapshot(mut r: Record, path: &str) -> io::Result<()> {
    r.schema_version = SCHEMA_VERSION;
    r.algorithm = ALGORITHM_ID.into();
    r.certified_hex_count = r.hex_digits.len() as u64;
    r.next_hex_index = r.certified_hex_count;
    r.hex_sha256 = sha256_hex(&r.hex_digits);
    validate_record(&r)?;
    let json = serde_json::to_string_pretty(&r)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    let destination = Path::new(path);
    let temporary = destination.with_file_name(format!(
        ".{}.tmp-{}",
        destination
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("record"),
        std::process::id()
    ));
    let result = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
            .or_else(|e| {
                if e.kind() == io::ErrorKind::AlreadyExists {
                    fs::remove_file(&temporary)?;
                    OpenOptions::new()
                        .write(true)
                        .create_new(true)
                        .open(&temporary)
                } else {
                    Err(e)
                }
            })?;
        file.write_all(json.as_bytes())?;
        file.flush()?;
        file.sync_all()?;
        fs::rename(&temporary, destination)?;
        // A bare filename without a directory component has an empty parent, so fall back to ".".
        let parent = destination
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        File::open(parent)?.sync_all()?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }

    result
}

pub fn load_record(path: &str) -> io::Result<(Record, DecimalConverter)> {
    let content = match fs::read_to_string(path) {
        Ok(v) => v,
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            return Ok((Record::default(), DecimalConverter::default()))
        }
        Err(e) => return Err(e),
    };
    let value: serde_json::Value = serde_json::from_str(&content)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    if value.get("schema_version").is_none() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "v1 record is not certified; verification/migration with --reverify is required",
        ));
    }
    let record: Record =
        serde_json::from_value(value).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    validate_record(&record)?;
    let converter = DecimalConverter::from_digits(
        &record.hex_digits,
        &record.decimal_digits,
        &record.decimal_y_hex,
    )
    .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    Ok((record, converter))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{lock::OutputLock, record::CoreState};
    use tempfile::{tempdir, TempDir};

    fn temp_path(name: &str) -> (TempDir, String) {
        let dir = tempdir().unwrap();
        let path = dir.path().join(name).to_str().unwrap().to_string();
        (dir, path)
    }

    #[test]
    fn output_with_lock_extension_keeps_its_record() {
        let (_dir, path) = temp_path("record.lock");
        let _lock = OutputLock::acquire(&path).unwrap();
        save_record_snapshot(Record::default(), &path).unwrap();
        load_record(&path).unwrap();
    }

    #[test]
    fn v2_roundtrip_validates_converter() {
        let (_dir, path) = temp_path("v2-roundtrip.json");
        let mut core = CoreState {
            record: Record::default(),
            converter: DecimalConverter::default(),
        };
        core.commit_batch(&[2, 4, 3, 15, 6, 10], 0.001).unwrap();
        let expected_y = core.converter.y.clone();
        save_record_snapshot(core.snapshot_record(), &path).unwrap();
        let raw_json = fs::read_to_string(&path).unwrap();
        assert!(!raw_json.contains("decimal_y_hex"));
        let (loaded_record, loaded_converter) = load_record(&path).unwrap();
        assert_eq!(loaded_record.hex_digits, "243F6A");
        assert_eq!(loaded_converter.y, expected_y);

        // Also verify backwards-compatibility with v2 records that explicitly store decimal_y_hex.
        let mut with_explicit_y = loaded_record;
        with_explicit_y.decimal_y_hex = loaded_converter.y_hex_str();
        save_record_snapshot(with_explicit_y, &path).unwrap();
        let (_, explicit_converter) = load_record(&path).unwrap();
        assert_eq!(explicit_converter.y, expected_y);
    }
}
