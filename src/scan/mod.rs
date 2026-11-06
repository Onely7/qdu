//! Walk a directory tree in parallel and store its usage in a snapshot.
//!
//! A scan runs in four steps:
//!
//! 1. [`walk`] reads every directory on a thread pool. A directory gets an
//!    increasing ID when its parent finds it, so a parent's ID is always
//!    smaller than its children's, and becomes a [`Node`] with the usage of
//!    its own entries.
//! 2. [`tree::Tree`] places the nodes by ID, charges each hard-linked file to
//!    the link whose path sorts first, and adds every subtree into its
//!    parent in one pass from the highest ID down.
//! 3. The capacity of the filesystems that the walk entered is measured.
//! 4. [`write`] stores the rows in the order of their paths.
//!
//! Memory grows with the number of directories, about 200 bytes each, and
//! with the number of hard-linked files rather than the number of their
//! links. Paths are built only while the rows are written.

mod tree;
mod walk;
mod write;

use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::Instant;

use rusqlite::Connection;

use self::walk::Filesystem;
use crate::error::{Result, describe_io, usage};
use crate::patterns::PathMatcher;
use crate::sys::{self, Directory, FilesystemUsage, Stat, storable_text};
use crate::traverse::{AtPath, FileEntry};

const MAX_RETAINED_ERRORS: usize = 100;
const MAX_RETAINED_SKIPPED_FILESYSTEMS: usize = 100;

/// Settings for one scan.
pub struct ScanOptions<'a> {
    pub root: &'a Path,
    pub matcher: &'a PathMatcher,
    pub collect_users: bool,
    pub user_max_depth: u32,
    pub large_file_limit: u64,
    pub record_max_depth: Option<u32>,
    pub one_file_system: bool,
    pub threads: usize,
}

/// Which part of the work a progress report describes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Scanning,
    Writing,
}

/// Counters reported periodically while a scan runs.
#[derive(Clone, Debug)]
pub struct ScanProgress {
    pub phase: Phase,
    pub entries: u64,
    pub directories: u64,
    pub allocated_bytes: u64,
    pub errors: u64,
    pub elapsed_seconds: f64,
    pub current_path: String,
}

/// A recoverable failure recorded during the scan.
#[derive(Clone, Debug)]
pub struct ScanError {
    pub path: String,
    pub operation: &'static str,
    pub message: String,
    /// Permission was denied, which leaves the snapshot complete.
    pub denied: bool,
}

impl ScanError {
    /// How the failure is stored: `denied` or `error`.
    pub fn kind(&self) -> &'static str {
        if self.denied { "denied" } else { "error" }
    }
}

/// Aggregated totals for a directory tree.
#[derive(Clone, Copy, Debug, Default)]
pub struct Totals {
    pub allocated_bytes: u64,
    pub apparent_bytes: u64,
    pub file_count: u64,
    pub directory_count: u64,
    pub inode_count: u64,
    pub latest_modified_epoch: f64,
}

impl Totals {
    /// Add a subtree. Sizes saturate instead of wrapping: sparse files can
    /// claim apparent sizes near the 64-bit limit.
    fn add(&mut self, other: &Totals) {
        self.allocated_bytes = self.allocated_bytes.saturating_add(other.allocated_bytes);
        self.apparent_bytes = self.apparent_bytes.saturating_add(other.apparent_bytes);
        self.file_count = self.file_count.saturating_add(other.file_count);
        self.directory_count = self.directory_count.saturating_add(other.directory_count);
        self.inode_count = self.inode_count.saturating_add(other.inode_count);
        self.latest_modified_epoch = self.latest_modified_epoch.max(other.latest_modified_epoch);
    }

    /// One file's contribution.
    fn file(allocated: u64, apparent: u64, modified: f64) -> Self {
        Totals {
            allocated_bytes: allocated,
            apparent_bytes: apparent,
            file_count: 1,
            directory_count: 0,
            inode_count: 1,
            latest_modified_epoch: modified,
        }
    }

    /// A directory's own contribution, without its entries.
    fn directory(stat: &Stat) -> Self {
        Totals {
            allocated_bytes: stat.allocated(),
            apparent_bytes: stat.size,
            file_count: 0,
            directory_count: 1,
            inode_count: 1,
            latest_modified_epoch: stat.modified,
        }
    }
}

