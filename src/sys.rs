//! Thin safe wrappers around the Unix calls that qdu needs.

use std::ffi::{CStr, CString, OsStr, OsString};
use std::io;
use std::mem::MaybeUninit;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// A broken-down local time.
pub struct LocalTime {
    pub year: i32,
    pub month: u32,
    pub day: u32,
    pub hour: u32,
    pub minute: u32,
    pub second: u32,
    pub offset_seconds: i64,
}

/// Convert an epoch to local time, honoring `TZ` and the system time zone.
pub fn local_time(epoch: i64) -> LocalTime {
    let time: libc::time_t = epoch as libc::time_t;
    // SAFETY: `tm` is plain data that localtime_r fully initializes on
    // success; both pointers are valid for the duration of the call.
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    let converted = unsafe { !libc::localtime_r(&time, &mut tm).is_null() };
    if !converted {
        return LocalTime {
            year: 1970,
            month: 1,
            day: 1,
            hour: 0,
            minute: 0,
            second: 0,
            offset_seconds: 0,
        };
    }
    LocalTime {
        year: tm.tm_year + 1900,
        month: (tm.tm_mon + 1) as u32,
        day: tm.tm_mday as u32,
        hour: tm.tm_hour as u32,
        minute: tm.tm_min as u32,
        second: tm.tm_sec.min(59) as u32,
        offset_seconds: tm.tm_gmtoff as i64,
    }
}

/// The epoch of local midnight at the start of a date; a day past the end
/// of its month continues into the next.
pub fn local_epoch(year: i32, month: u32, day: u32) -> Option<i64> {
    // SAFETY: `tm` is plain data; mktime reads and normalizes it.
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    tm.tm_year = year - 1900;
    tm.tm_mon = i32::try_from(month).ok()? - 1;
    tm.tm_mday = i32::try_from(day).ok()?;
    // Let mktime decide whether daylight saving time applies.
    tm.tm_isdst = -1;
    let epoch = unsafe { libc::mktime(&mut tm) };
    (epoch != -1).then_some(epoch as i64)
}

/// Render an epoch as a compact UTC stamp such as `20260824T031513Z`.
pub fn utc_stamp(epoch: i64) -> String {
    let time: libc::time_t = epoch as libc::time_t;
    // SAFETY: as in local_time, with gmtime_r.
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    unsafe {
        libc::gmtime_r(&time, &mut tm);
    }
    format!(
        "{:04}{:02}{:02}T{:02}{:02}{:02}Z",
        tm.tm_year + 1900,
        tm.tm_mon + 1,
        tm.tm_mday,
        tm.tm_hour,
        tm.tm_min,
        tm.tm_sec.min(59)
    )
}

/// Seconds since the Unix epoch as a float.
pub fn now() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs_f64())
        .unwrap_or(0.0)
}

/// Whole seconds since the Unix epoch.
pub fn now_epoch() -> i64 {
    now().floor() as i64
}

/// Capacity of the filesystem that holds a path.
#[derive(Clone, Copy, Debug, Default)]
pub struct FilesystemUsage {
    pub total_bytes: u64,
    /// Free to unprivileged users.
    pub available_bytes: u64,
    /// Free, including the blocks reserved for root.
    pub free_bytes: u64,
    pub total_inodes: u64,
    pub available_inodes: u64,
}

fn c_path(path: &Path) -> Option<CString> {
    CString::new(path.as_os_str().as_bytes()).ok()
}

impl FilesystemUsage {
    // The field types differ between platforms: macOS has 32-bit block
    // counts, so the casts are needed there.
    #[allow(clippy::unnecessary_cast)]
    fn from_raw(value: &libc::statvfs) -> Self {
        let fragment = value.f_frsize as u64;
        FilesystemUsage {
            total_bytes: (value.f_blocks as u64).saturating_mul(fragment),
            available_bytes: (value.f_bavail as u64).saturating_mul(fragment),
            free_bytes: (value.f_bfree as u64).saturating_mul(fragment),
            total_inodes: value.f_files as u64,
            available_inodes: value.f_favail as u64,
        }
    }
}

/// Query `statvfs` for a path.
pub fn statvfs(path: &Path) -> std::io::Result<FilesystemUsage> {
    let path =
        c_path(path).ok_or_else(|| std::io::Error::from(std::io::ErrorKind::InvalidInput))?;
    // SAFETY: `value` is plain data written by statvfs; the path is a valid
    // NUL-terminated string that outlives the call.
    let mut value: libc::statvfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statvfs(path.as_ptr(), &mut value) } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(FilesystemUsage::from_raw(&value))
}

