//! Advisory lock ensuring exclusive single-process ownership of an output file.
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use std::ffi::OsString;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum LockState {
    Running,
    Paused,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StaleLock {
    pub pid: u32,
    pub state: LockState,
    pub started_unix_ms: u128,
    pub output_path: String,
    pub executable: String,
    pub detected_unix_ms: u128,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LockMetadata {
    pub schema_version: u32,
    pub state: LockState,
    pub pid: u32,
    pub started_unix_ms: u128,
    pub output_path: String,
    pub executable: String,
    pub recovered_stale_lock: Option<StaleLock>,
}

#[derive(Debug)]
pub struct OutputLock {
    file: Option<File>,
    pub path: PathBuf,
    pub metadata: LockMetadata,
}

impl OutputLock {
    pub fn acquire(path: &str) -> io::Result<Self> {
        let lock_path = lock_path_for(path);
        let mut file = open_and_lock(&lock_path)?;

        let previous = read_lock_metadata(&mut file).ok();
        let now = unix_time_ms()?;
        let recovered_stale_lock = previous.map(|metadata| StaleLock {
            pid: metadata.pid,
            state: metadata.state,
            started_unix_ms: metadata.started_unix_ms,
            output_path: metadata.output_path,
            executable: metadata.executable,
            detected_unix_ms: now,
        });
        if let Some(stale) = &recovered_stale_lock {
            eprintln!(
                "Warning: recovered stale output lock left by pid {} (started_unix_ms {})",
                stale.pid, stale.started_unix_ms
            );
        }

        let output_path = absolute_path(Path::new(path))?;
        let executable = std::env::current_exe()
            .map(|value| value.display().to_string())
            .unwrap_or_else(|_| "<unknown>".to_string());
        let metadata = LockMetadata {
            schema_version: 1,
            state: LockState::Running,
            pid: std::process::id(),
            started_unix_ms: now,
            output_path: output_path.display().to_string(),
            executable,
            recovered_stale_lock,
        };
        write_lock_metadata(&mut file, &metadata)?;

        Ok(Self {
            file: Some(file),
            path: lock_path,
            metadata,
        })
    }
}

impl Drop for OutputLock {
    fn drop(&mut self) {
        // Remove the lockfile while still holding the flock so any concurrent opener
        // that opened the old inode fails the identity check in `open_and_lock` and retries.
        if let Err(error) = fs::remove_file(&self.path) {
            if error.kind() != io::ErrorKind::NotFound {
                eprintln!(
                    "Warning: failed to remove output lock file ({}): {error}",
                    self.path.display()
                );
            }
        }
        if let Some(file) = self.file.take() {
            if let Err(error) = FileExt::unlock(&file) {
                eprintln!(
                    "Warning: failed to unlock output lock file ({}): {error}",
                    self.path.display()
                );
            }
        }
    }
}

/// The lock path is formed by appending `.lock` to the output path so it never
/// collides with the output file itself and `a.json` and `a.txt` do not share a lock.
fn lock_path_for(output_path: &str) -> PathBuf {
    let mut lock_path = OsString::from(output_path);
    lock_path.push(".lock");
    PathBuf::from(lock_path)
}

fn open_and_lock(lock_path: &Path) -> io::Result<File> {
    loop {
        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(lock_path)?;
        if let Err(error) = file.try_lock_exclusive() {
            let owner = read_lock_metadata(&mut file)
                .map(|metadata| {
                    format!(
                        "pid {}, started_unix_ms {}, executable {}",
                        metadata.pid, metadata.started_unix_ms, metadata.executable
                    )
                })
                .unwrap_or_else(|_| "owner metadata unreadable".to_string());
            return Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                format!("output file is locked by another process ({owner}): {error}"),
            ));
        }
        // The previous owner may have unlinked the file between our open and lock calls.
        if lock_file_is_current(&file, lock_path)? {
            return Ok(file);
        }
    }
}

#[cfg(unix)]
fn lock_file_is_current(file: &File, lock_path: &Path) -> io::Result<bool> {
    use std::os::unix::fs::MetadataExt;
    let on_disk = match fs::metadata(lock_path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error),
    };
    let held = file.metadata()?;
    Ok(on_disk.dev() == held.dev() && on_disk.ino() == held.ino())
}

#[cfg(not(unix))]
fn lock_file_is_current(_file: &File, _lock_path: &Path) -> io::Result<bool> {
    Ok(true)
}

pub fn update_lock_state(output_path: &str, state: LockState) -> io::Result<()> {
    let lock_path = lock_path_for(output_path);
    let mut file = OpenOptions::new().read(true).write(true).open(&lock_path)?;
    let mut metadata = read_lock_metadata(&mut file)?;
    if metadata.pid != std::process::id() {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!(
                "lock belongs to pid {}, current pid is {}",
                metadata.pid,
                std::process::id()
            ),
        ));
    }
    metadata.state = state;
    write_lock_metadata(&mut file, &metadata)
}

fn unix_time_ms() -> io::Result<u128> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .map_err(|error| io::Error::other(format!("system clock is before Unix epoch: {error}")))
}

fn absolute_path(path: &Path) -> io::Result<PathBuf> {
    if path.is_absolute() {
        Ok(path.to_path_buf())
    } else {
        Ok(std::env::current_dir()?.join(path))
    }
}

