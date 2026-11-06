//! Label a snapshot, or remove a label.

use crate::cli::LabelArgs;
use crate::commands::terminal_renderer;
use crate::error::{Result, usage};
use crate::locking::ProfileLock;
use crate::storage::{IndexRepository, ProfilePaths};

/// Give the selected snapshot a label, moving it from the snapshot that had
/// it, or remove the label with `--remove`.
pub fn label(args: &LabelArgs) -> Result<i32> {
    let mut renderer = terminal_renderer(&args.terminal);
    let paths = ProfilePaths::for_profile(&args.profile.profile)?;
    let index = IndexRepository::new(paths.clone());
    let _lock = ProfileLock::acquire(
        &paths.lock,
        &format!("label {} profile={}", args.name, args.profile.profile),
    )?;
    let current = index.load()?;
    let holder = current
        .snapshots
        .iter()
        .find(|record| record.labels.contains(&args.name))
        .map(|record| record.snapshot_id.clone());
    if args.remove {
        let Some(holder) = holder else {
            return Err(usage(format!("no snapshot is labeled {}", args.name)));
        };
        let mut records = current.snapshots.clone();
        for record in &mut records {
            record.labels.retain(|label| *label != args.name);
        }
        index.save(&current.with_records(records))?;
        renderer.message(&format!("Removed label {} from {holder}", args.name));
        return Ok(0);
    }
    let target = index.resolve(&args.selector.snapshot, true)?;
    let mut records = current.snapshots.clone();
    for record in &mut records {
        record.labels.retain(|label| *label != args.name);
        if record.snapshot_id == target.snapshot_id {
            record.labels.push(args.name.clone());
        }
    }
    index.save(&current.with_records(records))?;
    let mut message = format!("Labeled {} as {}", target.snapshot_id, args.name);
    if let Some(holder) = holder.filter(|holder| *holder != target.snapshot_id) {
        message.push_str(&format!(" (moved from {holder})"));
    }
    renderer.message(&message);
    Ok(0)
}
