//! Diagnose a profile's configuration, storage, schedule, and lock.

use std::fs;
use std::path::Path;

use crate::cli::DoctorArgs;
use crate::commands::schedule::scheduled_entry;
use crate::commands::{effective_profile, renderer, status_paint};
use crate::config::ConfigRepository;
use crate::error::{Result, describe_io, exit};
use crate::locking::{describe_holder, inspect_lock};
use crate::render::{Align, Renderer};
use crate::report::{Column, Field, Report, Table};
use crate::repository::canonical_root;
use crate::storage::{IndexRepository, ProfilePaths};
use crate::sys;
use crate::units::{format_age, format_local_time, format_size};

/// One environment check reported by `doctor`.
struct DoctorItem {
    name: &'static str,
    status: &'static str,
    detail: String,
}

impl DoctorItem {
    fn new(name: &'static str, status: &'static str, detail: impl Into<String>) -> Self {
        DoctorItem {
            name,
            status,
            detail: detail.into(),
        }
    }
}

fn snapshot_items(paths: &ProfilePaths, root: &Path) -> Vec<DoctorItem> {
    if !paths.index.exists() {
        return vec![DoctorItem::new(
            "Index",
            "INFO",
            "no snapshots yet; run 'qdu snapshot'",
        )];
    }
    let index = match IndexRepository::new(paths.clone()).load() {
        Ok(index) => index,
        Err(error) => {
            return vec![DoctorItem::new(
                "Index",
                "FAILED",
                format!("{error}; run 'qdu repair'"),
            )];
        }
    };
    let mut stored = 0u64;
    let mut missing = 0u64;
    for record in &index.snapshots {
        match paths.stored_bytes(record) {
            Some(size) => stored += size,
            None => missing += 1,
        }
    }
    let mut detail = format!(
        "{} snapshot(s) use {}",
        crate::units::group(index.snapshots.len() as i128),
        format_size(stored)
    );
    if missing > 0 {
        detail.push_str(&format!("; {missing} file(s) missing, run 'qdu repair'"));
    }
    let mut items = vec![DoctorItem::new(
        "Index",
        if missing > 0 { "FAILED" } else { "OK" },
        detail,
    )];
    let Some(newest) = index.snapshots.last() else {
        return items;
    };
    let age = format_age(sys::now_epoch() - newest.created_epoch);
    let state = if newest.complete {
        "complete".to_string()
    } else {
        format!(
            "incomplete with {} error(s); see 'qdu errors'",
            crate::units::group(i128::from(newest.error_count))
        )
    };
    items.push(DoctorItem::new(
        "Latest snapshot",
        if newest.complete { "OK" } else { "WARN" },
        format!(
            "{} ({age} ago), {state}",
            format_local_time(newest.created_epoch as f64)
        ),
    ));
    // A snapshot refuses to run when the root differs from the snapshots' root.
    let root = canonical_root(root);
    if Path::new(&newest.root) == root {
        items.push(DoctorItem::new("Snapshot root", "OK", newest.root.clone()));
    } else {
        items.push(DoctorItem::new(
            "Snapshot root",
            "FAILED",
            format!(
                "snapshots are of {}, but the profile scans {}, so 'qdu snapshot' refuses to run",
                newest.root,
                root.display()
            ),
        ));
    }
    items
}

fn schedule_item(profile: &str) -> DoctorItem {
    match scheduled_entry(profile) {
        Err(error) => DoctorItem::new("Schedule", "INFO", error.to_string()),
        Ok(None) => DoctorItem::new("Schedule", "INFO", "not scheduled; see 'qdu schedule'"),
        Ok(Some(entry)) => {
            let fields: Vec<&str> = entry.split_whitespace().take(5).collect();
            DoctorItem::new("Schedule", "OK", format!("cron {}", fields.join(" ")))
        }
    }
}