/// The host name, or `localhost` when it cannot be read.
pub fn hostname() -> String {
    let mut buffer = [0u8; 256];
    // SAFETY: the buffer is writable for its full length.
    let result =
        unsafe { libc::gethostname(buffer.as_mut_ptr().cast::<libc::c_char>(), buffer.len()) };
    if result != 0 {
        return "localhost".to_string();
    }
    let end = buffer
        .iter()
        .position(|byte| *byte == 0)
        .unwrap_or(buffer.len());
    String::from_utf8_lossy(&buffer[..end]).into_owned()
}

/// Resolve a UID to a local account name, falling back to its decimal form.
pub fn username_for_uid(uid: u32) -> String {
    let mut buffer = vec![0u8; 4096];
    loop {
        // SAFETY: `entry` and `buffer` are valid for writes; getpwuid_r only
        // stores pointers into `buffer`, which outlives their use below.
        let mut entry: libc::passwd = unsafe { std::mem::zeroed() };
        let mut found: *mut libc::passwd = std::ptr::null_mut();
        let result = unsafe {
            libc::getpwuid_r(
                uid as libc::uid_t,
                &mut entry,
                buffer.as_mut_ptr().cast::<libc::c_char>(),
                buffer.len(),
                &mut found,
            )
        };
        if result == libc::ERANGE && buffer.len() < 1 << 20 {
            buffer.resize(buffer.len() * 2, 0);
            continue;
        }
        if result != 0 || found.is_null() || entry.pw_name.is_null() {
            return uid.to_string();
        }
        // SAFETY: pw_name points to a NUL-terminated string inside `buffer`.
        let name = unsafe { CStr::from_ptr(entry.pw_name) };
        return name.to_string_lossy().into_owned();
    }
}

/// Resolve a local account name to its UID.
pub fn uid_for_username(name: &str) -> Option<u32> {
    let name = CString::new(name).ok()?;
    let mut buffer = vec![0u8; 4096];
    loop {
        // SAFETY: as in username_for_uid.
        let mut entry: libc::passwd = unsafe { std::mem::zeroed() };
        let mut found: *mut libc::passwd = std::ptr::null_mut();
        let result = unsafe {
            libc::getpwnam_r(
                name.as_ptr(),
                &mut entry,
                buffer.as_mut_ptr().cast::<libc::c_char>(),
                buffer.len(),
                &mut found,
            )
        };
        if result == libc::ERANGE && buffer.len() < 1 << 20 {
            buffer.resize(buffer.len() * 2, 0);
            continue;
        }
        if result != 0 || found.is_null() {
            return None;
        }
        return Some(entry.pw_uid as u32);
    }
}

/// Whether a file descriptor refers to a terminal.
pub fn is_terminal(descriptor: i32) -> bool {
    // SAFETY: isatty only inspects the descriptor number.
    unsafe { libc::isatty(descriptor) == 1 }
}

/// The terminal width from `COLUMNS` or standard output, or a fallback.
pub fn terminal_columns(fallback: usize) -> usize {
    terminal_columns_of(libc::STDOUT_FILENO, fallback)
}

/// The terminal width from `COLUMNS` or the given descriptor, or a fallback.
pub fn terminal_columns_of(descriptor: i32, fallback: usize) -> usize {
    if let Ok(value) = std::env::var("COLUMNS") {
        if let Ok(columns) = value.trim().parse::<usize>() {
            if columns > 0 {
                return columns;
            }
        }
    }
    // SAFETY: `size` is plain data that the ioctl fills on success.
    let mut size: libc::winsize = unsafe { std::mem::zeroed() };
    let result = unsafe { libc::ioctl(descriptor, libc::TIOCGWINSZ, &mut size) };
    if result == 0 && size.ws_col > 0 {
        usize::from(size.ws_col)
    } else {
        fallback
    }
}

/// Expand a leading `~` to the home directory.
pub fn expand_user(path: &str) -> std::path::PathBuf {
    if path == "~" {
        return home_dir();
    }
    if let Some(rest) = path.strip_prefix("~/") {
        return home_dir().join(rest);
    }
    std::path::PathBuf::from(path)
}

/// The home directory from `HOME`, or `/` when it is unset.
pub fn home_dir() -> std::path::PathBuf {
    std::env::var_os("HOME")
        .filter(|value| !value.is_empty())
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from("/"))
}

/// The status fields of a file that qdu records.
#[derive(Clone, Copy, Debug)]
pub struct Stat {
    pub device: u64,
    pub inode: u64,
    mode: u32,
    pub links: u64,
    pub uid: u32,
    pub size: u64,
    blocks: u64,
    pub modified: f64,
}