/// What a scan found, used to finish the snapshot's metadata.
pub struct ScanSummary {
    pub root: PathBuf,
    /// No failure other than a denied permission.
    pub complete: bool,
    /// The first failures of each kind by path; the snapshot stores them all.
    pub errors: Vec<ScanError>,
    pub error_count: u64,
    pub denied_count: u64,
    pub duration_seconds: f64,
    pub totals: Totals,
    pub filesystem: FilesystemUsage,
    pub cross_filesystems: bool,
    pub skipped_filesystem_count: u64,
    pub skipped_filesystems: Vec<String>,
    pub filesystem_paths: Vec<String>,
    /// Automount points entered, whose unmounted entries the scan missed.
    pub automount_paths: Vec<String>,
    pub stored_directory_count: u64,
}

/// Usage by one owner.
#[derive(Clone, Copy, Debug, Default)]
struct OwnerUsage {
    allocated: u64,
    apparent: u64,
    files: u64,
    directories: u64,
    inodes: u64,
}

impl OwnerUsage {
    fn file(allocated: u64, apparent: u64) -> Self {
        OwnerUsage {
            allocated,
            apparent,
            files: 1,
            directories: 0,
            inodes: 1,
        }
    }

    fn directory(allocated: u64, apparent: u64) -> Self {
        OwnerUsage {
            allocated,
            apparent,
            files: 0,
            directories: 1,
            inodes: 1,
        }
    }

    fn add(&mut self, other: &OwnerUsage) {
        self.allocated = self.allocated.saturating_add(other.allocated);
        self.apparent = self.apparent.saturating_add(other.apparent);
        self.files = self.files.saturating_add(other.files);
        self.directories = self.directories.saturating_add(other.directories);
        self.inodes = self.inodes.saturating_add(other.inodes);
    }
}

/// Usage by owner, sized exactly: most directories have one owner, and a
/// growable list would reserve room for four.
#[derive(Clone, Debug, Default)]
struct Owners(Box<[(u32, OwnerUsage)]>);

impl Owners {
    fn add(&mut self, uid: u32, usage: &OwnerUsage) {
        if let Some((_, total)) = self.0.iter_mut().find(|(owner, _)| *owner == uid) {
            total.add(usage);
            return;
        }
        // Grow by one entry: shrinking a larger allocation later would
        // fragment the heap.
        let mut grown = std::mem::take(&mut self.0).into_vec();
        grown.reserve_exact(1);
        grown.push((uid, *usage));
        self.0 = grown.into_boxed_slice();
    }

    fn merge(&mut self, other: &Owners) {
        for (uid, usage) in &other.0 {
            self.add(*uid, usage);
        }
    }

    fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    fn get(&self, uid: u32) -> Option<&OwnerUsage> {
        self.0
            .iter()
            .find(|(owner, _)| *owner == uid)
            .map(|(_, usage)| usage)
    }

    fn iter(&self) -> impl Iterator<Item = &(u32, OwnerUsage)> {
        self.0.iter()
    }
}

/// One scanned directory; its totals become subtree totals after the walk.
struct Node {
    id: u32,
    parent: u32,
    depth: u32,
    name: Box<str>,
    totals: Totals,
    owners: Owners,
}

/// A file with more than one hard link, counted once at the link whose path
/// sorts first.
struct HardLink {
    /// The ID of the directory that holds this link.
    directory: u32,
    file: FileEntry,
}

impl AtPath for HardLink {
    fn path(&self) -> &str {
        &self.file.path
    }
}

/// The canonical root, open, and its status, or a usage error that explains
/// why it cannot be scanned.
fn open_root(root: &Path) -> Result<(PathBuf, Directory, Stat)> {
    if fs::symlink_metadata(root).is_err() {
        return Err(usage(format!(
            "scan root does not exist: {}",
            root.display()
        )));
    }
    let root = fs::canonicalize(root).map_err(|error| {
        usage(format!(
            "cannot resolve scan root {}: {}",
            root.display(),
            describe_io(&error)
        ))
    })?;
    if root.to_str().is_none() {
        return Err(usage(format!(
            "scan root is not valid UTF-8: {}",
            storable_text(root.as_os_str())
        )));
    }
    // The canonical root has no symbolic links left to follow.
    let stat = sys::lstat(&root).map_err(|error| {
        usage(format!(
            "cannot stat scan root {}: {}",
            root.display(),
            describe_io(&error)
        ))
    })?;
    if !stat.is_dir() {
        return Err(usage(format!(
            "scan root is not a directory: {}",
            root.display()
        )));
    }
    let cannot_read = |error: io::Error| {
        usage(format!(
            "cannot read scan root {}: {}",
            root.display(),
            describe_io(&error)
        ))
    };
    let directory = Directory::open(&root).map_err(cannot_read)?;
    // Opening an automount point mounts it, so the status that counts is
    // the one of the open directory.
    let stat = directory.stat().map_err(cannot_read)?;
    Ok((root, directory, stat))
}

