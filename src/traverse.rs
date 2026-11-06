//! Building blocks shared by the snapshot scan and the live file scan.

use std::cmp::Reverse;
use std::collections::hash_map::Entry;
use std::collections::{BinaryHeap, HashMap, HashSet};
use std::ffi::{OsStr, OsString};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use crate::error::{Error, Result};
use crate::sys::{self, Directory, DirectoryHandle, Stat};

/// Lock a mutex even if a panicking thread poisoned it: every update of the
/// shared collections is a single push or insert, so none is left half done.
pub fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// `parent/name`, where the root is `.`.
pub fn join_relative(parent: &str, name: &str) -> String {
    if parent == "." {
        name.to_string()
    } else {
        let mut path = String::with_capacity(parent.len() + 1 + name.len());
        path.push_str(parent);
        path.push('/');
        path.push_str(name);
        path
    }
}

const PROGRESS_INTERVAL: Duration = Duration::from_millis(500);

/// Run `work` on its own thread and call `report` twice a second until it
/// ends. A panic ends the thread too, and is raised again here.
pub fn run_reporting(work: impl FnOnce() + Send, report: Option<&(dyn Fn() + Sync)>) {
    std::thread::scope(|scope| {
        let worker = scope.spawn(work);
        let Some(report) = report else {
            return;
        };
        let mut next_report = Instant::now() + PROGRESS_INTERVAL;
        while !worker.is_finished() {
            std::thread::sleep(Duration::from_millis(50));
            if Instant::now() >= next_report {
                report();
                next_report += PROGRESS_INTERVAL;
            }
        }
    });
}

/// A thread pool for one walk.
pub fn thread_pool(threads: usize) -> Result<rayon::ThreadPool> {
    rayon::ThreadPoolBuilder::new()
        .num_threads(threads.max(1))
        .thread_name(|index| format!("qdu-walk-{index}"))
        .build()
        .map_err(|error| Error::Runtime(format!("cannot start scan threads: {error}")))
}

/// The N largest items seen so far, by their `Ord`.
struct TopN<T: Ord> {
    limit: usize,
    heap: BinaryHeap<Reverse<T>>,
}

impl<T: Ord> TopN<T> {
    fn new(limit: usize) -> Self {
        TopN {
            limit,
            heap: BinaryHeap::new(),
        }
    }

    /// Keep `item` if it is among the largest; true if it was kept.
    fn offer(&mut self, item: T) -> bool {
        if self.limit == 0 {
            return false;
        }
        if self.heap.len() < self.limit {
            self.heap.push(Reverse(item));
            return true;
        }
        if self.heap.peek().is_some_and(|smallest| item > smallest.0) {
            self.heap.pop();
            self.heap.push(Reverse(item));
            return true;
        }
        false
    }

    /// The smallest kept item once the limit is reached.
    fn floor(&self) -> Option<&T> {
        (self.heap.len() >= self.limit)
            .then(|| self.heap.peek().map(|smallest| &smallest.0))
            .flatten()
    }

    /// The kept items, largest first.
    fn into_sorted(self) -> Vec<T> {
        let mut items: Vec<T> = self.heap.into_iter().map(|item| item.0).collect();
        items.sort_by(|left, right| right.cmp(left));
        items
    }
}

/// A file found by a walk: larger files rank higher, and among equal sizes
/// the path that sorts first does, as queries list them.
#[derive(Clone, Debug)]
pub struct FileEntry {
    pub allocated: u64,
    pub path: String,
    pub uid: u32,
    pub apparent: u64,
    pub modified: f64,
}

impl PartialEq for FileEntry {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == std::cmp::Ordering::Equal
    }
}

impl Eq for FileEntry {}

impl PartialOrd for FileEntry {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for FileEntry {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.allocated
            .cmp(&other.allocated)
            .then_with(|| other.path.cmp(&self.path))
    }
}

/// The largest files seen so far, shared by the threads of a walk.
pub struct LargestFiles {
    top: Mutex<TopN<FileEntry>>,
    /// The smallest kept size once the list is full: smaller files are
    /// passed over without taking the lock.
    floor: AtomicU64,
    enabled: bool,
}

