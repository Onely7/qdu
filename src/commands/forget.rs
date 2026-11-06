//! Remove chosen snapshots from a profile.

use crate::cli::ForgetArgs;
use crate::commands::terminal_renderer;
use crate::error::{Result, usage};
use crate::locking::ProfileLock;
use crate::model::SnapshotRecord;
use crate::storage::{IndexRepository, ProfilePaths};
use crate::units::format_local_time;

/// Remove the selected snapshots and their files.
pub fn forget(args: &ForgetArgs) -> Result<i32> {
    let mut renderer = terminal_renderer(&args.terminal);
    let paths = ProfilePaths::for_profile(&args.profile.profile)?;
    let index = IndexRepository::new(paths.clone());
    let _lock = ProfileLock::acquire(
        &paths.lock,
        &format!("forget profile={}", args.profile.profile),
    )?;
    let current = index.load()?;
    let mut chosen: Vec<SnapshotRecord> = Vec::new();
    for selector in &args.snapshots {
        let record = selector.select(&current, true).map_err(usage)?;
        if !chosen
            .iter()
            .any(|item| item.snapshot_id == record.snapshot_id)
        {
            chosen.push(record);
        }
    }
    let describe = |record: &SnapshotRecord| {
        format!(
            "{}  {}",
            format_local_time(record.created_epoch as f64),
            record.snapshot_id
        )
    };
    if args.dry_run {
        renderer.heading("Would remove (dry run; nothing was changed)");
        for record in &chosen {
            renderer.message(&format!("- {}", describe(record)));
        }
        return Ok(0);
    }
    let ids: Vec<String> = chosen
        .iter()
        .map(|record| record.snapshot_id.clone())
        .collect();
    // Publish the shorter index first: an interruption then leaves only
    // unreferenced files, never an index entry without its file.
    index.remove(&ids)?;
    for record in &chosen {
        let _ = std::fs::remove_file(paths.snapshots.join(&record.filename));
        renderer.message(&format!("Removed {}", describe(record)));
    }
    Ok(0)
}
