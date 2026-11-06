//! Rank the largest directories of a snapshot.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use serde_json::{Value, json};

use crate::capacity::{self, CapacityAssessment};
use crate::cli::ShowArgs;
use crate::commands::{format_metric, renderer, select_snapshot, snapshot_text, status_text};
use crate::config::ConfigRepository;
use crate::error::{Result, usage};
use crate::filesystem::FilesystemInfo;
use crate::model::{DirectoryRecord, Metric, SnapshotRecord};
use crate::patterns::PathPattern;
use crate::query::{self, QueryService, RankingFilter, relative_to_scope, scope_in_root};
use crate::render::{Align, bar};
use crate::report::{Column, Field, Report, Table};
use crate::staleness::{self, StaleThresholds};
use crate::sys;
use crate::units::{format_iso_local, group, percent};

/// The summary lines of `show`: the root, the snapshot, its status, the
/// scope's total, and the capacity of its filesystems.
fn snapshot_summary(
    report: &mut Report,
    record: &SnapshotRecord,
    filesystem: &FilesystemInfo,
    scope: &DirectoryRecord,
    metric: Metric,
) {
    report.summary("Root", record.root.clone());
    report.summary("Snapshot", snapshot_text(record));
    if !record.complete || record.denied_count > 0 {
        report.summary(
            "Status",
            format!("{}; see 'qdu errors'", status_text(record)),
        );
    }
    report.summary(
        "Scope",
        format!(
            "{}  {} {}",
            scope.path,
            format_metric(metric.of(scope), metric, false),
            metric.label()
        ),
    );
    if metric == Metric::Inodes {
        report.summary("Inodes", filesystem.inode_text());
    } else {
        report.summary("Disk", filesystem.disk_text());
    }
    if filesystem.skipped_filesystem_count > 0 {
        report.summary(
            "Mounts",
            format!(
                "{} skipped; scan with --cross-filesystems to include",
                group(i128::from(filesystem.skipped_filesystem_count))
            ),
        );
    }
}

fn staleness_json(enabled: bool, minimum_age: Option<u64>, thresholds: &StaleThresholds) -> Value {
    let [notice, warning, elevated, critical] = thresholds.0;
    json!({
        "enabled": enabled,
        "minimum_age_days": minimum_age,
        "thresholds_days": if enabled {
            json!({"notice": notice, "warning": warning, "elevated": elevated, "critical": critical})
        } else {
            Value::Null
        },
        "reference": if enabled { json!("snapshot_captured_at") } else { Value::Null },
    })
}

