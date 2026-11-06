//! Read every directory below the root on a thread pool.
//!
//! Each directory is one task. A task opens its directory relative to the
//! parent's descriptor when the parent stayed open, otherwise by path;
//! examines the entries relative to its own descriptor; records the
//! directory as a [`Node`]; and spawns one task per subdirectory without
//! waiting for them, so the call stack stays shallow on deep trees.

use std::collections::HashMap;
use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::time::Instant;

use rayon::prelude::*;

use super::tree::NO_PARENT;
use super::{
    HardLink, Node, OwnerUsage, Owners, Phase, ScanError, ScanOptions, ScanProgress, Totals,
};
use crate::error::{Result, describe_io};
use crate::sys::{Directory, FilesystemUsage, Stat};
use crate::traverse::{
    self, Claims, Entered, Failure, FileEntry, Found, HardLinks, LargestFiles, OpenBudget, Opened,
    entry_name, join_relative, lock,
};

/// Directories with more entries than this examine them on several threads.
const PARALLEL_ENTRY_THRESHOLD: usize = 2048;
const ENTRY_CHUNK: usize = 512;

/// A filesystem the walk entered, at the path that sorts first among the
/// places where it did.
pub(super) struct Filesystem {
    pub relative: String,
    pub usage: io::Result<FilesystemUsage>,
    /// An automounter map, which lists only the entries mounted when read.
    pub automount: bool,
}

/// Everything the walk collected.
pub(super) struct Walked {
    /// The directories that were read, in no particular order.
    pub nodes: Vec<Node>,
    /// The number of IDs handed out, read or not.
    pub ids: u32,
    pub hard_links: HardLinks<HardLink>,
    pub large_files: LargestFiles,
    pub errors: Vec<ScanError>,
    pub error_count: u64,
    pub denied_count: u64,
    pub skipped: Vec<String>,
    pub filesystems: HashMap<u64, Filesystem>,
}

/// A per-thread collection, so threads append without contention.
struct PerThread<T> {
    slots: Vec<Mutex<Vec<T>>>,
}

impl<T> PerThread<T> {
    fn new(threads: usize) -> Self {
        PerThread {
            slots: (0..threads.max(1))
                .map(|_| Mutex::new(Vec::new()))
                .collect(),
        }
    }

    fn push(&self, value: T) {
        let slot = rayon::current_thread_index().unwrap_or(0) % self.slots.len();
        lock(&self.slots[slot]).push(value);
    }

    /// All values in one list, freeing each thread's list as it is moved.
    fn into_vec(self, capacity: usize) -> Vec<T> {
        let mut all = Vec::with_capacity(capacity);
        for slot in self.slots {
            all.extend(
                slot.into_inner()
                    .unwrap_or_else(|poisoned| poisoned.into_inner()),
            );
        }
        all
    }
}

/// State shared by all directory tasks.
struct Shared<'a> {
    options: &'a ScanOptions<'a>,
    root: &'a Path,
    root_device: u64,
    budget: OpenBudget,
    claims: Claims,
    next_id: AtomicU32,
    nodes: PerThread<Node>,
    hard_links: HardLinks<HardLink>,
    errors: PerThread<ScanError>,
    large_files: LargestFiles,
    skipped: Mutex<Vec<String>>,
    filesystems: Mutex<HashMap<u64, Filesystem>>,
    entries: AtomicU64,
    directories: AtomicU64,
    allocated: AtomicU64,
    error_count: AtomicU64,
    denied_count: AtomicU64,
    current_path: Mutex<String>,
    #[cfg(debug_assertions)]
    injected: Vec<String>,
}

