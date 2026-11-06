//! Run the qdu binary against private state, configuration, and data trees.

#![allow(dead_code)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::io::{AsRawFd, FromRawFd};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};

use serde_json::Value;

static COUNTER: AtomicUsize = AtomicUsize::new(0);

/// The outcome of one qdu run.
#[derive(Debug)]
pub struct Run {
    pub status: i32,
    pub stdout: String,
    pub stderr: String,
}

impl Run {
    /// Assert a zero exit status and return the run.
    #[track_caller]
    pub fn ok(self) -> Self {
        assert_eq!(
            self.status, 0,
            "stdout:\n{}\nstderr:\n{}",
            self.stdout, self.stderr
        );
        self
    }

    /// Assert an exit status and return the run.
    #[track_caller]
    pub fn status(self, expected: i32) -> Self {
        assert_eq!(
            self.status, expected,
            "stdout:\n{}\nstderr:\n{}",
            self.stdout, self.stderr
        );
        self
    }

    #[track_caller]
    pub fn json(&self) -> Value {
        serde_json::from_str(&self.stdout)
            .unwrap_or_else(|error| panic!("invalid JSON ({error}):\n{}", self.stdout))
    }
}

/// A private HOME, XDG directories, and a scan root, removed on drop.
pub struct Qdu {
    pub base: PathBuf,
    pub home: PathBuf,
    pub state: PathBuf,
    pub config: PathBuf,
    pub root: PathBuf,
    environment: Vec<(String, String)>,
}

impl Qdu {
    pub fn new() -> Self {
        let name = format!(
            "qdu-test-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        );
        let base = std::env::temp_dir().join(name);
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(&base).unwrap();
        let base = base.canonicalize().unwrap();
        let qdu = Qdu {
            home: base.join("home"),
            state: base.join("state"),
            config: base.join("config"),
            root: base.join("root"),
            base,
            environment: Vec::new(),
        };
        for path in [&qdu.home, &qdu.state, &qdu.config, &qdu.root] {
            fs::create_dir_all(path).unwrap();
        }
        qdu
    }

    /// Set an extra environment variable for later runs.
    pub fn env(&mut self, name: &str, value: impl Into<String>) {
        self.environment.push((name.to_string(), value.into()));
    }

    pub fn command(&self, arguments: &[&str]) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_qdu"));
        command
            .args(arguments)
            .env("HOME", &self.home)
            .env("XDG_STATE_HOME", &self.state)
            .env("XDG_CONFIG_HOME", &self.config)
            .env("QDU_PROFILE", "default")
            .env("COLUMNS", "200")
            .env_remove("QDU_THREADS")
            .env_remove("NO_COLOR")
            .stdin(Stdio::null());
        for (name, value) in &self.environment {
            command.env(name, value);
        }
        command
    }

    pub fn run(&self, arguments: &[&str]) -> Run {
        let output = self.command(arguments).output().expect("run qdu");
        Run {
            status: output.status.code().unwrap_or(-1),
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        }
    }

    /// Run with `--format json` and parse the successful output.
    #[track_caller]
    pub fn json(&self, arguments: &[&str]) -> Value {
        let mut arguments = arguments.to_vec();
        arguments.extend(["--format", "json"]);
        self.run(&arguments).ok().json()
    }

    /// Take a quiet snapshot of the scan root.
    pub fn snapshot(&self, extra: &[&str]) -> Run {
        let root = self.root.to_str().unwrap().to_string();
        let mut arguments = vec!["snapshot", "--path", root.as_str(), "--quiet"];
        arguments.extend(extra);
        self.run(&arguments)
    }