/// Render directory usage from one snapshot.
pub fn show(args: &ShowArgs, config: &ConfigRepository) -> Result<i32> {
    let mut renderer = renderer(&args.output);
    let service = QueryService::new(&args.profile.profile)?;
    let record = select_snapshot(&service, &args.selector.snapshot, &mut renderer)?;
    let scope_path = scope_in_root(&record.root, args.path.as_deref())?;
    let stale_enabled = args.staleness.enabled();
    let profile = config.load_profile(&args.profile.profile)?;
    let policy = args.capacity.overrides().policy(&profile)?;
    let modified_before = staleness::cutoff_epoch(record.created_epoch, args.staleness.stale_only);
    let context = service.open(&record)?;
    let pattern = args
        .ranking
        .pattern
        .as_deref()
        .map(PathPattern::new)
        .transpose()?;
    let (scope, rows) = query::directory_ranking(
        &context.connection,
        &RankingFilter {
            under: &scope_path,
            max_depth: args.ranking.max_depth as i64,
            top: args.ranking.top,
            min_size: args.min_size as i64,
            pattern: pattern.as_ref(),
            metric: args.metric,
            modified_before_epoch: modified_before,
        },
    )?;
    let scope_value = args.metric.of(&scope);
    // Owners are known down to the depth the snapshot collected them.
    let owner_depth = if args.owners {
        if !context.has_owner_statistics() {
            return Err(usage(
                "--owners needs owner statistics; use 'qdu snapshot --with-users'",
            ));
        }
        Some(
            context
                .metadata_value("user_max_depth")
                .and_then(|value| value.parse::<i64>().ok())
                .unwrap_or(0),
        )
    } else {
        None
    };
    let assessment: Option<CapacityAssessment> = match &policy {
        None => None,
        Some(policy) => match capacity::assess(&context.connection, &context.metadata, policy) {
            Ok(assessment) => Some(assessment),
            Err(error) => {
                // The ranking stays useful when the snapshot cannot answer the
                // capacity question; 'qdu check' still fails on it.
                renderer.warning(&format!("capacity limit not evaluated: {error}"));
                None
            }
        },
    };
    capacity::warn(&mut renderer, assessment.as_ref());
    let thresholds = &args.staleness.stale_thresholds;
    let filesystem = FilesystemInfo::new(&context.metadata, Path::new(&record.root));

    let mut report = Report::new("show", Some(&args.profile.profile));
    report.context("snapshot", record.to_json());
    report.context("scope", scope.path.clone());
    report.context("metric", args.metric.column());
    report.context("scope_value", scope_value);
    report.context(
        "staleness",
        staleness_json(stale_enabled, args.staleness.stale_only, thresholds),
    );
    report.context(
        "capacity",
        assessment
            .as_ref()
            .map_or(Value::Null, CapacityAssessment::to_json),
    );
    report.context("filesystem", filesystem.to_json());
    report.heading("Disk usage");
    snapshot_summary(&mut report, &record, &filesystem, &scope, args.metric);
    if let Some(assessment) = &assessment {
        report.table_before(capacity::status_table(assessment));
    }
    let value_title = if args.metric == Metric::Inodes {
        "Inodes"
    } else {
        "Size"
    };
    let mut columns = vec![
        Column::new("rank", "Rank", Align::Right),
        Column::table(value_title, Align::Right),
        Column::data("allocated_bytes"),
        Column::data("apparent_bytes"),
        Column::data("inode_count"),
        Column::new("share_percent", "Share", Align::Right),
        Column::table("Distribution", Align::Left),
        Column::new("file_count", "Files", Align::Right),
        Column::data("directory_count"),
    ];
    if stale_enabled {
        columns.extend([
            Column::data("latest_modified_epoch"),
            Column::data("latest_modified_at"),
            Column::data("stale_age_days"),
            Column::data("stale_level"),
            Column::table("Stale", Align::Right),
        ]);
    }
    if owner_depth.is_some() {
        columns.extend([
            Column::data("owner_uid"),
            Column::new("owner", "Owner", Align::Left),
            Column::data("owner_share_percent"),
        ]);
    }
    columns.push(Column::path("path", "Path"));
    let owners = match owner_depth {
        Some(depth) => {
            let wanted: HashSet<&str> = rows
                .iter()
                .filter(|item| item.depth <= depth)
                .map(|item| item.path.as_str())
                .collect();
            let shallowest = rows.iter().map(|item| item.depth).min().unwrap_or(0);
            query::top_owners(
                &context.connection,
                &wanted,
                (shallowest, depth),
                args.metric,
            )?
        }
        None => HashMap::new(),
    };
    let mut table = Table::new(columns);
    for (index, item) in rows.iter().enumerate() {
        let value = args.metric.of(item);
        let share = if scope_value > 0 {
            value as f64 * 100.0 / scope_value as f64
        } else {
            0.0
        };
        let mut row = vec![
            Field::new(index + 1, (index + 1).to_string()),
            Field::shown(format_metric(value, args.metric, false)),
            Field::data(item.allocated_bytes),
            Field::data(item.apparent_bytes),
            Field::data(item.inode_count),
            Field::new(share, percent(value, scope_value)),
            Field::shown(bar(value, scope_value)),
            Field::new(item.file_count, group(i128::from(item.file_count))),
            Field::data(item.directory_count),
        ];
        let mut paint = None;
        if stale_enabled {
            let assessment =
                staleness::assess(item.latest_modified_epoch, record.created_epoch, thresholds);
            paint = assessment.level.paint();
            row.extend([
                Field::data(item.latest_modified_epoch),
                Field::data(format_iso_local(item.latest_modified_epoch)),
                Field::data(assessment.age_days),
                Field::data(assessment.level.name()),
                Field::shown(assessment.label()).painted(paint),
            ]);
        }
        if owner_depth.is_some() {
            match owners.get(&item.path).copied() {
                Some((uid, owned)) => {
                    let name = sys::username_for_uid(uid);
                    let owned_share = if value > 0 {
                        owned as f64 * 100.0 / value as f64
                    } else {
                        0.0
                    };
                    row.extend([
                        Field::data(uid),
                        Field::new(name.clone(), format!("{name} {}", percent(owned, value))),
                        Field::data(owned_share),
                    ]);
                }
                None => row.extend([
                    Field::data(Value::Null),
                    Field::new(Value::Null, ""),
                    Field::data(Value::Null),
                ]),
            }
        }
        row.push(Field::text(relative_to_scope(&item.path, &scope.path)).painted(paint));
        table.push(row);
    }
    report.rows(table);
    if stale_enabled {
        report.note(
            "Stale age is measured at snapshot time from the newest modification inside each \
             directory tree.",
        );
    }
    report.render(&mut renderer);
    Ok(0)
}
