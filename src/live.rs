//! Find the largest files by walking the live filesystem in parallel.

use std::io;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use crate::error::{Result, describe_io, usage};
use crate::model::FileRecord;
use crate::patterns::PathMatcher;
use crate::query::normalize_relative_path;
use crate::sys::{self, Directory, Stat, storable_text};
use crate::traverse::{
    self, Claims, Entered, Failure, FileEntry, Found, HardLinks, LargestFiles, OpenBudget, Opened,
    entry_name, join_relative, lock,
};

/// Filters for a live scan.
pub struct LiveFilter<'a> {
    pub root: &'a Path,
    pub matcher: &'a PathMatcher,
    pub top: usize,
    pub under: &'a str,
    pub min_size: u64,
    pub uid: Option<u32>,
    pub modified_before_epoch: Option<f64>,
    pub one_file_system: bool,
    pub threads: usize,
    pub progress: Option<&'a (dyn Fn(&LiveProgress) + Sync)>,
}

/// Counters reported periodically while a live scan runs.
pub struct LiveProgress {
    pub entries: u64,
    pub directories: u64,
    pub elapsed_seconds: f64,
    pub current_path: String,
}

impl LiveFilter<'_> {
    fn accepts(&self, stat: &Stat) -> bool {
        self.uid.is_none_or(|uid| uid == stat.uid)
            && stat.allocated() >= self.min_size
            && self
                .modified_before_epoch
                .is_none_or(|cutoff| stat.modified <= cutoff)
    }
}

struct State<'a> {
    filter: &'a LiveFilter<'a>,
    root: &'a Path,
    root_device: u64,
    budget: OpenBudget,
    claims: Claims,
    largest: LargestFiles,
    /// One file per hard-linked inode: the link whose path sorts first.
    hard_links: HardLinks<FileEntry>,
    errors: Mutex<Vec<String>>,
    entries: AtomicU64,
    directories: AtomicU64,
    current_path: Mutex<String>,
}

impl State<'_> {
    /// Report a failure unless the entry merely vanished.
    fn error(&self, path: &Path, error: &io::Error) {
        if Failure::of(error) == Failure::Vanished {
            return;
        }
        lock(&self.errors).push(format!(
            "{}: {}",
            storable_text(path.as_os_str()),
            describe_io(error)
        ));
    }
}

fn enter<'s>(scope: &rayon::Scope<'s>, state: &'s State<'s>, found: Found<'s>, relative: String) {
    let (directory, stat) = match traverse::enter(&found, state.root) {
        Ok(Entered::Same(directory, stat)) => (directory, stat),
        // Opening mounted another filesystem, as an automount point does.
        Ok(Entered::Mounted(directory, stat)) if !state.filter.one_file_system => (directory, stat),
        Ok(Entered::Mounted(..)) => return,
        Err(error) => {
            state.error(&found.path, &error);
            return;
        }
    };
    let Found {
        parent: listed_in,
        path,
        ..
    } = found;
    // The parent closes once its last subdirectory is open.
    drop(listed_in);
    list(scope, state, directory, stat, path, relative);
}

fn list<'s>(
    scope: &rayon::Scope<'s>,
    state: &'s State<'s>,
    mut directory: Directory,
    stat: Stat,
    path: PathBuf,
    relative: String,
) {
    if !state.claims.claim(stat.device, stat.inode) {
        return;
    }
    let filter = state.filter;
    let (names, error) = traverse::read_names(&mut directory);
    if let Some(error) = error {
        state.error(&path, &error);
    }
    state
        .entries
        .fetch_add(names.len() as u64, Ordering::Relaxed);
    state.directories.fetch_add(1, Ordering::Relaxed);
    if let Ok(mut current) = state.current_path.try_lock() {
        current.clone_from(&relative);
    }
    let mut subdirectories = Vec::new();
    for raw in names {
        let name = entry_name(&raw);
        let child = join_relative(&relative, &name);
        if filter.matcher.matches_entry(&name, &child) {
            continue;
        }
        let entry = match directory.lstat(&raw) {
            Ok(entry) => entry,
            Err(error) => {
                state.error(&path.join(&raw), &error);
                continue;
            }
        };
        if entry.is_dir() {
            let other_filesystem = filter.one_file_system && entry.device != state.root_device;
            if !other_filesystem && !filter.matcher.matches_directory(&name, &child) {
                subdirectories.push((path.join(&raw), child, (entry.device, entry.inode)));
            }
            continue;
        }
        if !filter.accepts(&entry) {
            continue;
        }
        let file = FileEntry {
            allocated: entry.allocated(),
            path: child,
            uid: entry.uid,
            apparent: entry.size,
            modified: entry.modified,
        };
        if entry.links > 1 {
            state.hard_links.offer(entry.device, entry.inode, file);
        } else if state.largest.wants(file.allocated) {
            state.largest.offer(file);
        }
    }
    if subdirectories.is_empty() {
        return;
    }
    // Subdirectories open relative to this one while the budget allows.
    let opened = state
        .budget
        .reserve()
        .and_then(|reservation| Opened::keep(directory, reservation));
    for (path, relative, expected) in subdirectories {
        let found = Found {
            parent: opened.clone(),
            parent_identity: (stat.device, stat.inode),
            path,
            expected,
        };
        scope.spawn(move |scope| enter(scope, state, found, relative));
    }
}

