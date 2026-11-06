//! Create snapshots and apply retention and compaction.

use std::fs;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

use crate::config::ProfileConfig;
use crate::error::{PathContext, Result, usage};
use crate::locking::ProfileLock;
use crate::model::SnapshotRecord;
use crate::patterns::PathMatcher;
use crate::scan::{self, ScanOptions, ScanProgress, ScanSummary};
use crate::signals::Cleanup;
use crate::storage::{
    IndexRepository, ProfilePaths, create_snapshot_database, fsync_directory, gzip_snapshot,
    sha256_file, unique_token, write_metadata,
};
use crate::sys;
use crate::units::format_iso_local;

/// The newest snapshots stay uncompressed so that `show` and `diff` read them
/// directly.
pub const AUTOMATIC_KEEP_UNCOMPRESSED: usize = 2;

/// A published snapshot and the retention work done after it.
pub struct SnapshotOutcome {
    pub record: SnapshotRecord,
    pub summary: ScanSummary,
    pub removed: Vec<String>,
    pub archived: Vec<String>,
}

/// The number of scanning threads: `--threads`, else `QDU_THREADS`, else the
/// CPU count up to 16.
pub fn scan_threads(requested: Option<usize>) -> usize {
    if let Some(threads) = requested {
        return threads;
    }
    if let Ok(value) = std::env::var("QDU_THREADS") {
        if let Ok(threads) = value.trim().parse::<usize>() {
            if threads > 0 {
                return threads;
            }
        }
    }
    std::thread::available_parallelism()
        .map(|threads| threads.get())
        .unwrap_or(4)
        .clamp(1, 16)
}

/// The absolute, symlink-free form of a root, or its absolute form if missing.
pub fn canonical_root(root: &Path) -> PathBuf {
    fs::canonicalize(root).unwrap_or_else(|_| {
        if root.is_absolute() {
            root.to_path_buf()
        } else {
            std::env::current_dir()
                .map(|directory| directory.join(root))
                .unwrap_or_else(|_| root.to_path_buf())
        }
    })
}

fn snapshot_id() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.subsec_nanos())
        .unwrap_or(0);
    format!(
        "{}-{nanos:09}-{}",
        sys::utc_stamp(sys::now_epoch()),
        unique_token(0)
    )
}

/// Scan the profile root, publish a snapshot, and apply retention.
///
/// The temporary database is removed after any fatal failure. Recoverable
/// scan errors produce an indexed incomplete snapshot.
pub fn create_snapshot(
    profile: &ProfileConfig,
    labels: &[String],
    threads: Option<usize>,
    progress: Option<&(dyn Fn(&ScanProgress) + Sync)>,
) -> Result<SnapshotOutcome> {
    let paths = ProfilePaths::for_profile(&profile.name)?;
    let index = IndexRepository::new(paths.clone());
    paths.ensure()?;
    let command = format!(
        "snapshot profile={} root={}",
        profile.name,
        profile.root.display()
    );
    let _lock = ProfileLock::acquire(&paths.lock, &command)?;
    let existing = index.load()?;
    let root = canonical_root(&profile.root);
    if let Some(recorded) = existing.recorded_root() {
        if Path::new(recorded) != root {
            return Err(usage(format!(
                "profile '{}' holds snapshots of {}, not {}; use --path {} or another profile",
                profile.name,
                recorded,
                root.display(),
                shell_quote(recorded)
            )));
        }
    }
    let matcher = PathMatcher::new(&profile.excludes)?;
    let id = snapshot_id();
    let final_path = paths.snapshots.join(format!("{id}.sqlite3"));
    // Removed on failure or interruption; kept once renamed into place. The
    // empty file is registered before SQLite writes to it, so a failure
    // while the schema is created removes it as well.
    let temporary_path = paths.temporary.join(format!("{id}.sqlite3.tmp"));
    let (temporary, _) = Cleanup::create(&temporary_path, |path| {
        fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(path)
    })
    .at(&temporary_path)?;
    let mut connection = create_snapshot_database(temporary.path())?;
    let options = ScanOptions {
        root: &root,
        matcher: &matcher,
        collect_users: profile.collect_users,
        user_max_depth: profile.user_max_depth,
        large_file_limit: profile.large_file_limit,
        record_max_depth: profile.record_max_depth,
        one_file_system: !profile.cross_filesystems,
        threads: scan_threads(threads),
    };
    let summary = scan::scan(&mut connection, &options, progress)?;
    let created_epoch = sys::now_epoch();
    finalize_database(&connection, &id, created_epoch, &summary, profile, &matcher)?;
    drop(connection);
    // The database is written without a journal or syncs; flush it before
    // the index refers to it, so that a crash cannot publish an empty file.
    fs::File::open(temporary.path())
        .and_then(|file| file.sync_all())
        .at(temporary.path())?;
    fs::rename(temporary.path(), &final_path).at(&final_path)?;
    temporary.keep();
    fsync_directory(&paths.snapshots);
    let record = SnapshotRecord {
        snapshot_id: id.clone(),
        filename: format!("{id}.sqlite3"),
        created_epoch,
        created_at: format_iso_local(created_epoch as f64),
        root: summary.root.display().to_string(),
        host: sys::hostname(),
        complete: summary.complete,
        error_count: summary.error_count,
        denied_count: summary.denied_count,
        allocated_bytes: summary.totals.allocated_bytes,
        apparent_bytes: summary.totals.apparent_bytes,
        directory_count: summary.totals.directory_count,
        file_count: summary.totals.file_count,
        inode_count: summary.totals.inode_count,
        collect_users: profile.collect_users,
        large_file_limit: profile.large_file_limit,
        exclude_hash: matcher.digest(),
        sha256: sha256_file(&final_path)?,
        archived: false,
        labels: labels.to_vec(),
    };
    index.add(record.clone())?;
    let removed = prune(&paths, &index, profile.keep_snapshots)?;
    let archived = match profile.compact_after_seconds {
        Some(older_than) => archive_snapshots(
            &paths,
            &index,
            older_than,
            AUTOMATIC_KEEP_UNCOMPRESSED,
            false,
        )?,
        None => Vec::new(),
    };
    Ok(SnapshotOutcome {
        record,
        summary,
        removed,
        archived,
    })
}