impl Shared<'_> {
    /// Record a failure unless the entry merely vanished.
    fn record_failure(&self, path: String, operation: &'static str, error: &io::Error) {
        let message = describe_io(error);
        match Failure::of(error) {
            Failure::Vanished => {}
            Failure::Denied => self.record(ScanError {
                path,
                operation,
                message,
                denied: true,
            }),
            Failure::Error => self.record(ScanError {
                path,
                operation,
                message,
                denied: false,
            }),
        }
    }

    fn record(&self, error: ScanError) {
        let count = if error.denied {
            &self.denied_count
        } else {
            &self.error_count
        };
        count.fetch_add(1, Ordering::Relaxed);
        self.errors.push(error);
    }

    /// Debug builds fail to open the directories listed in
    /// `QDU_DEBUG_IO_ERROR`, relative paths separated by `:`, so that tests
    /// can produce an incomplete snapshot without a failing disk.
    #[cfg(debug_assertions)]
    fn injected_error(&self, relative: &str) -> Option<io::Error> {
        self.injected
            .iter()
            .any(|path| path == relative)
            .then(|| io::Error::from_raw_os_error(libc::EIO))
    }

    fn skip_filesystem(&self, relative: String) {
        lock(&self.skipped).push(relative);
    }

    /// Note a filesystem entered at `relative`, measuring it the first time.
    fn register_filesystem(&self, device: u64, relative: &str, directory: &Directory) {
        let mut filesystems = lock(&self.filesystems);
        match filesystems.get_mut(&device) {
            Some(known) if known.relative.as_str() <= relative => {}
            Some(known) => known.relative = relative.to_string(),
            None => {
                filesystems.insert(
                    device,
                    Filesystem {
                        relative: relative.to_string(),
                        usage: directory.statvfs(),
                        automount: directory.is_automount(),
                    },
                );
            }
        }
    }

    fn progress(&self, phase: Phase, started: Instant) -> ScanProgress {
        ScanProgress {
            phase,
            entries: self.entries.load(Ordering::Relaxed),
            directories: self.directories.load(Ordering::Relaxed),
            allocated_bytes: self.allocated.load(Ordering::Relaxed),
            errors: self.error_count.load(Ordering::Relaxed)
                + self.denied_count.load(Ordering::Relaxed),
            elapsed_seconds: started.elapsed().as_secs_f64(),
            current_path: lock(&self.current_path).clone(),
        }
    }
}

/// A directory waiting to be read.
struct Task<'s> {
    id: u32,
    parent: u32,
    depth: u32,
    found: Found<'s>,
    relative: String,
}

/// A directory that is open and about to be read.
struct Reading {
    id: u32,
    parent: u32,
    depth: u32,
    name: Box<str>,
    path: PathBuf,
    relative: String,
    stat: Stat,
}

/// A subdirectory found in a listing.
struct Subdirectory {
    id: u32,
    path: PathBuf,
    relative: String,
    expected: (u64, u64),
}

/// Entries whose status was denied. A directory that may be listed but not
/// searched denies every entry, so they are recorded as one failure.
#[derive(Default)]
struct DeniedEntries {
    count: u64,
    /// The entry whose path sorts first, and its error.
    first: Option<(String, io::Error)>,
}

impl DeniedEntries {
    fn add(&mut self, path: String, error: io::Error) {
        self.count += 1;
        self.keep_first(path, error);
    }

    fn merge(&mut self, other: DeniedEntries) {
        self.count += other.count;
        if let Some((path, error)) = other.first {
            self.keep_first(path, error);
        }
    }

    fn keep_first(&mut self, path: String, error: io::Error) {
        if self.first.as_ref().is_none_or(|(first, _)| path < *first) {
            self.first = Some((path, error));
        }
    }

    /// The failure to record for a directory, if any entry was denied.
    fn into_failure(self, directory: &str) -> Option<ScanError> {
        let (path, error) = self.first?;
        let (path, message) = if self.count == 1 {
            (path, describe_io(&error))
        } else {
            let message = format!("{} ({} entries)", describe_io(&error), self.count);
            (directory.to_string(), message)
        };
        Some(ScanError {
            path,
            operation: "stat",
            message,
            denied: true,
        })
    }
}

/// What one run of directory entries contributed.
#[derive(Default)]
struct Batch {
    totals: Totals,
    entries: u64,
    owners: Owners,
    subdirectories: Vec<Subdirectory>,
    denied: DeniedEntries,
}

impl Batch {
    fn merge(mut self, other: Batch) -> Batch {
        self.totals.add(&other.totals);
        self.entries += other.entries;
        self.owners.merge(&other.owners);
        self.subdirectories.extend(other.subdirectories);
        self.denied.merge(other.denied);
        self
    }
}