    pub fn write_file(&self, relative: &str, size: usize) -> PathBuf {
        let path = self.root.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, vec![b'x'; size]).unwrap();
        path
    }

    pub fn profile_dir(&self, profile: &str) -> PathBuf {
        self.state.join("qdu").join("profiles").join(profile)
    }

    pub fn snapshots_dir(&self) -> PathBuf {
        self.profile_dir("default").join("snapshots")
    }

    pub fn index_path(&self) -> PathBuf {
        self.profile_dir("default").join("index.json")
    }

    pub fn index(&self) -> Value {
        serde_json::from_str(&fs::read_to_string(self.index_path()).unwrap()).unwrap()
    }

    pub fn write_index(&self, index: &Value) {
        fs::write(
            self.index_path(),
            serde_json::to_string_pretty(index).unwrap(),
        )
        .unwrap();
    }

    /// Snapshot records from the index, oldest first.
    pub fn records(&self) -> Vec<Value> {
        self.index()["snapshots"]
            .as_array()
            .cloned()
            .unwrap_or_default()
    }

    pub fn snapshot_ids(&self) -> Vec<String> {
        self.records()
            .iter()
            .map(|record| record["snapshot_id"].as_str().unwrap().to_string())
            .collect()
    }

    /// The database file of the newest snapshot.
    pub fn latest_database(&self) -> PathBuf {
        let records = self.records();
        let record = records.last().expect("a snapshot");
        self.snapshots_dir()
            .join(record["filename"].as_str().unwrap())
    }

    pub fn config_file(&self) -> PathBuf {
        self.config.join("qdu").join("config.ini")
    }

    /// Install an executable shell script under `<base>/bin`.
    pub fn script(&self, name: &str, body: &str) -> PathBuf {
        let directory = self.base.join("bin");
        fs::create_dir_all(&directory).unwrap();
        let path = directory.join(name);
        fs::write(&path, format!("#!/bin/sh\n{body}")).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    /// A PATH that finds the scripts first.
    pub fn path_with_scripts(&self) -> String {
        format!(
            "{}:{}",
            self.base.join("bin").display(),
            std::env::var("PATH").unwrap_or_default()
        )
    }
}

impl Drop for Qdu {
    fn drop(&mut self) {
        restore_permissions(&self.base);
        let _ = fs::remove_dir_all(&self.base);
    }
}

fn restore_permissions(path: &Path) {
    let Ok(metadata) = fs::symlink_metadata(path) else {
        return;
    };
    if metadata.is_dir() {
        let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o700));
        if let Ok(entries) = fs::read_dir(path) {
            for entry in entries.flatten() {
                restore_permissions(&entry.path());
            }
        }
    }
}

/// Whether permission bits are enforced; root reads unreadable directories.
pub fn permissions_enforced() -> bool {
    // SAFETY: geteuid has no preconditions.
    unsafe { libc::geteuid() != 0 }
}

/// A directory the scanner cannot list, or `None` when running as root.
pub fn unreadable_directory(qdu: &Qdu, relative: &str) -> Option<PathBuf> {
    if !permissions_enforced() {
        return None;
    }
    let path = qdu.root.join(relative);
    fs::create_dir_all(&path).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o000)).unwrap();
    Some(path)
}

fn c_name(name: &str) -> std::ffi::CString {
    std::ffi::CString::new(name).unwrap()
}

/// Make a subdirectory of an open directory and open it. Working through
/// descriptors builds trees whose paths are longer than one system call
/// accepts.
pub fn make_directory_at(parent: &fs::File, name: &str) -> fs::File {
    let name = c_name(name);
    // SAFETY: the descriptor is open and the name is NUL-terminated.
    let made = unsafe { libc::mkdirat(parent.as_raw_fd(), name.as_ptr(), 0o755) };
    assert_eq!(made, 0, "mkdirat: {}", std::io::Error::last_os_error());
    // SAFETY: as above; the new descriptor is checked before it is owned.
    let descriptor = unsafe {
        libc::openat(
            parent.as_raw_fd(),
            name.as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC,
        )
    };
    assert!(
        descriptor >= 0,
        "openat: {}",
        std::io::Error::last_os_error()
    );
    // SAFETY: openat returned a new descriptor that nothing else owns.
    unsafe { fs::File::from_raw_fd(descriptor) }
}

/// Write a file of `size` bytes into an open directory.
pub fn write_file_at(directory: &fs::File, name: &str, size: usize) {
    use std::io::Write;
    let name = c_name(name);
    // SAFETY: the descriptor is open and the name is NUL-terminated; the
    // new descriptor is checked before it is owned.
    let descriptor = unsafe {
        libc::openat(
            directory.as_raw_fd(),
            name.as_ptr(),
            libc::O_WRONLY | libc::O_CREAT | libc::O_TRUNC | libc::O_CLOEXEC,
            0o644 as libc::c_uint,
        )
    };
    assert!(
        descriptor >= 0,
        "openat: {}",
        std::io::Error::last_os_error()
    );
    // SAFETY: openat returned a new descriptor that nothing else owns.
    let mut file = unsafe { fs::File::from_raw_fd(descriptor) };
    file.write_all(&vec![b'x'; size]).unwrap();
}

