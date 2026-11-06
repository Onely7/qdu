//! List a profile's snapshots and their disk use.

use std::collections::HashMap;

use crate::cli::ListArgs;
use crate::commands::{format_stored, renderer};
use crate::error::Result;
use crate::model::SnapshotRecord;
use crate::render::Align;
use crate::report::{Column, Field, Report, Table};
use crate::storage::{IndexRepository, ProfilePaths};
use crate::units::{format_local_time, format_size, group};

/// List the snapshots of a profile with their disk space.
pub fn list(args: &ListArgs) -> Result<i32> {
    let mut renderer = renderer(&args.output);
    let paths = ProfilePaths::for_profile(&args.profile.profile)?;
    let index = IndexRepository::new(paths.clone()).load()?;
    let stored: HashMap<&str, Option<u64>> = index
        .snapshots
        .iter()
        .map(|record| (record.snapshot_id.as_str(), paths.stored_bytes(record)))
        .collect();
    let stored_total: u64 = stored.values().flatten().sum();
    let records: Vec<&SnapshotRecord> = index.snapshots.iter().rev().take(args.top).collect();

    let mut report = Report::new("list", Some(&args.profile.profile));
    report.context("latest_complete", index.latest_complete.clone());
    report.context("latest_any", index.latest_any.clone());
    report.context("stored_bytes_total", stored_total);
    report.heading(format!("Snapshots — profile: {}", args.profile.profile));
    // Snapshot IDs are copied into other commands, so they come last and
    // are never shortened.
    let mut table = Table::new(vec![
        Column::new("created_at", "Created", Align::Left),
        Column::data("created_epoch"),
        Column::new("status", "Status", Align::Left),
        Column::data("error_count"),
        Column::data("denied_count"),
        Column::new("allocated_bytes", "Size", Align::Right),
        Column::data("apparent_bytes"),
        Column::new("file_count", "Files", Align::Right),
        Column::data("directory_count"),
        Column::data("inode_count"),
        Column::new("archived", "Archive", Align::Center),
        Column::new("stored_bytes", "Stored", Align::Right),
        Column::new("labels", "Labels", Align::Left),
        Column::data("collect_users"),
        Column::data("large_file_limit"),
        Column::data("exclude_hash"),
        Column::data("root"),
        Column::data("host"),
        Column::data("filename"),
        Column::data("sha256"),
        Column::new("snapshot_id", "Snapshot", Align::Left),
    ]);
    for record in records {
        let stored_bytes = stored[record.snapshot_id.as_str()];
        let status = if record.complete {
            "complete"
        } else {
            "incomplete"
        };
        table.push(vec![
            Field::new(
                record.created_at.clone(),
                format_local_time(record.created_epoch as f64),
            ),
            Field::data(record.created_epoch),
            Field::text(status),
            Field::data(record.error_count),
            Field::data(record.denied_count),
            Field::new(record.allocated_bytes, format_size(record.allocated_bytes)),
            Field::data(record.apparent_bytes),
            Field::new(record.file_count, group(i128::from(record.file_count))),
            Field::data(record.directory_count),
            Field::data(record.inode_count),
            Field::new(record.archived, if record.archived { "yes" } else { "no" }),
            Field::new(stored_bytes, format_stored(stored_bytes)),
            Field::new(record.labels.clone(), record.labels.join(", ")),
            Field::data(record.collect_users),
            Field::data(record.large_file_limit),
            Field::data(record.exclude_hash.clone()),
            Field::data(record.root.clone()),
            Field::data(record.host.clone()),
            Field::data(record.filename.clone()),
            Field::data(record.sha256.clone()),
            Field::text(record.snapshot_id.clone()),
        ]);
    }
    report.rows(table);
    report.note(format!(
        "{} snapshot(s) use {} in {}",
        group(index.snapshots.len() as i128),
        format_size(stored_total),
        paths.snapshots.display()
    ));
    report.render(&mut renderer);
    Ok(0)
}