impl LargestFiles {
    pub fn new(limit: usize) -> Self {
        LargestFiles {
            top: Mutex::new(TopN::new(limit)),
            floor: AtomicU64::new(0),
            enabled: limit > 0,
        }
    }

    /// Whether a file of this size could still enter the list.
    pub fn wants(&self, allocated: u64) -> bool {
        self.enabled && allocated >= self.floor.load(Ordering::Relaxed)
    }

    pub fn offer(&self, file: FileEntry) {
        let mut top = lock(&self.top);
        if top.offer(file) {
            if let Some(floor) = top.floor() {
                self.floor.store(floor.allocated, Ordering::Relaxed);
            }
        }
    }

    /// The kept files, largest first and then by path.
    pub fn into_sorted(self) -> Vec<FileEntry> {
        self.top
            .into_inner()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .into_sorted()
    }
}

const SHARDS: usize = 64;

fn shard(device: u64, inode: u64) -> usize {
    ((device.wrapping_mul(31) ^ inode) as usize) % SHARDS
}

/// Directories already entered, by device and inode, so that a bind mount
/// or a mount loop does not walk a tree twice.
pub struct Claims {
    shards: Vec<Mutex<HashSet<(u64, u64)>>>,
}

impl Claims {
    pub fn new() -> Self {
        Claims {
            shards: (0..SHARDS).map(|_| Mutex::new(HashSet::new())).collect(),
        }
    }

    /// Claim a directory; false if another path already did.
    pub fn claim(&self, device: u64, inode: u64) -> bool {
        lock(&self.shards[shard(device, inode)]).insert((device, inode))
    }
}

impl Default for Claims {
    fn default() -> Self {
        Self::new()
    }
}

/// Something found at a path, compared by that path.
pub trait AtPath {
    fn path(&self) -> &str;
}

impl AtPath for FileEntry {
    fn path(&self) -> &str {
        &self.path
    }
}

/// One entry per hard-linked inode: the link whose path sorts first, so the
/// result does not depend on which thread found which link.
pub struct HardLinks<T: AtPath> {
    shards: Vec<Mutex<HashMap<(u64, u64), T>>>,
}

impl<T: AtPath> HardLinks<T> {
    pub fn new() -> Self {
        HardLinks {
            shards: (0..SHARDS).map(|_| Mutex::new(HashMap::new())).collect(),
        }
    }

    pub fn offer(&self, device: u64, inode: u64, link: T) {
        match lock(&self.shards[shard(device, inode)]).entry((device, inode)) {
            Entry::Occupied(mut kept) => {
                if link.path() < kept.get().path() {
                    kept.insert(link);
                }
            }
            Entry::Vacant(slot) => {
                slot.insert(link);
            }
        }
    }

    /// The kept links, in no particular order.
    pub fn into_links(self) -> impl Iterator<Item = T> {
        self.shards.into_iter().flat_map(|shard| {
            shard
                .into_inner()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .into_values()
        })
    }
}

impl<T: AtPath> Default for HardLinks<T> {
    fn default() -> Self {
        Self::new()
    }
}

/// How many directories may stay open at once beyond the one each thread
/// reads.
///
/// A directory stays open until each of its subdirectories has been opened
/// relative to it, and a large directory while its entries are examined on
/// several threads. The rest of the file descriptor limit is left for the
/// directories being read and for SQLite.
pub struct OpenBudget {
    open: AtomicUsize,
    limit: usize,
}

impl OpenBudget {
    pub fn new(threads: usize) -> Self {
        let soft = sys::open_file_limit().unwrap_or(256);
        let limit = usize::try_from(soft / 2)
            .unwrap_or(usize::MAX)
            .saturating_sub(threads + 16)
            .min(4096);
        OpenBudget {
            open: AtomicUsize::new(0),
            limit,
        }
    }