/// The directory to start from, open, or the message that explains why the
/// walk cannot start there.
fn open_start(root: &Path, start: &Path) -> std::result::Result<(Directory, Stat), String> {
    let failed = |error: io::Error| format!("{}: {}", start.display(), describe_io(&error));
    // A symbolic link inside the root could lead the walk elsewhere.
    if std::fs::canonicalize(start).is_ok_and(|resolved| !resolved.starts_with(root)) {
        return Err(failed(io::Error::other("the path leads outside the root")));
    }
    let listed = sys::lstat(start).map_err(failed)?;
    if !listed.is_dir() {
        return Err(failed(io::Error::other("not a directory")));
    }
    let directory = traverse::open_path(root, start).map_err(failed)?;
    let stat = directory.stat().map_err(failed)?;
    if (stat.device, stat.inode) != (listed.device, listed.inode) {
        return Err(failed(io::Error::other(
            "the directory was replaced during the scan",
        )));
    }
    Ok((directory, stat))
}

/// The largest matching live files and the recoverable errors met on the
/// way. A root or start directory that cannot be read is a usage error.
///
/// The walk stays on the root's filesystem unless `one_file_system` is false,
/// does not follow symbolic links, and counts each hard-linked inode once.
pub fn largest_files(filter: &LiveFilter) -> Result<(Vec<FileRecord>, Vec<String>)> {
    let root = crate::repository::canonical_root(filter.root);
    let scope = normalize_relative_path(Some(filter.under))?;
    let start = if scope == "." {
        root.clone()
    } else {
        root.join(&scope)
    };
    let root_device = sys::lstat(&root)
        .map_err(|error| usage(format!("{}: {}", root.display(), describe_io(&error))))?
        .device;
    if filter.matcher.matches(&scope, true) {
        // The walk starts inside the scope, so an excluded scope has no files.
        return Ok((Vec::new(), Vec::new()));
    }
    let (directory, stat) = open_start(&root, &start).map_err(usage)?;
    let threads = filter.threads.max(1);
    let pool = traverse::thread_pool(threads)?;
    let state = State {
        filter,
        root: &root,
        root_device,
        budget: OpenBudget::new(threads),
        claims: Claims::new(),
        largest: LargestFiles::new(filter.top),
        hard_links: HardLinks::new(),
        errors: Mutex::new(Vec::new()),
        entries: AtomicU64::new(0),
        directories: AtomicU64::new(0),
        current_path: Mutex::new(String::new()),
    };
    let started = Instant::now();
    {
        let (pool, state) = (&pool, &state);
        let report = filter.progress.map(|progress| {
            move || {
                progress(&LiveProgress {
                    entries: state.entries.load(Ordering::Relaxed),
                    directories: state.directories.load(Ordering::Relaxed),
                    elapsed_seconds: started.elapsed().as_secs_f64(),
                    current_path: lock(&state.current_path).clone(),
                });
            }
        });
        traverse::run_reporting(
            move || {
                pool.install(move || {
                    rayon::scope(move |tasks| list(tasks, state, directory, stat, start, scope));
                });
            },
            report.as_ref().map(|report| report as &(dyn Fn() + Sync)),
        );
    }
    let State {
        largest,
        hard_links,
        errors,
        ..
    } = state;
    // Count each hard-linked inode once, at the path that sorts first.
    for file in hard_links.into_links() {
        if largest.wants(file.allocated) {
            largest.offer(file);
        }
    }
    let records = largest
        .into_sorted()
        .into_iter()
        .map(|file| FileRecord {
            username: sys::username_for_uid(file.uid),
            path: file.path,
            uid: file.uid,
            allocated_bytes: i64::try_from(file.allocated).unwrap_or(i64::MAX),
            apparent_bytes: i64::try_from(file.apparent).unwrap_or(i64::MAX),
            modified_epoch: file.modified,
        })
        .collect();
    let mut errors = errors
        .into_inner()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    errors.sort();
    Ok((records, errors))
}