/// Whether qdu can write the state directory, which a diagnosis does not
/// create when it is missing.
fn state_item(root: &Path) -> DoctorItem {
    if root.exists() {
        let probe = root.join(".write-test");
        let writable = fs::write(&probe, "ok").and_then(|()| fs::remove_file(&probe));
        return match writable {
            Ok(()) => DoctorItem::new("State directory", "OK", root.display().to_string()),
            Err(error) => DoctorItem::new(
                "State directory",
                "FAILED",
                format!("{}: {}", root.display(), describe_io(&error)),
            ),
        };
    }
    let existing = root.ancestors().find(|ancestor| ancestor.exists());
    match existing {
        Some(ancestor) if sys::writable(ancestor) => DoctorItem::new(
            "State directory",
            "INFO",
            format!("{} is created with the first snapshot", root.display()),
        ),
        Some(ancestor) => DoctorItem::new(
            "State directory",
            "FAILED",
            format!(
                "{} cannot be created: {} is not writable",
                root.display(),
                ancestor.display()
            ),
        ),
        None => DoctorItem::new("State directory", "FAILED", root.display().to_string()),
    }
}

/// Diagnose a profile's configuration, storage, schedule, and lock.
pub fn doctor(args: &DoctorArgs, config: &ConfigRepository) -> Result<i32> {
    let mut renderer = renderer(&args.output);
    let paths = ProfilePaths::for_profile(&args.profile.profile)?;
    // A broken configuration is a finding, not a reason to stop.
    let loaded = effective_profile(config, &args.profile.profile);
    let mut items = vec![
        DoctorItem::new("qdu", "OK", env!("CARGO_PKG_VERSION")),
        DoctorItem::new("SQLite", "OK", rusqlite::version()),
        match &loaded {
            Err(error) => DoctorItem::new("Config", "FAILED", error.to_string()),
            Ok(_) if config.path.exists() => {
                DoctorItem::new("Config", "OK", config.path.display().to_string())
            }
            Ok(_) => DoctorItem::new(
                "Config",
                "INFO",
                format!(
                    "{} not created; built-in defaults apply",
                    config.path.display()
                ),
            ),
        },
    ];
    items.push(state_item(&paths.root));
    let Ok(profile) = loaded else {
        return render(args, &mut renderer, items);
    };
    items.push(match fs::metadata(&profile.root) {
        Ok(metadata) if metadata.is_dir() => {
            let readable = sys::readable(&profile.root);
            DoctorItem::new(
                "Profile root",
                if readable { "OK" } else { "WARN" },
                profile.root.display().to_string(),
            )
        }
        _ => DoctorItem::new(
            "Profile root",
            "FAILED",
            format!("missing: {}", profile.root.display()),
        ),
    });
    items.push(DoctorItem::new(
        "Filesystem traversal",
        "INFO",
        if profile.cross_filesystems {
            "cross-filesystems"
        } else {
            "one filesystem"
        },
    ));
    let (active, metadata) = inspect_lock(&paths.lock);
    items.push(if active {
        DoctorItem::new(
            "Lock",
            "BUSY",
            metadata
                .as_ref()
                .map(describe_holder)
                .unwrap_or_else(|| "held".into()),
        )
    } else {
        DoctorItem::new("Lock", "OK", "not held")
    });
    items.extend(snapshot_items(&paths, &profile.root));
    items.push(schedule_item(&args.profile.profile));
    // The state directory may not exist yet; its filesystem is that of
    // the nearest existing ancestor.
    let free = paths
        .root
        .ancestors()
        .find(|ancestor| ancestor.exists())
        .and_then(|ancestor| sys::statvfs(ancestor).ok())
        .map_or(0, |usage| usage.available_bytes);
    items.push(DoctorItem::new(
        "State free space",
        if free > 100 * 1024 * 1024 {
            "OK"
        } else {
            "WARN"
        },
        format_size(free),
    ));
    render(args, &mut renderer, items)
}

fn render(args: &DoctorArgs, renderer: &mut Renderer, items: Vec<DoctorItem>) -> Result<i32> {
    let mut report = Report::new("doctor", Some(&args.profile.profile));
    report.heading(format!("qdu doctor — profile: {}", args.profile.profile));
    let mut table = Table::new(vec![
        Column::new("name", "Check", Align::Left),
        Column::new("status", "Status", Align::Left),
        Column::new("detail", "Detail", Align::Left),
    ]);
    for item in &items {
        table.push(vec![
            Field::text(item.name),
            Field::text(item.status).painted(status_paint(item.status)),
            Field::text(item.detail.clone()),
        ]);
    }
    report.rows(table);
    report.render(renderer);
    Ok(if items.iter().any(|item| item.status == "FAILED") {
        exit::FAILED_CHECK
    } else {
        0
    })
}
