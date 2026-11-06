//! Remove partial files when qdu is interrupted.
//!
//! qdu blocks SIGINT, SIGTERM, and SIGHUP in every thread and waits for them
//! on a dedicated thread. That thread removes the registered temporary files,
//! such as a half-written snapshot database, and exits with 128 plus the
//! signal number. Child processes start with the default signal mask because
//! `std::process::Command` resets it.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use crate::sys;

static PENDING: Mutex<Vec<PathBuf>> = Mutex::new(Vec::new());

fn pending() -> std::sync::MutexGuard<'static, Vec<PathBuf>> {
    PENDING
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Start the signal thread; call before any other thread starts.
pub fn install() {
    // SAFETY: the set is initialized by sigemptyset before use, and the
    // mask only changes which signals this process receives asynchronously.
    unsafe {
        let mut set: libc::sigset_t = std::mem::zeroed();
        libc::sigemptyset(&mut set);
        for signal in [libc::SIGINT, libc::SIGTERM, libc::SIGHUP] {
            libc::sigaddset(&mut set, signal);
        }
        if libc::pthread_sigmask(libc::SIG_BLOCK, &set, std::ptr::null_mut()) != 0 {
            return;
        }
        let waited = set;
        let spawned = std::thread::Builder::new()
            .name("qdu-signals".into())
            .spawn(move || {
                let mut signal = 0;
                if libc::sigwait(&waited, &mut signal) == 0 {
                    interrupted(signal);
                }
            });
        if spawned.is_err() {
            // Without the thread, restore the default behaviour.
            libc::pthread_sigmask(libc::SIG_UNBLOCK, &set, std::ptr::null_mut());
        }
    }
}

fn interrupted(signal: i32) -> ! {
    // Keep the list locked until the process exits, so that no thread
    // creates a file after its entry was handled here.
    let pending = pending();
    for path in pending.iter() {
        let _ = std::fs::remove_file(path);
    }
    let mut stderr = std::io::stderr();
    if sys::is_terminal(libc::STDERR_FILENO) {
        // Erase a progress line that may be showing.
        let _ = write!(stderr, "\r\x1b[K");
    }
    let _ = writeln!(stderr, "qdu: interrupted");
    std::process::exit(128 + signal);
}

/// A file removed when the guard drops or qdu is interrupted.
pub struct Cleanup {
    path: PathBuf,
}

impl Cleanup {
    /// Create a file with `create` and register it in one step, so that an
    /// interruption either comes before the file exists or removes it.
    pub fn create<T, E>(
        path: impl Into<PathBuf>,
        create: impl FnOnce(&Path) -> Result<T, E>,
    ) -> Result<(Self, T), E> {
        let path = path.into();
        let mut pending = pending();
        let created = create(&path)?;
        pending.push(path.clone());
        Ok((Cleanup { path }, created))
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Keep the file; call after it has been renamed into place.
    pub fn keep(self) {
        forget(&self.path);
        std::mem::forget(self);
    }
}

impl Drop for Cleanup {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
        forget(&self.path);
    }
}

fn forget(path: &Path) {
    let mut pending = pending();
    if let Some(index) = pending.iter().rposition(|item| item == path) {
        pending.swap_remove(index);
    }
}
