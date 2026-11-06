//! Rank the largest files of a snapshot or of a live scan.

use std::path::PathBuf;
use std::sync::Mutex;

use serde_json::{Value, json};

use crate::cli::FilesArgs;
use crate::commands::{effective_profile, renderer, select_snapshot, snapshot_text};
use crate::config::ConfigRepository;
use crate::error::Result;
use crate::live::{self, LiveFilter, LiveProgress};
use crate::patterns::PathMatcher;
use crate::query::{self, FileFilter, QueryService, resolve_user, scope_in_root};
use crate::render::{Align, ProgressLine};
use crate::report::{Column, Field, Report, Table};
use crate::repository::{canonical_root, scan_threads};
use crate::staleness;
use crate::sys;
use crate::units::{format_age, format_bytes, format_local_time, group};

/// Render large or stale files from a snapshot or a live scan.
pub fn files(args: &FilesArgs, config: &ConfigRepository) -> Result<i32> {
    let mut renderer = renderer(&args.output);
    let stale_enabled = args.staleness.enabled();
    let min_size = args.min_size;
    let uid = args.user.as_deref().map(resolve_user).transpose()?;
    let mut report = Report::new("files", Some(&args.profile.profile));
    report.heading("Largest files");
    let (records, reference_epoch) = if args.live {
        let profile = effective_profile(config, &args.profile.profile)?;
        let matcher = PathMatcher::new(&profile.excludes)?;
        let root: PathBuf = match &args.root {
            Some(path) => path.clone(),
            None => QueryService::new(&args.profile.profile)
                .and_then(|service| service.resolve(&args.selector.snapshot))
                .map(|record| PathBuf::from(record.root))
                .unwrap_or_else(|_| profile.root.clone()),
        };
        let root = canonical_root(&root);
        let scope = scope_in_root(&root.to_string_lossy(), args.path.as_deref())?;
        let reference_epoch = sys::now_epoch();
        let line = Mutex::new(ProgressLine::new());
        let draw = |progress: &LiveProgress| {
            if let Ok(mut line) = line.lock() {
                line.update(
                    &format!(
                        "Scanning {} entries in {} directories, {}",
                        group(i128::from(progress.entries)),
                        group(i128::from(progress.directories)),
                        format_age(progress.elapsed_seconds as i64)
                    ),
                    &progress.current_path,
                );
            }
        };
        let show_progress = sys::is_terminal(libc::STDERR_FILENO);
        let scanned = live::largest_files(&LiveFilter {
            root: &root,
            matcher: &matcher,
            top: args.top,
            under: &scope,
            min_size,
            uid,
            modified_before_epoch: staleness::cutoff_epoch(
                reference_epoch,
                args.staleness.stale_only,
            ),
            one_file_system: !profile.cross_filesystems,
            threads: scan_threads(args.threads),
            progress: show_progress.then_some(&draw as _),
        });
        if let Ok(mut line) = line.lock() {
            line.clear();
        }
        let (records, errors) = scanned?;
        for message in errors.iter().take(10) {
            renderer.warning(message);
        }
        report.context("source", "live");
        report.context("snapshot", Value::Null);
        report.context("root", root.display().to_string());
        report.context("scope", scope);
        report.context("stored_candidate_limit", Value::Null);
        report.summary("Root", root.display().to_string());
        report.summary(
            "Source",
            format!("live scan at {}", format_local_time(reference_epoch as f64)),
        );
        (records, reference_epoch)
    } else {
        let service = QueryService::new(&args.profile.profile)?;
        let record = select_snapshot(&service, &args.selector.snapshot, &mut renderer)?;
        let scope = scope_in_root(&record.root, args.path.as_deref())?;
        let context = service.open(&record)?;
        let records = query::large_files(
            &context.connection,
            &FileFilter {
                top: args.top,
                under: &scope,
                min_size: min_size as i64,
                uid,
                modified_before_epoch: staleness::cutoff_epoch(
                    record.created_epoch,
                    args.staleness.stale_only,
                ),
            },
        )?;
        let stored_limit: u64 = context
            .metadata_value("large_file_limit")
            .and_then(|value| value.parse().ok())
            .unwrap_or(0);
        report.context("source", "snapshot");
        report.context("snapshot", record.to_json());
        report.context("root", record.root.clone());
        report.context("scope", scope);
        report.context("stored_candidate_limit", stored_limit);
        report.summary("Root", record.root.clone());
        report.summary("Snapshot", snapshot_text(&record));
        report.summary(
            "Candidates",
            if stored_limit > 0 {
                format!(
                    "the {} largest files at snapshot time; --live checks every file",
                    group(i128::from(stored_limit))
                )
            } else {
                "none recorded; use --live, or snapshot with --file-top N".to_string()
            },
        );
        (records, record.created_epoch)
    };
    let thresholds = &args.staleness.stale_thresholds;
    report.context(
        "staleness",
        json!({
            "enabled": stale_enabled,
            "reference_epoch": stale_enabled.then_some(reference_epoch),
            "thresholds_days": stale_enabled.then(|| thresholds.as_text()),
        }),
    );
    let mut columns = vec![
        Column::new("rank", "Rank", Align::Right),
        Column::new("allocated_bytes", "Allocated", Align::Right),
        Column::new("apparent_bytes", "Apparent", Align::Right),
        Column::data("uid"),
        Column::new("user", "Owner", Align::Left),
        Column::data("modified_epoch"),
        Column::table("Modified", Align::Left),
    ];
    if stale_enabled {
        columns.extend([
            Column::data("stale_age_days"),
            Column::data("stale_level"),
            Column::table("Stale", Align::Right),
        ]);
    }
    columns.push(Column::path("path", "Path"));
    let mut table = Table::new(columns);
    for (index, item) in records.iter().enumerate() {
        let mut row = vec![
            Field::new(index + 1, (index + 1).to_string()),
            Field::new(
                item.allocated_bytes,
                format_bytes(item.allocated_bytes, false),
            ),
            Field::new(
                item.apparent_bytes,
                format_bytes(item.apparent_bytes, false),
            ),
            Field::data(item.uid),
            Field::text(item.username.clone()),
            Field::data(item.modified_epoch),
            Field::shown(format_local_time(item.modified_epoch)),
        ];
        let mut paint = None;
        if stale_enabled {
            let assessment = staleness::assess(item.modified_epoch, reference_epoch, thresholds);
            paint = assessment.level.paint();
            row.extend([
                Field::data(assessment.age_days),
                Field::data(assessment.level.name()),
                Field::shown(assessment.label()).painted(paint),
            ]);
        }
        row.push(Field::text(item.path.clone()).painted(paint));
        table.push(row);
    }
    report.rows(table);
    report.render(&mut renderer);
    Ok(0)
}
