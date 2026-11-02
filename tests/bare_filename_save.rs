//! Runs in its own integration test binary because it changes the process working directory.
use pi_hesaplayici::{
    lock::OutputLock,
    record::Record,
    storage::{load_record, save_record_snapshot},
};

#[test]
fn bare_filename_saves_and_reloads() {
    let dir = tempfile::tempdir().unwrap();
    std::env::set_current_dir(dir.path()).unwrap();

    let lock = OutputLock::acquire("pi_record.json").unwrap();
    save_record_snapshot(Record::default(), "pi_record.json").unwrap();
    load_record("pi_record.json").unwrap();
    drop(lock);
}
