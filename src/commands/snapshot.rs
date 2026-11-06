//! Scan a profile root and save a snapshot.

use std::path::PathBuf;
use std::sync::Mutex;

use serde_json::json;

use crate::cli::SnapshotArgs;
use crate::commands::{count, format_stored, load_profile, renderer, status_text};
use crate::config::ConfigRepository;
use crate::error::{Result, exit};
use crate::render::ProgressLine;
use crate::report::Report;
use crate::repository::{SnapshotOutcome, create_snapshot};
use crate::scan::{Phase, ScanProgress};
use crate::storage::{IndexRepository, ProfilePaths};
use crate::sys;
use crate::units::{format_age, format_size, group};

fn retention_text(outcome: &SnapshotOutcome) -> String {
    let mut parts = Vec::new();
    if !outcome.removed.is_empty() {
        parts.push(format!("removed {} old snapshot(s)", outcome.removed.len()));
    }
    if !outcome.archived.is_empty() {
        parts.push(format!(
            "compressed {} older snapshot(s)",
            outcome.archived.len()
        ));
    }
    parts.join(", ")
}

fn progress_text(progress: &ScanProgress) -> String {
    let elapsed = format_age(progress.elapsed_seconds as i64);
    if progress.phase == Phase::Writing {
        return format!(
            "Writing snapshot ({} entries scanned in {elapsed})",
            group(i128::from(progress.entries))
        );
    }
    let mut text = format!(
        "Scanning {} entries in {} directories, {}, {elapsed}",
        group(i128::from(progress.entries)),
        group(i128::from(progress.directories)),
        format_size(progress.allocated_bytes)
    );
    if progress.errors > 0 {
        text.push_str(&format!(", {} errors", group(i128::from(progress.errors))));
    }
    text
}