impl Stat {
    // The field types differ between platforms: macOS has a 32-bit device
    // and 16-bit mode and link count, so the casts are needed there.
    #[allow(clippy::unnecessary_cast)]
    fn from_raw(raw: &libc::stat) -> Self {
        Stat {
            device: raw.st_dev as u64,
            inode: raw.st_ino as u64,
            mode: raw.st_mode as u32,
            links: raw.st_nlink as u64,
            uid: raw.st_uid,
            size: raw.st_size.max(0) as u64,
            blocks: raw.st_blocks.max(0) as u64,
            modified: (raw.st_mtime as f64 + raw.st_mtime_nsec as f64 / 1e9).max(0.0),
        }
    }

    // mode_t is 16 bits on macOS.
    #[allow(clippy::unnecessary_cast)]
    pub fn is_dir(&self) -> bool {
        self.mode & libc::S_IFMT as u32 == libc::S_IFDIR as u32
    }

    /// Bytes allocated on disk; `st_blocks` counts 512-byte units on Linux
    /// and macOS.
    pub fn allocated(&self) -> u64 {
        self.blocks.saturating_mul(512)
    }
}

fn c_bytes(path: &[u8]) -> io::Result<CString> {
    CString::new(path).map_err(|_| io::Error::from(io::ErrorKind::InvalidInput))
}

/// `fstatat` without following a final symbolic link.
///
/// Unlike `statx`, which the standard library uses on Linux, the stat calls
/// do not trigger automounts, so a scan does not mount every autofs entry
/// it lists.
fn stat_at(directory: libc::c_int, name: &CStr) -> io::Result<Stat> {
    let mut raw = MaybeUninit::<libc::stat>::uninit();
    // SAFETY: `name` is NUL-terminated and `raw` is only read after the call
    // reports that it filled it.
    let result = unsafe {
        libc::fstatat(
            directory,
            name.as_ptr(),
            raw.as_mut_ptr(),
            libc::AT_SYMLINK_NOFOLLOW,
        )
    };
    if result != 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: fstatat succeeded, so it initialized `raw`.
    Ok(Stat::from_raw(&unsafe { raw.assume_init() }))
}

/// `lstat` of a path; see [`stat_at`] for why not `std::fs`.
pub fn lstat(path: &Path) -> io::Result<Stat> {
    stat_at(libc::AT_FDCWD, &c_bytes(path.as_os_str().as_bytes())?)
}

#[cfg(any(target_os = "linux", target_os = "android"))]
fn errno_location() -> *mut libc::c_int {
    // SAFETY: returns the calling thread's errno slot.
    unsafe { libc::__errno_location() }
}

#[cfg(any(
    target_os = "macos",
    target_os = "ios",
    target_os = "freebsd",
    target_os = "dragonfly"
))]
fn errno_location() -> *mut libc::c_int {
    // SAFETY: returns the calling thread's errno slot.
    unsafe { libc::__error() }
}

/// The soft limit on open file descriptors, if the system reports one.
// `rlim_t` is narrower than 64 bits on some platforms.
#[allow(clippy::unnecessary_cast)]
pub fn open_file_limit() -> Option<u64> {
    let mut limit = MaybeUninit::<libc::rlimit>::uninit();
    // SAFETY: getrlimit fills `limit` and is only read after it succeeds.
    if unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, limit.as_mut_ptr()) } != 0 {
        return None;
    }
    // SAFETY: getrlimit succeeded, so it initialized `limit`.
    let current = unsafe { limit.assume_init() }.rlim_cur;
    (current != libc::RLIM_INFINITY).then_some(current as u64)
}

/// An open directory whose entries are examined relative to it.
pub struct Directory {
    stream: std::ptr::NonNull<libc::DIR>,
}

// SAFETY: the stream belongs to one owner, so it may move to another thread
// with that owner.
unsafe impl Send for Directory {}

// SAFETY: methods that take `&self` only call fstat and fstatat on the
// descriptor, which are safe from several threads; reading the stream needs
// `&mut self`, so it never runs concurrently.
unsafe impl Sync for Directory {}

impl Directory {
    /// Open a directory, failing if the final component is a symbolic link.
    pub fn open(path: &Path) -> io::Result<Self> {
        Self::open_relative(libc::AT_FDCWD, &c_bytes(path.as_os_str().as_bytes())?)
    }