    /// Room for one more open directory, if there is any.
    pub fn reserve(&self) -> Option<Reservation<'_>> {
        // A compare-and-swap loop, as fetch_update does; newer Rust renames
        // that method, which the minimum supported version lacks.
        let mut open = self.open.load(Ordering::Acquire);
        while open < self.limit {
            match self.open.compare_exchange_weak(
                open,
                open + 1,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => return Some(Reservation { budget: self }),
                Err(current) => open = current,
            }
        }
        None
    }
}

/// One open directory counted against the budget until it is dropped.
pub struct Reservation<'b> {
    budget: &'b OpenBudget,
}

impl Drop for Reservation<'_> {
    fn drop(&mut self) {
        self.budget.open.fetch_sub(1, Ordering::AcqRel);
    }
}

/// A directory kept open for its subdirectories, shared by their tasks.
pub struct Opened<'b> {
    handle: DirectoryHandle,
    _reservation: Reservation<'b>,
}

impl<'b> Opened<'b> {
    /// Keep a directory open under a reservation; `None` if the descriptor
    /// cannot be kept, and the subdirectories are then opened by path.
    pub fn keep(directory: Directory, reservation: Reservation<'b>) -> Option<Arc<Self>> {
        let handle = directory.into_handle().ok()?;
        Some(Arc::new(Opened {
            handle,
            _reservation: reservation,
        }))
    }
}

/// Where the walk found a subdirectory, as its parent listed it.
pub struct Found<'b> {
    /// The parent, when it stayed open for its subdirectories.
    pub parent: Option<Arc<Opened<'b>>>,
    /// The parent's device and inode.
    pub parent_identity: (u64, u64),
    /// The full path; its last component is the name in the parent.
    pub path: PathBuf,
    /// The device and inode that the parent's listing reported.
    pub expected: (u64, u64),
}

impl Found<'_> {
    /// The name in the parent, as stored on disk.
    pub fn name(&self) -> &OsStr {
        self.path.file_name().unwrap_or(self.path.as_os_str())
    }
}

/// What a failed filesystem call means for the walk.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Failure {
    /// The entry was removed or replaced after its directory was listed, as
    /// temporary files are; it is left out without a record.
    Vanished,
    /// The user may not read it, and would get the same answer again; it is
    /// recorded, but the result still counts as complete.
    Denied,
    /// Anything else, such as EIO or ESTALE; it is recorded, and the result
    /// is incomplete.
    Error,
}

impl Failure {
    pub fn of(error: &io::Error) -> Self {
        match error.raw_os_error() {
            // ELOOP: a listed directory became a symbolic link, which is
            // never followed.
            Some(libc::ENOENT | libc::ENOTDIR | libc::ELOOP) => Failure::Vanished,
            Some(libc::EACCES | libc::EPERM) => Failure::Denied,
            Some(_) => Failure::Error,
            None if error.kind() == io::ErrorKind::NotFound => Failure::Vanished,
            None => Failure::Error,
        }
    }
}

/// The error for a directory that another one replaced after its parent was
/// listed.
fn replaced() -> io::Error {
    io::Error::new(
        io::ErrorKind::NotFound,
        "the directory was replaced during the scan",
    )
}

/// The outcome of opening a found subdirectory.
pub enum Entered {
    /// The directory the listing reported.
    Same(Directory, Stat),
    /// The root of a filesystem mounted there, as an automount point is once
    /// it is opened.
    Mounted(Directory, Stat),
}

/// Open a subdirectory and check that it is the one its parent listed.
///
/// The open is relative to the parent when it is still open, otherwise by
/// path, and never follows a final symbolic link. The device and inode must
/// match the listing, except at a mount point, which is accepted only while
/// its `..` is still the parent; so a directory swapped for another during
/// the walk cannot lead it outside the root.
pub fn enter(found: &Found, root: &Path) -> io::Result<Entered> {
    let directory = match &found.parent {
        Some(parent) => parent.handle.open_at(found.name()),
        None => open_path(root, &found.path),
    }?;
    let stat = directory.stat()?;
    if (stat.device, stat.inode) == found.expected {
        return Ok(Entered::Same(directory, stat));
    }
    if stat.device != found.expected.0 {
        let parent = directory.stat_parent()?;
        if (parent.device, parent.inode) == found.parent_identity {
            return Ok(Entered::Mounted(directory, stat));
        }
    }
    Err(replaced())
}

