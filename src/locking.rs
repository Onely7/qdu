//! Coordinate profile changes with advisory process locks.

use std::fs;
use std::io::{Read, Seek, SeekFrom, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::Path;
use std::time::{Duration, Instant};

use serde_json::{Map, Value, json};

use crate::error::{Error, PathContext, Result};
use crate::storage::create_private_directory;
use crate::sys;
use crate::units::format_local_time;

/// An exclusive lock on a profile, with metadata that describes its holder.
///
/// The kernel releases the lock when its process exits, even after a crash,
/// so a lock never outlives its holder; leftover metadata in the file has no
/// effect on the next holder.
pub struct ProfileLock {
    file: fs::File,
}

fn open_lock_file(path: &Path) -> Result<fs::File> {
    if let Some(parent) = path.parent() {
        create_private_directory(parent)?;
    }
    fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .open(path)
        .at(path)
}

fn try_lock(file: &fs::File) -> std::io::Result<bool> {
    // SAFETY: flock only reads the descriptor number, which `file` owns.
    let result = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    if result == 0 {
        return Ok(true);
    }
    let error = std::io::Error::last_os_error();
    if error.raw_os_error() == Some(libc::EWOULDBLOCK) {
        Ok(false)
    } else {
        Err(error)
    }
}

fn unlock(file: &fs::File) {
    // SAFETY: as in try_lock.
    unsafe {
        libc::flock(file.as_raw_fd(), libc::LOCK_UN);
    }
}

fn read_metadata(file: &mut fs::File) -> Option<Map<String, Value>> {
    let mut text = String::new();
    file.seek(SeekFrom::Start(0)).ok()?;
    file.read_to_string(&mut text).ok()?;
    match serde_json::from_str(text.trim()).ok()? {
        Value::Object(object) => Some(object),
        _ => None,
    }
}

/// Whether `file` is still the file at `path`, rather than one that was
/// removed, perhaps with the profile's data, and replaced.
fn still_at(file: &fs::File, path: &Path) -> bool {
    match (file.metadata(), fs::symlink_metadata(path)) {
        (Ok(open), Ok(current)) => (open.dev(), open.ino()) == (current.dev(), current.ino()),
        _ => false,
    }
}

impl ProfileLock {
    /// Take the profile lock or report the operation that holds it.
    pub fn acquire(path: &Path, command: &str) -> Result<Self> {
        let mut attempts = 0;
        let mut file = loop {
            let mut file = open_lock_file(path)?;
            // `qdu doctor` holds the lock for a moment to see whether it is
            // free, so retry briefly before reporting the profile as busy.
            let deadline = Instant::now() + Duration::from_secs(2);
            let mut locked = try_lock(&file)?;
            while !locked && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(50));
                locked = try_lock(&file)?;
            }
            if !locked {
                let mut message = "another qdu operation is using this profile".to_string();
                if let Some(metadata) = read_metadata(&mut file) {
                    message.push_str(": ");
                    message.push_str(&describe_holder(&metadata));
                }
                return Err(Error::Busy(message));
            }
            // A lock on a file that has since been removed excludes nobody
            // who opens the path now, so lock the file that is there.
            attempts += 1;
            if still_at(&file, path) || attempts == 10 {
                break file;
            }
            unlock(&file);
        };
        let metadata = json!({
            "pid": std::process::id(),
            "host": sys::hostname(),
            "started_epoch": sys::now_epoch(),
            "command": command,
            "argv": std::env::args().collect::<Vec<_>>(),
        });
        file.set_len(0)?;
        file.seek(SeekFrom::Start(0))?;
        writeln!(file, "{metadata}")?;
        file.sync_all()?;
        Ok(ProfileLock { file })
    }
}

impl Drop for ProfileLock {
    fn drop(&mut self) {
        let _ = self.file.set_len(0);
        unlock(&self.file);
    }
}

/// Whether a lock is held, and the metadata its holder published. A missing
/// lock file is not created.
pub fn inspect_lock(path: &Path) -> (bool, Option<Map<String, Value>>) {
    if !path.exists() {
        return (false, None);
    }
    let Ok(mut file) = open_lock_file(path) else {
        return (false, None);
    };
    let active = match try_lock(&file) {
        Ok(true) => {
            unlock(&file);
            false
        }
        Ok(false) => true,
        Err(_) => false,
    };
    (active, read_metadata(&mut file))
}

/// Describe the process that holds a lock from its published metadata.
pub fn describe_holder(metadata: &Map<String, Value>) -> String {
    let text = |key: &str, default: &str| -> String {
        match metadata.get(key) {
            Some(Value::String(value)) => value.clone(),
            Some(Value::Number(value)) => value.to_string(),
            _ => default.to_string(),
        }
    };
    let since = metadata
        .get("started_epoch")
        .and_then(Value::as_i64)
        .map(|epoch| format!(" since {}", format_local_time(epoch as f64)))
        .unwrap_or_default();
    format!(
        "{} (pid {} on {}{since})",
        text("command", "qdu"),
        text("pid", "?"),
        text("host", "?")
    )
}