    /// Open a subdirectory of this directory by name, without following a
    /// symbolic link. No path is looked up, so the length of the full path
    /// does not matter and a renamed ancestor cannot redirect the open.
    pub fn open_at(&self, name: &OsStr) -> io::Result<Self> {
        Self::open_relative(self.descriptor(), &c_bytes(name.as_bytes())?)
    }

    fn open_relative(directory: libc::c_int, path: &CStr) -> io::Result<Self> {
        // SAFETY: `path` is NUL-terminated and `directory` is AT_FDCWD or an
        // open descriptor; the result is checked below.
        let descriptor = unsafe {
            libc::openat(
                directory,
                path.as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            )
        };
        if descriptor < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: the descriptor is open; on success the stream owns it.
        let stream = unsafe { libc::fdopendir(descriptor) };
        match std::ptr::NonNull::new(stream) {
            Some(stream) => Ok(Directory { stream }),
            None => {
                let error = io::Error::last_os_error();
                // SAFETY: fdopendir failed, so the descriptor is still ours.
                unsafe { libc::close(descriptor) };
                Err(error)
            }
        }
    }

    fn descriptor(&self) -> libc::c_int {
        // SAFETY: the stream is open for the lifetime of `self`.
        unsafe { libc::dirfd(self.stream.as_ptr()) }
    }

    /// The status of the directory itself.
    pub fn stat(&self) -> io::Result<Stat> {
        let mut raw = MaybeUninit::<libc::stat>::uninit();
        // SAFETY: the descriptor is open and `raw` is read only after fstat
        // reports success.
        let result = unsafe { libc::fstat(self.descriptor(), raw.as_mut_ptr()) };
        if result != 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: fstat succeeded, so it initialized `raw`.
        Ok(Stat::from_raw(&unsafe { raw.assume_init() }))
    }

    /// The next entry name, skipping `.` and `..`.
    pub fn next_name(&mut self) -> Option<io::Result<OsString>> {
        loop {
            // readdir reports errors only through errno, so clear it first.
            // SAFETY: errno_location returns this thread's errno slot.
            unsafe { *errno_location() = 0 };
            // SAFETY: the stream is open and used by this owner only.
            let entry = unsafe { libc::readdir(self.stream.as_ptr()) };
            if entry.is_null() {
                let error = io::Error::last_os_error();
                return match error.raw_os_error() {
                    Some(0) | None => None,
                    Some(_) => Some(Err(error)),
                };
            }
            // SAFETY: readdir returned an entry whose name is NUL-terminated
            // and stays valid until the next readdir call.
            let name = unsafe { CStr::from_ptr((*entry).d_name.as_ptr()) }.to_bytes();
            if name != b"." && name != b".." {
                return Some(Ok(OsStr::from_bytes(name).to_os_string()));
            }
        }
    }

    /// Keep only the descriptor, which opens entries relative to this
    /// directory, and free the stream with its buffer of entries.
    pub fn into_handle(self) -> io::Result<DirectoryHandle> {
        // SAFETY: the descriptor is open; the duplicate is checked below.
        let duplicate = unsafe { libc::fcntl(self.descriptor(), libc::F_DUPFD_CLOEXEC, 0) };
        if duplicate < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: fcntl returned a new descriptor that nothing else owns.
        let descriptor = unsafe { OwnedFd::from_raw_fd(duplicate) };
        Ok(DirectoryHandle { descriptor })
    }

    /// The status of the directory that contains this one, through `..`.
    pub fn stat_parent(&self) -> io::Result<Stat> {
        stat_at(self.descriptor(), c"..")
    }

    /// The capacity of the filesystem that holds this directory.
    pub fn statvfs(&self) -> io::Result<FilesystemUsage> {
        // SAFETY: `value` is plain data that fstatvfs writes; the descriptor
        // is open.
        let mut value: libc::statvfs = unsafe { std::mem::zeroed() };
        if unsafe { libc::fstatvfs(self.descriptor(), &mut value) } != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(FilesystemUsage::from_raw(&value))
    }

    /// Whether this directory is the mount point of an automounter map,
    /// whose listing holds only the entries mounted at the moment.
    #[cfg(any(target_os = "linux", target_os = "android"))]
    pub fn is_automount(&self) -> bool {
        const AUTOFS_SUPER_MAGIC: i64 = 0x0187;
        // SAFETY: `value` is plain data that fstatfs writes; the descriptor
        // is open.
        let mut value: libc::statfs = unsafe { std::mem::zeroed() };
        // The type of f_type differs between C libraries.
        #[allow(clippy::unnecessary_cast)]
        let automount = unsafe { libc::fstatfs(self.descriptor(), &mut value) } == 0
            && value.f_type as i64 == AUTOFS_SUPER_MAGIC;
        automount
    }