/// Open a directory below `root` by its path, one component at a time when
/// the path is longer than the system accepts in one call.
pub fn open_path(root: &Path, path: &Path) -> io::Result<Directory> {
    match Directory::open(path) {
        Err(error) if error.raw_os_error() == Some(libc::ENAMETOOLONG) => open_beneath(root, path),
        opened => opened,
    }
}

fn open_beneath(root: &Path, path: &Path) -> io::Result<Directory> {
    let relative = path
        .strip_prefix(root)
        .map_err(|_| io::Error::from_raw_os_error(libc::ENAMETOOLONG))?;
    let mut directory = Directory::open(root)?;
    for component in relative.components() {
        directory = directory.open_at(component.as_os_str())?;
    }
    Ok(directory)
}

/// The entry names of a directory, and the error that cut the listing
/// short, if any: on NFS a listing can break off with ESTALE or EIO, and the
/// names read so far are still worth examining.
pub fn read_names(directory: &mut Directory) -> (Vec<OsString>, Option<io::Error>) {
    let mut names = Vec::new();
    while let Some(next) = directory.next_name() {
        match next {
            Ok(name) => names.push(name),
            Err(error) => return (names, Some(error)),
        }
    }
    (names, None)
}

/// The name of a directory entry as text: valid UTF-8 as is, other bytes as
/// `\xNN` escapes.
pub fn entry_name(raw: &OsStr) -> String {
    match raw.to_str() {
        Some(name) => name.to_string(),
        None => sys::storable_text(raw),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn top_n_keeps_the_largest_and_breaks_ties_by_path() {
        let mut top = TopN::new(2);
        let file = |allocated, path: &str| FileEntry {
            allocated,
            path: path.into(),
            uid: 0,
            apparent: allocated,
            modified: 0.0,
        };
        for entry in [file(5, "c"), file(9, "a"), file(5, "b"), file(1, "d")] {
            top.offer(entry);
        }
        assert_eq!(top.floor().map(|entry| entry.path.as_str()), Some("b"));
        let kept: Vec<String> = top
            .into_sorted()
            .into_iter()
            .map(|entry| entry.path)
            .collect();
        assert_eq!(kept, ["a", "b"]);
        assert!(!TopN::<FileEntry>::new(0).offer(file(1, "x")));
    }

    #[test]
    fn hard_links_keep_the_first_path() {
        let links = HardLinks::new();
        for path in ["z/f", "a/f", "m/f"] {
            links.offer(
                1,
                7,
                FileEntry {
                    allocated: 1,
                    path: path.into(),
                    uid: 0,
                    apparent: 1,
                    modified: 0.0,
                },
            );
        }
        let kept: Vec<String> = links.into_links().map(|link| link.path).collect();
        assert_eq!(kept, ["a/f"]);
    }

    #[test]
    fn failures_are_classified_by_their_cause() {
        let of = |code| Failure::of(&io::Error::from_raw_os_error(code));
        assert_eq!(of(libc::ENOENT), Failure::Vanished);
        assert_eq!(of(libc::ENOTDIR), Failure::Vanished);
        assert_eq!(Failure::of(&replaced()), Failure::Vanished);
        assert_eq!(of(libc::EACCES), Failure::Denied);
        assert_eq!(of(libc::EPERM), Failure::Denied);
        assert_eq!(of(libc::EIO), Failure::Error);
        assert_eq!(of(libc::ESTALE), Failure::Error);
        assert_eq!(Failure::of(&io::Error::other("odd")), Failure::Error);
    }

    #[test]
    fn relative_paths_join_below_the_root() {
        assert_eq!(join_relative(".", "a"), "a");
        assert_eq!(join_relative("a/b", "c"), "a/b/c");
    }
}