/// Scan a profile root and save a new snapshot.
pub fn snapshot(args: &SnapshotArgs, config: &ConfigRepository) -> Result<i32> {
    let mut renderer = renderer(&args.output);
    let mut overrides = args.collection.overrides();
    overrides.root = args.root.clone();
    // Only a new profile's first snapshot keeps its options unasked: a
    // profile with snapshots but no section, as from the Python qdu 0.2.0,
    // may be trying an option once.
    let new_profile = !config.has_profile(&args.profile.profile)?
        && IndexRepository::new(ProfilePaths::for_profile(&args.profile.profile)?)
            .load()
            .is_ok_and(|index| index.snapshots.is_empty());
    let profile = load_profile(
        config,
        &args.profile.profile,
        overrides.clone(),
        &args.exclude,
        &args.exclude_from,
    )?;
    let show_progress = !args.quiet && sys::is_terminal(libc::STDERR_FILENO);
    let line = Mutex::new(ProgressLine::new());
    let report = |progress: &ScanProgress| {
        if let Ok(mut line) = line.lock() {
            let detail = if progress.phase == Phase::Writing {
                ""
            } else {
                progress.current_path.as_str()
            };
            line.update(&progress_text(progress), detail);
        }
    };
    let outcome = create_snapshot(
        &profile,
        &args.label,
        args.threads,
        show_progress.then_some(&report as _),
    );
    if let Ok(mut line) = line.lock() {
        line.clear();
    }
    let outcome = outcome?;
    let record = &outcome.record;
    let summary = &outcome.summary;
    // What was given on the command line becomes the profile's settings on
    // its first snapshot or with --save.
    if !args.exclude.is_empty() || !args.exclude_from.is_empty() {
        overrides.excludes = Some(profile.excludes.clone());
    }
    if overrides.root.is_some() {
        // The root as scanned: absolute and without symbolic links.
        overrides.root = Some(PathBuf::from(&record.root));
    }
    let settings = overrides.render();
    let saved: Vec<&str> = if !settings.is_empty() && (args.save || new_profile) {
        config.update_profile(&profile.name, &settings)?;
        settings.iter().map(|(key, _)| *key).collect()
    } else {
        Vec::new()
    };
    if !args.quiet {
        let paths = ProfilePaths::for_profile(&profile.name)?;
        let mut report = Report::new("snapshot", Some(&profile.name));
        report.context("snapshot", record.to_json());
        report.context(
            "scan",
            json!({
                "duration_seconds": summary.duration_seconds,
                "error_count": summary.error_count,
                "denied_count": summary.denied_count,
                "errors": summary
                    .errors
                    .iter()
                    .map(|error| json!({
                        "path": error.path,
                        "operation": error.operation,
                        "kind": error.kind(),
                        "message": error.message,
                    }))
                    .collect::<Vec<_>>(),
                "cross_filesystems": summary.cross_filesystems,
                "skipped_filesystem_count": summary.skipped_filesystem_count,
                "skipped_filesystems": summary.skipped_filesystems,
                "automount_paths": summary.automount_paths,
            }),
        );
        report.context("stored_bytes", paths.stored_bytes(record));
        report.context(
            "retention",
            json!({"removed": outcome.removed, "archived": outcome.archived}),
        );
        report.context("saved_settings", saved.clone());
        report.heading("Snapshot saved");
        report.summary("Profile", profile.name.clone());
        report.summary("Snapshot", record.snapshot_id.clone());
        report.summary("Root", record.root.clone());
        report.summary("Status", status_text(record));
        report.summary(
            "Usage",
            format!(
                "{} in {} and {}",
                format_size(record.allocated_bytes),
                count(record.file_count, "file", "files"),
                count(record.directory_count, "directory", "directories")
            ),
        );
        report.summary(
            "Duration",
            format_age(summary.duration_seconds.round_ties_even() as i64),
        );
        report.summary("Stored", format_stored(paths.stored_bytes(record)));
        if summary.cross_filesystems {
            report.summary("Filesystems", "crossed into other mounts");
        }
        if !outcome.removed.is_empty() || !outcome.archived.is_empty() {
            report.summary("Retention", retention_text(&outcome));
        }
        if !saved.is_empty() {
            report.summary(
                "Saved",
                format!("{} for later snapshots of this profile", saved.join(", ")),
            );
        }
        report.render(&mut renderer);
    }
    if summary.skipped_filesystem_count > 0 {
        let skipped = summary.skipped_filesystem_count;
        renderer.warning(&format!(
            "{} {} skipped because the scan is limited to one filesystem. \
             Use --cross-filesystems to include them.",
            count(skipped, "mounted directory", "mounted directories"),
            if skipped == 1 { "was" } else { "were" }
        ));
        for path in summary.skipped_filesystems.iter().take(10) {
            renderer.warning(&format!("skipped mount: {path}"));
        }
    }
    for path in &summary.automount_paths {
        // An automounter map lists only what is mounted when it is read.
        let shown = if path == "." {
            record.root.clone()
        } else {
            format!("{}/{path}", record.root.trim_end_matches('/'))
        };
        renderer.warning(&format!(
            "{shown} is an automount point, which lists only the entries mounted when the \
             scan read it; the others were left out. Open each one first, for example with \
             'ls {shown}/NAME', to include it."
        ));
    }
    if summary.error_count > 0 {
        renderer.warning(&format!(
            "{} could not be read. The snapshot is kept as latest-any but does not \
             replace latest.",
            count(summary.error_count, "path", "paths")
        ));
        for error in summary.errors.iter().filter(|error| !error.denied).take(10) {
            renderer.warning(&format!("{}: {}", error.path, error.message));
        }
    }
    if summary.denied_count > 0 {
        renderer.warning(&format!(
            "{} could not be read for lack of permission and {} left out; \
             see 'qdu errors'.",
            count(summary.denied_count, "path", "paths"),
            if summary.denied_count == 1 {
                "is"
            } else {
                "are"
            }
        ));
        for error in summary.errors.iter().filter(|error| error.denied).take(10) {
            renderer.warning(&format!("{}: {}", error.path, error.message));
        }
    }
    Ok(if record.complete { 0 } else { exit::INCOMPLETE })
}