    /// Whether this directory is the mount point of an automounter map,
    /// whose listing holds only the entries mounted at the moment.
    #[cfg(not(any(target_os = "linux", target_os = "android")))]
    pub fn is_automount(&self) -> bool {
        // SAFETY: `value` is plain data that fstatfs writes; the descriptor
        // is open.
        let mut value: libc::statfs = unsafe { std::mem::zeroed() };
        let read = unsafe { libc::fstatfs(self.descriptor(), &mut value) };
        if read != 0 {
            return false;
        }
        // SAFETY: fstatfs filled f_fstypename with a NUL-terminated name.
        let name = unsafe { std::ffi::CStr::from_ptr(value.f_fstypename.as_ptr()) };
        name.to_bytes() == b"autofs"
    }

    /// `lstat` of an entry of this directory.
    pub fn lstat(&self, name: &OsStr) -> io::Result<Stat> {
        stat_at(self.descriptor(), &c_bytes(name.as_bytes())?)
    }
}

impl Drop for Directory {
    fn drop(&mut self) {
        // SAFETY: the stream is open and closed exactly once, here.
        unsafe { libc::closedir(self.stream.as_ptr()) };
    }
}

/// A directory kept open without a stream, to open its subdirectories
/// relative to it. A stream holds a buffer of 32 KiB or more for its
/// entries; a handle holds only the descriptor.
pub struct DirectoryHandle {
    descriptor: OwnedFd,
}

impl DirectoryHandle {
    /// Open a subdirectory by name; see [`Directory::open_at`].
    pub fn open_at(&self, name: &OsStr) -> io::Result<Directory> {
        Directory::open_relative(self.descriptor.as_raw_fd(), &c_bytes(name.as_bytes())?)
    }
}

/// Find an executable on `PATH`.
pub fn find_executable(name: &str) -> Option<PathBuf> {
    use std::os::unix::fs::PermissionsExt;
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|directory| directory.join(name))
        .find(|candidate| {
            std::fs::metadata(candidate)
                .map(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
                .unwrap_or(false)
        })
}

/// Whether this process may read and search a directory.
pub fn readable(path: &Path) -> bool {
    access(path, libc::R_OK | libc::X_OK)
}

/// Whether this process may create entries in a directory.
pub fn writable(path: &Path) -> bool {
    access(path, libc::W_OK | libc::X_OK)
}

fn access(path: &Path, mode: libc::c_int) -> bool {
    let Ok(path) = CString::new(path.as_os_str().as_bytes()) else {
        return false;
    };
    // SAFETY: access only reads the NUL-terminated path.
    unsafe { libc::access(path.as_ptr(), mode) == 0 }
}

/// Text that SQLite and JSON can store for a file name.
///
/// Valid UTF-8 is returned unchanged. Bytes that are not valid UTF-8 are
/// spelled as `\xNN` escapes, as Python's `backslashreplace` does.
pub fn storable_text(name: &OsStr) -> String {
    let bytes = name.as_bytes();
    match std::str::from_utf8(bytes) {
        Ok(text) => text.to_string(),
        Err(_) => escape_invalid_utf8(bytes),
    }
}

fn escape_invalid_utf8(mut bytes: &[u8]) -> String {
    let mut text = String::with_capacity(bytes.len() + 8);
    loop {
        match std::str::from_utf8(bytes) {
            Ok(valid) => {
                text.push_str(valid);
                return text;
            }
            Err(error) => {
                let (valid, rest) = bytes.split_at(error.valid_up_to());
                // SAFETY: from_utf8 reported that this prefix is valid UTF-8.
                text.push_str(unsafe { std::str::from_utf8_unchecked(valid) });
                let invalid_length = error.error_len().unwrap_or(rest.len());
                for byte in &rest[..invalid_length] {
                    text.push_str(&format!("\\x{byte:02x}"));
                }
                bytes = &rest[invalid_length..];
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::ffi::OsStrExt;

    #[test]
    fn an_ordinary_directory_is_not_an_automount_point() {
        let directory = Directory::open(&std::env::temp_dir()).unwrap();
        assert!(!directory.is_automount());
    }

    #[test]
    fn invalid_utf8_bytes_become_escapes() {
        let name = OsStr::from_bytes(b"report-\x93\xfa.txt");
        assert_eq!(storable_text(name), "report-\\x93\\xfa.txt");
        assert_eq!(storable_text(OsStr::new("caf\u{e9}")), "caf\u{e9}");
    }
}