fn process_entries(
    shared: &Shared,
    reading: &Reading,
    directory: &Directory,
    names: &[OsString],
) -> Batch {
    let options = shared.options;
    let matcher = options.matcher;
    let check_entries = !matcher.is_empty();
    let needs_path = matcher.needs_path();
    let check_directories = matcher.has_directory_rules();
    let mut batch = Batch::default();
    for raw in names {
        let name = entry_name(raw);
        let relative = if needs_path {
            join_relative(&reading.relative, &name)
        } else {
            String::new()
        };
        if check_entries && matcher.matches_entry(&name, &relative) {
            continue;
        }
        let stat = match directory.lstat(raw) {
            Ok(stat) => stat,
            Err(error) => {
                let path = join_relative(&reading.relative, &name);
                match Failure::of(&error) {
                    Failure::Vanished => {}
                    Failure::Denied => batch.denied.add(path, error),
                    Failure::Error => shared.record_failure(path, "stat", &error),
                }
                continue;
            }
        };
        batch.entries += 1;
        let path = |relative: String| {
            if needs_path {
                relative
            } else {
                join_relative(&reading.relative, &name)
            }
        };
        if stat.is_dir() {
            if check_directories && matcher.matches_directory(&name, &relative) {
                continue;
            }
            let relative = path(relative);
            if options.one_file_system && stat.device != shared.root_device {
                shared.skip_filesystem(relative);
                continue;
            }
            batch.subdirectories.push(Subdirectory {
                id: shared.next_id.fetch_add(1, Ordering::Relaxed),
                path: reading.path.join(raw),
                relative,
                expected: (stat.device, stat.inode),
            });
            continue;
        }
        // Freshness follows paths: a hard-linked file updates every directory
        // that contains it, although its bytes are counted once.
        batch.totals.latest_modified_epoch = batch.totals.latest_modified_epoch.max(stat.modified);
        let allocated = stat.allocated();
        let file = |path: String| FileEntry {
            allocated,
            path,
            uid: stat.uid,
            apparent: stat.size,
            modified: stat.modified,
        };
        if stat.links > 1 {
            shared.hard_links.offer(
                stat.device,
                stat.inode,
                HardLink {
                    directory: reading.id,
                    file: file(path(relative)),
                },
            );
            continue;
        }
        batch
            .totals
            .add(&Totals::file(allocated, stat.size, stat.modified));
        if options.collect_users {
            batch
                .owners
                .add(stat.uid, &OwnerUsage::file(allocated, stat.size));
        }
        if shared.large_files.wants(allocated) {
            shared.large_files.offer(file(path(relative)));
        }
    }
    batch
}

/// Open a task's directory and read it, unless it is skipped.
fn enter_directory<'s>(scope: &rayon::Scope<'s>, shared: &'s Shared<'s>, task: Task<'s>) {
    let Task {
        id,
        parent,
        depth,
        found,
        relative,
    } = task;
    #[cfg(debug_assertions)]
    if let Some(error) = shared.injected_error(&relative) {
        shared.record_failure(relative, "scandir", &error);
        return;
    }
    let (directory, stat) = match traverse::enter(&found, shared.root) {
        Ok(Entered::Same(directory, stat)) => (directory, stat),
        Ok(Entered::Mounted(directory, stat)) => {
            if shared.options.one_file_system {
                shared.skip_filesystem(relative);
                return;
            }
            (directory, stat)
        }
        Err(error) => {
            shared.record_failure(relative, "scandir", &error);
            return;
        }
    };
    let name = entry_name(found.name()).into_boxed_str();
    let Found {
        parent: listed_in,
        parent_identity,
        path,
        ..
    } = found;
    // The parent closes once its last subdirectory is open.
    drop(listed_in);
    let boundary = stat.device != parent_identity.0;
    let reading = Reading {
        id,
        parent,
        depth,
        name,
        path,
        relative,
        stat,
    };
    read_directory(scope, shared, reading, directory, boundary);
}