/// The rows of one type in the JSON of `qdu users`: `user` or `directory`.
pub fn rows_of_type<'a>(payload: &'a Value, kind: &str) -> Vec<&'a Value> {
    payload["rows"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|row| row["type"] == kind)
        .collect()
}

/// The `path` fields of a JSON array.
pub fn paths(items: &Value) -> Vec<String> {
    items
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["path"].as_str().unwrap().to_string())
        .collect()
}

/// Open a pseudo-terminal and return its controller and device sides.
pub fn pseudo_terminal() -> Option<(fs::File, fs::File)> {
    let mut controller = 0;
    let mut device = 0;
    // SAFETY: openpty writes two descriptors that this function then owns.
    let result = unsafe {
        libc::openpty(
            &mut controller,
            &mut device,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    };
    if result != 0 {
        return None;
    }
    for descriptor in [controller, device] {
        // Tests run on several threads: without this, a child that another
        // test starts meanwhile inherits the device and keeps it open.
        // SAFETY: fcntl only changes the flags of a descriptor we own.
        unsafe { libc::fcntl(descriptor, libc::F_SETFD, libc::FD_CLOEXEC) };
    }
    // SAFETY: both descriptors are open and owned by nothing else.
    Some(unsafe {
        (
            fs::File::from_raw_fd(controller),
            fs::File::from_raw_fd(device),
        )
    })
}

/// Which standard stream of a command goes to a pseudo-terminal.
pub enum Terminal {
    Stdout,
    Stderr,
}

/// Run a command with one standard stream on a pseudo-terminal.
///
/// Returns the exit status, what the command wrote to the terminal, and what
/// it wrote to the other stream, or `None` without pseudo-terminals. The
/// terminal is read while the command runs: on macOS a writer stops once a
/// little output is waiting, and a read never reports the end of the output.
pub fn run_on_terminal(
    mut command: std::process::Command,
    terminal: Terminal,
) -> Option<(std::process::ExitStatus, String, String)> {
    use std::io::Read;
    use std::process::Stdio;
    let (mut controller, device) = pseudo_terminal()?;
    match terminal {
        Terminal::Stdout => command.stdout(Stdio::from(device)).stderr(Stdio::piped()),
        Terminal::Stderr => command.stderr(Stdio::from(device)).stdout(Stdio::piped()),
    };
    // SAFETY: fcntl only changes the flags of a descriptor we own.
    unsafe {
        let flags = libc::fcntl(controller.as_raw_fd(), libc::F_GETFL);
        libc::fcntl(
            controller.as_raw_fd(),
            libc::F_SETFL,
            flags | libc::O_NONBLOCK,
        );
    }
    let mut child = command.spawn().expect("start qdu");
    // Close this process's copy of the device side.
    drop(command);
    let mut other: Box<dyn Read + Send> = match terminal {
        Terminal::Stdout => Box::new(child.stderr.take().expect("piped stderr")),
        Terminal::Stderr => Box::new(child.stdout.take().expect("piped stdout")),
    };
    let collector = std::thread::spawn(move || {
        let mut text = String::new();
        let _ = other.read_to_string(&mut text);
        text
    });
    let mut written = Vec::new();
    let mut buffer = [0u8; 4096];
    let mut drain = |controller: &mut fs::File| {
        // Stops when no more is waiting (WouldBlock), at EOF, or with EIO.
        while let Ok(count @ 1..) = controller.read(&mut buffer) {
            written.extend_from_slice(&buffer[..count]);
        }
    };
    let status = loop {
        drain(&mut controller);
        if let Some(status) = child.try_wait().expect("wait for qdu") {
            break status;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    };
    drain(&mut controller);
    let other = collector.join().expect("collect output");
    Some((
        status,
        String::from_utf8_lossy(&written).into_owned(),
        other,
    ))
}
