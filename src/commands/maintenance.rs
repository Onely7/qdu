//! Compress, verify, and repair a profile's snapshots.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use crate::cli::{CompactArgs, RepairArgs, VerifyArgs};
use crate::commands::{renderer, status_paint, terminal_renderer};
use crate::error::{Error, Result, describe_io};
use crate::locking::ProfileLock;
use crate::model::{ProfileIndex, SnapshotRecord, is_safe_snapshot_filename};
use crate::render::Align;
use crate::report::{Column, Field, Report, Table};
use crate::repository::archive_snapshots;
use crate::storage::{
    INDEX_VERSION, IndexRepository, MaterializedSnapshot, ProfilePaths, decompress_snapshot,
    sha256_file, validate_database,
};
use crate::units::{format_duration, format_size};

fn stored_total(paths: &ProfilePaths) -> u64 {
    fs::read_dir(&paths.snapshots)
        .map(|entries| {
            entries
                .flatten()
                .filter_map(|entry| entry.metadata().ok())
                .map(|metadata| metadata.len())
                .sum()
        })
        .unwrap_or(0)
}

/// Compress eligible snapshots now.
pub fn compact(args: &CompactArgs) -> Result<i32> {
    let mut renderer = terminal_renderer(&args.terminal);
    let paths = ProfilePaths::for_profile(&args.profile.profile)?;
    let index = IndexRepository::new(paths.clone());
    let older_than = args.older_than;
    let compacted = {
        // A dry run only reads, so it neither creates the profile's
        // directories nor waits for the lock.
        let _lock = if args.dry_run {
            None
        } else {
            paths.ensure()?;
            Some(ProfileLock::acquire(
                &paths.lock,
                &format!("compact profile={}", args.profile.profile),
            )?)
        };
        archive_snapshots(
            &paths,
            &index,
            older_than,
            usize::try_from(args.keep_latest).unwrap_or(usize::MAX),
            args.dry_run,
        )?
    };
    if compacted.is_empty() {
        renderer.message(&format!(
            "Nothing to compress: no snapshot is older than {} apart from the {} newest.",
            format_duration(args.older_than),
            args.keep_latest
        ));
    } else if args.dry_run {
        renderer.message(&format!("Would compress {} snapshot(s):", compacted.len()));
    } else {
        renderer.message(&format!(
            "Compressed {} snapshot(s); the profile's snapshots now use {}:",
            compacted.len(),
            format_size(stored_total(&paths))
        ));
    }
    for snapshot_id in &compacted {
        renderer.message(&format!("  {snapshot_id}"));
    }
    Ok(0)
}

fn verify_record(paths: &ProfilePaths, record: &SnapshotRecord) -> Result<()> {
    let digest = sha256_file(&paths.snapshots.join(&record.filename))?;
    if digest != record.sha256 {
        return Err(Error::Verification("SHA-256 mismatch".into()));
    }
    let materialized = MaterializedSnapshot::open(paths, record)?;
    let metadata = validate_database(&materialized.path)?;
    if metadata.get("snapshot_id") != Some(&record.snapshot_id) {
        return Err(Error::Verification("snapshot ID mismatch".into()));
    }
    Ok(())
}

/// Verify snapshot checksums and database integrity.
pub fn verify(args: &VerifyArgs) -> Result<i32> {
    let mut renderer = renderer(&args.output);
    let paths = ProfilePaths::for_profile(&args.profile.profile)?;
    let index = IndexRepository::new(paths.clone());
    let records = if args.all {
        index.load()?.snapshots
    } else {
        vec![index.resolve(&args.selector.snapshot, true)?]
    };
    let mut rows = Vec::new();
    for record in &records {
        match verify_record(&paths, record) {
            Ok(()) => rows.push((record.snapshot_id.clone(), "OK", "verified".to_string())),
            Err(error) => rows.push((record.snapshot_id.clone(), "FAILED", error.to_string())),
        }
    }
    let failed = rows.iter().any(|row| row.1 == "FAILED");
    let mut report = Report::new("verify", Some(&args.profile.profile));
    report.context("verified", !failed);
    report.heading("Snapshot verification");
    let mut table = Table::new(vec![
        Column::new("snapshot_id", "Snapshot", Align::Left),
        Column::new("status", "Status", Align::Left),
        Column::new("detail", "Detail", Align::Left),
    ]);
    for (snapshot, status, detail) in rows {
        table.push(vec![
            Field::text(snapshot),
            Field::text(status).painted(status_paint(status)),
            Field::text(detail),
        ]);
    }
    report.rows(table);
    report.render(&mut renderer);
    if failed {
        renderer.flush();
        return Err(Error::Verification(
            "one or more snapshots failed verification".into(),
        ));
    }
    Ok(0)
}