fn finalize_database(
    connection: &rusqlite::Connection,
    snapshot_id: &str,
    created_epoch: i64,
    summary: &ScanSummary,
    profile: &ProfileConfig,
    matcher: &PathMatcher,
) -> Result<()> {
    let flag = |value: bool| if value { "true" } else { "false" }.to_string();
    let totals = summary.totals;
    let metadata: Vec<(&str, String)> = vec![
        ("snapshot_id", snapshot_id.to_string()),
        ("created_epoch", created_epoch.to_string()),
        ("created_at", format_iso_local(created_epoch as f64)),
        ("host", sys::hostname()),
        ("root", summary.root.display().to_string()),
        ("complete", flag(summary.complete)),
        ("error_count", summary.error_count.to_string()),
        ("denied_count", summary.denied_count.to_string()),
        (
            "duration_seconds",
            format!("{:.6}", summary.duration_seconds),
        ),
        ("allocated_bytes", totals.allocated_bytes.to_string()),
        ("apparent_bytes", totals.apparent_bytes.to_string()),
        ("directory_count", totals.directory_count.to_string()),
        ("file_count", totals.file_count.to_string()),
        ("inode_count", totals.inode_count.to_string()),
        ("collect_users", flag(profile.collect_users)),
        ("large_file_limit", profile.large_file_limit.to_string()),
        (
            "record_max_depth",
            profile
                .record_max_depth
                .map(|depth| depth.to_string())
                .unwrap_or_default(),
        ),
        ("user_max_depth", profile.user_max_depth.to_string()),
        ("exclude_patterns", matcher.patterns().join("\n")),
        ("exclude_hash", matcher.digest()),
        (
            "filesystem_total_bytes",
            summary.filesystem.total_bytes.to_string(),
        ),
        (
            "filesystem_available_bytes",
            summary.filesystem.available_bytes.to_string(),
        ),
        (
            "filesystem_free_bytes",
            summary.filesystem.free_bytes.to_string(),
        ),
        (
            "filesystem_total_inodes",
            summary.filesystem.total_inodes.to_string(),
        ),
        (
            "filesystem_available_inodes",
            summary.filesystem.available_inodes.to_string(),
        ),
        ("cross_filesystems", flag(summary.cross_filesystems)),
        (
            "skipped_filesystem_count",
            summary.skipped_filesystem_count.to_string(),
        ),
        (
            "skipped_filesystems",
            summary.skipped_filesystems.join("\n"),
        ),
        ("filesystem_paths", summary.filesystem_paths.join("\n")),
        ("automount_paths", summary.automount_paths.join("\n")),
        (
            "filesystem_count",
            summary.filesystem_paths.len().to_string(),
        ),
        (
            "stored_directory_count",
            summary.stored_directory_count.to_string(),
        ),
    ];
    write_metadata(connection, &metadata)
}