/// Total capacity of the filesystems the walk entered and the paths where
/// it entered them, the root's first, with a recorded error for each
/// filesystem that could not be measured.
/// The summed capacity of the filesystems entered, their paths, and the
/// paths of the automount points among them.
fn total_capacity(
    filesystems: HashMap<u64, Filesystem>,
    errors: &mut Vec<ScanError>,
    error_count: &mut u64,
) -> (FilesystemUsage, Vec<String>, Vec<String>) {
    let mut filesystems: Vec<Filesystem> = filesystems.into_values().collect();
    filesystems.sort_by(|left, right| {
        (left.relative != ".", &left.relative).cmp(&(right.relative != ".", &right.relative))
    });
    let mut total = FilesystemUsage::default();
    let mut paths = Vec::with_capacity(filesystems.len());
    let mut automounts = Vec::new();
    for filesystem in filesystems {
        if filesystem.automount {
            automounts.push(filesystem.relative.clone());
        }
        match filesystem.usage {
            Ok(usage) => {
                total.total_bytes = total.total_bytes.saturating_add(usage.total_bytes);
                total.available_bytes = total.available_bytes.saturating_add(usage.available_bytes);
                total.free_bytes = total.free_bytes.saturating_add(usage.free_bytes);
                total.total_inodes = total.total_inodes.saturating_add(usage.total_inodes);
                total.available_inodes = total
                    .available_inodes
                    .saturating_add(usage.available_inodes);
            }
            Err(error) => {
                *error_count += 1;
                errors.push(ScanError {
                    path: filesystem.relative.clone(),
                    operation: "statvfs",
                    message: describe_io(&error),
                    denied: false,
                });
            }
        }
        paths.push(filesystem.relative);
    }
    (total, paths, automounts)
}

/// The first `MAX_RETAINED_ERRORS` failures of each kind, in order.
fn first_of_each_kind(errors: Vec<ScanError>) -> Vec<ScanError> {
    let (mut kept_errors, mut kept_denied) = (0, 0);
    errors
        .into_iter()
        .filter(|error| {
            let kept = if error.denied {
                &mut kept_denied
            } else {
                &mut kept_errors
            };
            *kept += 1;
            *kept <= MAX_RETAINED_ERRORS
        })
        .collect()
}

/// Scan a root and store the snapshot tables in an open database.
pub fn scan(
    connection: &mut Connection,
    options: &ScanOptions,
    progress: Option<&(dyn Fn(&ScanProgress) + Sync)>,
) -> Result<ScanSummary> {
    let started = Instant::now();
    let (root, root_directory, root_stat) = open_root(options.root)?;
    let walked = walk::walk(options, &root, root_directory, root_stat, progress, started)?;
    let mut tree = tree::Tree::from_nodes(walked.nodes, walked.ids)
        .ok_or_else(|| usage(format!("cannot read scan root: {}", root.display())))?;
    let large_files = walked.large_files;
    tree.count_hard_links(walked.hard_links, &large_files, options.collect_users);
    tree.roll_up(options.collect_users, options.user_max_depth);
    let totals = tree.root().totals;

    let mut errors = walked.errors;
    let mut error_count = walked.error_count;
    let (filesystem, filesystem_paths, automount_paths) =
        total_capacity(walked.filesystems, &mut errors, &mut error_count);
    errors.sort_by(|left, right| (&left.path, left.operation).cmp(&(&right.path, right.operation)));
    let mut skipped = walked.skipped;
    skipped.sort();
    let skipped_filesystem_count = skipped.len() as u64;
    skipped.truncate(MAX_RETAINED_SKIPPED_FILESYSTEMS);

    let stored_directory_count = write::write_tables(
        connection,
        options,
        &tree,
        large_files.into_sorted(),
        &errors,
    )?;
    Ok(ScanSummary {
        root,
        complete: error_count == 0,
        errors: first_of_each_kind(errors),
        error_count,
        denied_count: walked.denied_count,
        duration_seconds: started.elapsed().as_secs_f64(),
        totals,
        filesystem,
        cross_filesystems: !options.one_file_system,
        skipped_filesystem_count,
        skipped_filesystems: skipped,
        filesystem_paths,
        automount_paths,
        stored_directory_count,
    })
}