fn record_from_metadata(
    metadata: &std::collections::HashMap<String, String>,
    file: &Path,
    archived: bool,
) -> Result<SnapshotRecord> {
    let text = |key: &str| -> Result<String> {
        metadata
            .get(key)
            .cloned()
            .ok_or_else(|| Error::Format(format!("missing metadata {key}")))
    };
    let number = |key: &str, default: Option<&str>| -> Result<u64> {
        let value = match metadata.get(key) {
            Some(value) => value.clone(),
            None => default
                .map(str::to_string)
                .ok_or_else(|| Error::Format(format!("missing metadata {key}")))?,
        };
        value
            .trim()
            .parse()
            .map_err(|_| Error::Format(format!("invalid metadata {key}")))
    };
    let filename = file
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    if !is_safe_snapshot_filename(&filename) {
        return Err(Error::Format("unsafe snapshot filename".into()));
    }
    Ok(SnapshotRecord {
        snapshot_id: text("snapshot_id")?,
        filename,
        created_epoch: number("created_epoch", None)? as i64,
        created_at: text("created_at")?,
        root: text("root")?,
        host: text("host")?,
        complete: metadata.get("complete").map(String::as_str) == Some("true"),
        error_count: number("error_count", Some("0"))?,
        denied_count: number("denied_count", Some("0"))?,
        allocated_bytes: number("allocated_bytes", None)?,
        apparent_bytes: number("apparent_bytes", None)?,
        directory_count: number("directory_count", None)?,
        file_count: number("file_count", None)?,
        inode_count: number("inode_count", None)?,
        collect_users: metadata.get("collect_users").map(String::as_str) == Some("true"),
        large_file_limit: number("large_file_limit", Some("0"))?,
        exclude_hash: metadata.get("exclude_hash").cloned().unwrap_or_default(),
        sha256: sha256_file(file)?,
        archived,
        labels: Vec::new(),
    })
}

/// Rebuild a profile index from the valid snapshot files.
pub fn repair(args: &RepairArgs) -> Result<i32> {
    let mut renderer = terminal_renderer(&args.terminal);
    let paths = ProfilePaths::for_profile(&args.profile.profile)?;
    paths.ensure()?;
    let mut actions = Vec::new();
    {
        let _lock = ProfileLock::acquire(
            &paths.lock,
            &format!("repair profile={}", args.profile.profile),
        )?;
        let mut temporary: Vec<PathBuf> = fs::read_dir(&paths.temporary)?
            .flatten()
            .map(|entry| entry.path())
            .collect();
        // Partial archives and indexes left by an interrupted operation.
        temporary.extend(
            fs::read_dir(&paths.snapshots)?
                .flatten()
                .map(|entry| entry.path())
                .filter(|path| path.extension().is_some_and(|extension| extension == "tmp")),
        );
        let index_temporary = paths.index.with_extension("tmp");
        if index_temporary.exists() {
            temporary.push(index_temporary);
        }
        temporary.sort();
        for path in temporary {
            let name = path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default();
            // qdu writes only files there; anything else is someone's own.
            if fs::symlink_metadata(&path).is_ok_and(|metadata| !metadata.is_file()) {
                actions.push(format!("leave {name}: not a file that qdu wrote"));
                continue;
            }
            if args.dry_run {
                actions.push(format!("remove temporary file: {name}"));
                continue;
            }
            match fs::remove_file(&path) {
                Ok(()) => actions.push(format!("remove temporary file: {name}")),
                Err(error) => actions.push(format!(
                    "cannot remove temporary file {name}: {}",
                    describe_io(&error)
                )),
            }
        }
        let mut files: Vec<PathBuf> = fs::read_dir(&paths.snapshots)?
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| {
                path.file_name()
                    .map(|name| name.to_string_lossy().contains(".sqlite3"))
                    .unwrap_or(false)
                    && path.extension().is_none_or(|extension| extension != "tmp")
            })
            .collect();
        files.sort();
        let mut records: BTreeMap<String, SnapshotRecord> = BTreeMap::new();
        for path in files {
            let name = path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default();
            let archived = name.ends_with(".gz");
            let loaded = (|| -> Result<SnapshotRecord> {
                let metadata = if archived {
                    let temporary = decompress_snapshot(&path)?;
                    validate_database(&temporary.path)?
                } else {
                    validate_database(&path)?
                };
                record_from_metadata(&metadata, &path, archived)
            })();
            let record = match loaded {
                Ok(record) => record,
                Err(error) => {
                    actions.push(format!("skip invalid snapshot {name}: {error}"));
                    continue;
                }
            };
            if let Some(kept) = records.get(&record.snapshot_id) {
                // An interrupted compaction can leave "<id>.sqlite3" next to
                // "<id>.sqlite3.gz"; the sorted walk sees the uncompressed copy
                // first. Keep it and remove only that redundant archive.
                if record.archived && record.filename == format!("{}.gz", kept.filename) {
                    actions.push(format!("remove duplicate archive: {}", record.filename));
                    if !args.dry_run {
                        let _ = fs::remove_file(paths.snapshots.join(&record.filename));
                    }
                } else {
                    actions.push(format!(
                        "skip duplicate snapshot {}: {} is already indexed",
                        record.filename, record.snapshot_id
                    ));
                }
                continue;
            }
            records.insert(record.snapshot_id.clone(), record);
        }
        // Labels live only in the index; keep those that a readable index
        // still holds.
        if let Ok(previous) = IndexRepository::new(paths.clone()).load() {
            for old in previous.snapshots {
                if let Some(record) = records.get_mut(&old.snapshot_id) {
                    record.labels = old.labels;
                }
            }
        }
        let count = records.len();
        let index = ProfileIndex::empty(INDEX_VERSION, &args.profile.profile)
            .with_records(records.into_values().collect());
        actions.push(format!("rebuild index with {count} snapshot(s)"));
        if !args.dry_run {
            IndexRepository::new(paths.clone()).save(&index)?;
        }
    }
    let mut heading = format!("Repair — profile: {}", args.profile.profile);
    if args.dry_run {
        heading.push_str(" (dry run; nothing was changed)");
    }
    renderer.heading(&heading);
    for action in &actions {
        renderer.message(&format!("- {action}"));
    }
    Ok(0)
}