fn read_lock_metadata(file: &mut File) -> io::Result<LockMetadata> {
    file.seek(SeekFrom::Start(0))?;
    let mut content = String::new();
    file.read_to_string(&mut content)?;
    serde_json::from_str(&content)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
}

/// Rewrites lock metadata in place so the file inode never changes.
/// The file is never truncated: shorter content is padded with spaces.
fn write_lock_metadata(file: &mut File, metadata: &LockMetadata) -> io::Result<()> {
    let mut content = serde_json::to_vec_pretty(metadata)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    content.push(b'\n');
    let previous_len = usize::try_from(file.metadata()?.len()).unwrap_or(usize::MAX);
    if content.len() < previous_len {
        content.resize(previous_len, b' ');
    }
    file.seek(SeekFrom::Start(0))?;
    file.write_all(&content)?;
    file.flush()?;
    file.sync_all()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::{tempdir, TempDir};

    fn temp_path(name: &str) -> (TempDir, String) {
        let dir = tempdir().unwrap();
        let path = dir.path().join(name).to_str().unwrap().to_string();
        (dir, path)
    }

    #[test]
    fn double_lock_is_rejected() {
        let (_dir, path) = temp_path("double-lock.json");
        let first = OutputLock::acquire(&path).unwrap();
        let error = OutputLock::acquire(&path).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::WouldBlock);
        assert!(error.to_string().contains(&std::process::id().to_string()));
        drop(first);
    }

    #[test]
    fn lock_path_never_collides_with_output() {
        assert_eq!(lock_path_for("x.lock"), Path::new("x.lock.lock"));
        assert_ne!(lock_path_for("a.json"), lock_path_for("a.txt"));
        assert_eq!(lock_path_for("noext"), Path::new("noext.lock"));
    }

    #[cfg(unix)]
    #[test]
    fn unlinked_lock_file_is_not_current() {
        let (_dir, path) = temp_path("identity.json");
        let lock_path = lock_path_for(&path);
        let file = open_and_lock(&lock_path).unwrap();
        assert!(lock_file_is_current(&file, &lock_path).unwrap());

        fs::remove_file(&lock_path).unwrap();
        assert!(!lock_file_is_current(&file, &lock_path).unwrap());

        let replacement = open_and_lock(&lock_path).unwrap();
        assert!(!lock_file_is_current(&file, &lock_path).unwrap());
        assert!(lock_file_is_current(&replacement, &lock_path).unwrap());
    }

    #[test]
    fn lock_metadata_tracks_owner_and_clean_release() {
        let (_dir, path) = temp_path("lock-metadata.json");
        let lock_path = lock_path_for(&path);
        let lock = OutputLock::acquire(&path).unwrap();
        let active: LockMetadata =
            serde_json::from_str(&fs::read_to_string(&lock_path).unwrap()).unwrap();
        assert_eq!(active.state, LockState::Running);
        assert_eq!(active.pid, std::process::id());
        assert_eq!(active.output_path, path);
        drop(lock);
        assert!(!lock_path.exists());
    }

    #[test]
    fn stale_active_metadata_is_detected_after_lock_is_free() {
        let (_dir, path) = temp_path("stale-lock.json");
        let lock_path = lock_path_for(&path);
        let stale = LockMetadata {
            schema_version: 1,
            state: LockState::Running,
            pid: u32::MAX,
            started_unix_ms: 123,
            output_path: path.clone(),
            executable: "/stale/process".to_string(),
            recovered_stale_lock: None,
        };
        fs::write(&lock_path, serde_json::to_vec_pretty(&stale).unwrap()).unwrap();

        let lock = OutputLock::acquire(&path).unwrap();
        let recovered = lock.metadata.recovered_stale_lock.as_ref().unwrap();
        assert_eq!(recovered.pid, u32::MAX);
        assert_eq!(recovered.state, LockState::Running);
        assert_eq!(recovered.started_unix_ms, 123);
        drop(lock);
        assert!(!lock_path.exists());
    }

    #[test]
    fn lock_state_tracks_pause_and_resume() {
        let (_dir, path) = temp_path("lock-state.json");
        let lock_path = lock_path_for(&path);
        let lock = OutputLock::acquire(&path).unwrap();

        update_lock_state(&path, LockState::Paused).unwrap();
        let paused: LockMetadata =
            serde_json::from_str(&fs::read_to_string(&lock_path).unwrap()).unwrap();
        assert_eq!(paused.state, LockState::Paused);

        update_lock_state(&path, LockState::Running).unwrap();
        let running: LockMetadata =
            serde_json::from_str(&fs::read_to_string(&lock_path).unwrap()).unwrap();
        assert_eq!(running.state, LockState::Running);

        drop(lock);
        assert!(!lock_path.exists());
    }

    #[test]
    fn shorter_lock_metadata_replaces_longer_without_truncating() {
        let (_dir, path) = temp_path("rewrite.json");
        let lock_path = lock_path_for(&path);
        let lock = OutputLock::acquire(&path).unwrap();
        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&lock_path)
            .unwrap();

        let long_len = file.metadata().unwrap().len();
        let mut short = lock.metadata.clone();
        short.executable = "x".to_string();
        short.output_path = "y".to_string();
        write_lock_metadata(&mut file, &short).unwrap();

        assert_eq!(file.metadata().unwrap().len(), long_len);
        let reread = read_lock_metadata(&mut file).unwrap();
        assert_eq!(reread.executable, "x");
        assert_eq!(reread.output_path, "y");
    }
}
