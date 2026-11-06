//! Evaluate alert thresholds for scripts and cron.

use std::path::PathBuf;

use serde_json::Value;

use crate::capacity;
use crate::cli::CheckArgs;
use crate::commands::{renderer, select_snapshot};
use crate::config::ConfigRepository;
use crate::error::{Result, exit, usage};
use crate::filesystem::{FilesystemInfo, FilesystemUsage};
use crate::query::{QueryService, directory_allocation, scope_in_root};
use crate::render::{Align, Paint};
use crate::report::{Column, Field, Report, Table};
use crate::sys;
use crate::units::{format_age, format_bytes, format_size};

/// The outcome of one threshold check.
struct CheckResult {
    name: &'static str,
    passed: bool,
    /// The measured and allowed values, in `unit`.
    observed: Value,
    threshold: Value,
    unit: &'static str,
    observed_text: String,
    threshold_text: String,
    target: String,
}

/// Evaluate thresholds for automation; exit status 10 on an alert.
pub fn check(args: &CheckArgs, config: &ConfigRepository) -> Result<i32> {
    let mut renderer = renderer(&args.output);
    let service = QueryService::new(&args.profile.profile)?;
    let current_record = select_snapshot(&service, &args.selector.snapshot, &mut renderer)?;
    let profile = config.load_profile(&args.profile.profile)?;
    let policy = args.capacity.overrides().policy(&profile)?;
    let mut results = Vec::new();
    if let Some(growth_over) = args.growth_over {
        let previous_record = service.previous_of(&current_record)?;
        let previous = service.open(&previous_record)?;
        let current = service.open(&current_record)?;
        let scope = scope_in_root(&current_record.root, args.path.as_deref())?;
        let previous_value = directory_allocation(&previous.connection, &scope)?;
        let current_value = directory_allocation(&current.connection, &scope)?;
        if previous_value.is_none() && current_value.is_none() {
            return Err(usage(format!(
                "directory is absent from both snapshots: {scope}"
            )));
        }
        // A directory present on one side only was created or removed.
        let change = current_value.unwrap_or(0) - previous_value.unwrap_or(0);
        let threshold = i64::try_from(growth_over).unwrap_or(i64::MAX);
        results.push(CheckResult {
            name: "growth",
            passed: change <= threshold,
            observed: change.into(),
            threshold: threshold.into(),
            unit: "bytes",
            observed_text: format_bytes(change, true),
            threshold_text: format_bytes(threshold, false),
            target: scope,
        });
    }
    let root = PathBuf::from(&current_record.root);
    for (name, limit, inodes) in [
        ("disk_usage", args.disk_usage_over, false),
        ("inode_usage", args.inode_usage_over, true),
    ] {
        let Some(limit) = limit else {
            continue;
        };
        // Another host's filesystem can only be judged as the snapshot saw it.
        let usage = if current_record.host == sys::hostname() {
            let live = sys::statvfs(&root)?;
            FilesystemUsage {
                filesystem_count: 1,
                total_bytes: live.total_bytes,
                available_bytes: live.available_bytes,
                free_bytes: Some(live.free_bytes),
                total_inodes: live.total_inodes,
                available_inodes: live.available_inodes,
            }
        } else {
            FilesystemInfo::new(&service.open(&current_record)?.metadata, &root).snapshot
        };
        let used_percent = if !inodes {
            usage.used_percent()
        } else if usage.total_inodes == 0 {
            0.0
        } else {
            usage.total_inodes.saturating_sub(usage.available_inodes) as f64 * 100.0
                / usage.total_inodes as f64
        };
        results.push(CheckResult {
            name,
            passed: used_percent <= limit,
            observed: used_percent.into(),
            threshold: limit.into(),
            unit: "percent",
            observed_text: format!("{used_percent:.1}%"),
            threshold_text: format!("{limit:.1}%"),
            target: root.display().to_string(),
        });
    }
    if let Some(policy) = &policy {
        let current = service.open(&current_record)?;
        let assessment = capacity::assess(&current.connection, &current.metadata, policy)?;
        results.push(CheckResult {
            name: "operational_capacity",
            passed: !assessment.exceeded(),
            observed: assessment.used_bytes.into(),
            threshold: assessment.limit_bytes.into(),
            unit: "bytes",
            observed_text: format!(
                "{} ({:.1}%)",
                format_size(assessment.used_bytes),
                assessment.usage_percent()
            ),
            threshold_text: format_size(assessment.limit_bytes),
            target: assessment.target.clone(),
        });
    }
    if let Some(maximum) = args.snapshot_age_over {
        let age = (sys::now_epoch() - current_record.created_epoch).max(0);
        results.push(CheckResult {
            name: "snapshot_age",
            passed: age as u64 <= maximum,
            observed: age.into(),
            threshold: maximum.into(),
            unit: "seconds",
            observed_text: format_age(age),
            threshold_text: format_age(i64::try_from(maximum).unwrap_or(i64::MAX)),
            target: current_record.snapshot_id.clone(),
        });
    }
    if results.is_empty() {
        return Err(usage(
            "check requires at least one threshold option or a configured capacity limit",
        ));
    }
    let passed = results.iter().all(|result| result.passed);
    let mut report = Report::new("check", Some(&args.profile.profile));
    report.context("snapshot", current_record.to_json());
    report.context("passed", passed);
    report.heading("Checks");
    let mut table = Table::new(vec![
        Column::new("name", "Check", Align::Left),
        Column::data("passed"),
        Column::table("Result", Align::Center),
        Column::data("observed"),
        Column::data("threshold"),
        Column::data("unit"),
        Column::new("observed_text", "Observed", Align::Right),
        Column::new("threshold_text", "Threshold", Align::Right),
        Column::path("target", "Target"),
    ]);
    for result in results {
        table.push(vec![
            Field::text(result.name),
            Field::data(result.passed),
            Field::shown(if result.passed { "PASS" } else { "ALERT" }).painted(Some(
                if result.passed {
                    Paint::Green
                } else {
                    Paint::Red
                },
            )),
            Field::data(result.observed),
            Field::data(result.threshold),
            Field::data(result.unit),
            Field::text(result.observed_text),
            Field::text(result.threshold_text),
            Field::text(result.target),
        ]);
    }
    report.rows(table);
    report.render(&mut renderer);
    Ok(if passed { 0 } else { exit::ALERT })
}