/// Quote a string for a POSIX shell when it needs quoting.
pub fn shell_quote(value: &str) -> String {
    let safe = !value.is_empty()
        && value
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || "@%+=:,./-_".contains(character));
    if safe {
        value.to_string()
    } else {
        format!("'{}'", value.replace('\'', "'\"'\"'"))
    }
}

/// Remove the oldest unlabeled snapshots until at most `keep` unlabeled
/// ones remain; 0 keeps all. Labeled snapshots are always kept.
pub fn prune(paths: &ProfilePaths, index: &IndexRepository, keep: u64) -> Result<Vec<String>> {
    if keep == 0 {
        return Ok(Vec::new());
    }
    let current = index.load()?;
    let keep = usize::try_from(keep).unwrap_or(usize::MAX);
    let unlabeled: Vec<&SnapshotRecord> = current
        .snapshots
        .iter()
        .filter(|record| record.labels.is_empty())
        .collect();
    if unlabeled.len() <= keep {
        return Ok(Vec::new());
    }
    let removable: Vec<SnapshotRecord> = unlabeled[..unlabeled.len() - keep]
        .iter()
        .map(|record| (*record).clone())
        .collect();
    let ids: Vec<String> = removable
        .iter()
        .map(|record| record.snapshot_id.clone())
        .collect();
    // Publish the shorter index first: an interruption then leaves only
    // unreferenced files, never an index entry without its file.
    index.remove(&ids)?;
    for record in &removable {
        let _ = fs::remove_file(paths.snapshots.join(&record.filename));
    }
    Ok(ids)
}

/// Gzip snapshots older than a cutoff and return the archived IDs.
///
/// The newest `keep_latest` snapshots and the `latest` and `latest-any`
/// selectors stay uncompressed. Archives are written before the index points
/// at them, and sources are deleted only after the index is saved, so an
/// interruption never leaves an index entry without its file. The caller
/// must hold the profile lock.
pub fn archive_snapshots(
    paths: &ProfilePaths,
    index: &IndexRepository,
    older_than_seconds: u64,
    keep_latest: usize,
    dry_run: bool,
) -> Result<Vec<String>> {
    let cutoff = sys::now() - older_than_seconds as f64;
    let current = index.load()?;
    let mut protected: Vec<&str> = current
        .snapshots
        .iter()
        .rev()
        .take(keep_latest)
        .map(|record| record.snapshot_id.as_str())
        .collect();
    protected.extend(current.latest_complete.as_deref());
    protected.extend(current.latest_any.as_deref());
    let mut updated = Vec::with_capacity(current.snapshots.len());
    let mut archived = Vec::new();
    let mut sources = Vec::new();
    for record in &current.snapshots {
        let source = paths.snapshots.join(&record.filename);
        if record.archived {
            // Finish an earlier run that stopped after saving the index.
            if !dry_run && source.exists() {
                let _ = fs::remove_file(source.with_extension(""));
            }
            updated.push(record.clone());
            continue;
        }
        if record.created_epoch as f64 > cutoff || protected.contains(&record.snapshot_id.as_str())
        {
            updated.push(record.clone());
            continue;
        }
        archived.push(record.snapshot_id.clone());
        if dry_run {
            updated.push(record.clone());
            continue;
        }
        let destination = paths.snapshots.join(format!("{}.gz", record.filename));
        gzip_snapshot(&source, &destination)?;
        let mut archived_record = record.clone();
        archived_record.filename = format!("{}.gz", record.filename);
        archived_record.sha256 = sha256_file(&destination)?;
        archived_record.archived = true;
        updated.push(archived_record);
        sources.push(source);
    }
    if !dry_run && !archived.is_empty() {
        index.save(&current.with_records(updated))?;
        for source in sources {
            let _ = fs::remove_file(source);
        }
    }
    Ok(archived)
}