fn read_directory<'s>(
    scope: &rayon::Scope<'s>,
    shared: &'s Shared<'s>,
    reading: Reading,
    mut directory: Directory,
    boundary: bool,
) {
    let stat = reading.stat;
    // Claiming every directory keeps a bind-mounted tree from being counted
    // twice and ends mount loops; btrfs and overlay directories report a
    // single link, so the link count cannot tell which ones to claim.
    if !shared.claims.claim(stat.device, stat.inode) {
        return;
    }
    if boundary {
        shared.register_filesystem(stat.device, &reading.relative, &directory);
    }
    let (names, error) = traverse::read_names(&mut directory);
    if let Some(error) = error {
        shared.record_failure(reading.relative.clone(), "readdir", &error);
    }
    // A thread that waits for the chunks runs other tasks meanwhile, which
    // open directories of their own, so a directory is examined in parallel
    // only while it counts against the budget.
    let reservation = (names.len() > PARALLEL_ENTRY_THRESHOLD)
        .then(|| shared.budget.reserve())
        .flatten();
    let mut batch = if reservation.is_some() {
        names
            .par_chunks(ENTRY_CHUNK)
            .map(|chunk| process_entries(shared, &reading, &directory, chunk))
            .reduce(Batch::default, Batch::merge)
    } else {
        process_entries(shared, &reading, &directory, &names)
    };
    drop(names);
    if let Some(failure) = std::mem::take(&mut batch.denied).into_failure(&reading.relative) {
        shared.record(failure);
    }
    // Subdirectories open relative to this one while the budget allows.
    let opened = if batch.subdirectories.is_empty() {
        None
    } else {
        reservation
            .or_else(|| shared.budget.reserve())
            .and_then(|reservation| Opened::keep(directory, reservation))
    };

    shared.entries.fetch_add(batch.entries, Ordering::Relaxed);
    shared.directories.fetch_add(1, Ordering::Relaxed);
    shared.allocated.fetch_add(
        stat.allocated()
            .saturating_add(batch.totals.allocated_bytes),
        Ordering::Relaxed,
    );
    if let Ok(mut current) = shared.current_path.try_lock() {
        current.clone_from(&reading.relative);
    }

    let mut owners = batch.owners;
    if shared.options.collect_users {
        owners.add(
            stat.uid,
            &OwnerUsage::directory(stat.allocated(), stat.size),
        );
    }
    let mut totals = Totals::directory(&stat);
    totals.add(&batch.totals);
    let parent_identity = (stat.device, stat.inode);
    let Reading {
        id,
        parent,
        depth,
        name,
        ..
    } = reading;
    shared.nodes.push(Node {
        id,
        parent,
        depth,
        name,
        totals,
        owners,
    });
    for child in batch.subdirectories {
        let task = Task {
            id: child.id,
            parent: id,
            depth: depth + 1,
            found: Found {
                parent: opened.clone(),
                parent_identity,
                path: child.path,
                expected: child.expected,
            },
            relative: child.relative,
        };
        scope.spawn(move |scope| enter_directory(scope, shared, task));
    }
}

/// Read the tree below an open root on a pool of `options.threads` threads.
pub(super) fn walk(
    options: &ScanOptions,
    root: &Path,
    root_directory: Directory,
    root_stat: Stat,
    progress: Option<&(dyn Fn(&ScanProgress) + Sync)>,
    started: Instant,
) -> Result<Walked> {
    let threads = options.threads.max(1);
    let pool = traverse::thread_pool(threads)?;
    let shared = Shared {
        options,
        root,
        root_device: root_stat.device,
        budget: OpenBudget::new(threads),
        claims: Claims::new(),
        next_id: AtomicU32::new(1),
        nodes: PerThread::new(threads),
        hard_links: HardLinks::new(),
        errors: PerThread::new(threads),
        large_files: LargestFiles::new(
            usize::try_from(options.large_file_limit).unwrap_or(usize::MAX),
        ),
        skipped: Mutex::new(Vec::new()),
        filesystems: Mutex::new(HashMap::new()),
        entries: AtomicU64::new(0),
        directories: AtomicU64::new(0),
        allocated: AtomicU64::new(0),
        error_count: AtomicU64::new(0),
        denied_count: AtomicU64::new(0),
        current_path: Mutex::new(String::new()),
        #[cfg(debug_assertions)]
        injected: std::env::var("QDU_DEBUG_IO_ERROR")
            .map(|paths| paths.split(':').map(str::to_string).collect())
            .unwrap_or_default(),
    };
    let reading = Reading {
        id: 0,
        parent: NO_PARENT,
        depth: 0,
        name: "".into(),
        path: root.to_path_buf(),
        relative: ".".to_string(),
        stat: root_stat,
    };
    {
        let (pool, shared) = (&pool, &shared);
        let report =
            progress.map(|progress| move || progress(&shared.progress(Phase::Scanning, started)));
        traverse::run_reporting(
            move || {
                pool.install(move || {
                    rayon::scope(move |scope| {
                        read_directory(scope, shared, reading, root_directory, true);
                    });
                });
            },
            report.as_ref().map(|report| report as &(dyn Fn() + Sync)),
        );
    }
    if let Some(progress) = progress {
        progress(&shared.progress(Phase::Writing, started));
    }

    let Shared {
        next_id,
        nodes,
        directories,
        hard_links,
        errors,
        large_files,
        skipped,
        filesystems,
        error_count,
        denied_count,
        ..
    } = shared;
    let directories = usize::try_from(directories.into_inner()).unwrap_or(0);
    Ok(Walked {
        nodes: nodes.into_vec(directories),
        ids: next_id.into_inner(),
        hard_links,
        large_files,
        errors: errors.into_vec(0),
        error_count: error_count.into_inner(),
        denied_count: denied_count.into_inner(),
        skipped: skipped
            .into_inner()
            .unwrap_or_else(|poisoned| poisoned.into_inner()),
        filesystems: filesystems
            .into_inner()
            .unwrap_or_else(|poisoned| poisoned.into_inner()),
    })
}
