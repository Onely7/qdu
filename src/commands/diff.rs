//! Rank what changed between two snapshots.

use std::collections::HashMap;

use crate::cli::DiffArgs;
use crate::commands::{format_metric, renderer, select_snapshot};
use crate::error::{Result, usage};
use crate::model::{DiffRecord, SnapshotRecord};
use crate::patterns::PathPattern;
use crate::query::{self, DiffFilter, DiffOrder, QueryService, relative_to_scope, scope_in_root};
use crate::render::{Align, Renderer};
use crate::report::{Column, Field, Report, Table};
use crate::units::{format_age, format_local_time};

fn warn_metadata_difference(
    renderer: &mut Renderer,
    previous: &SnapshotRecord,
    current: &SnapshotRecord,
    previous_metadata: &HashMap<String, String>,
    current_metadata: &HashMap<String, String>,
) {
    if previous.root != current.root {
        renderer.warning(&format!(
            "snapshot roots differ: {} vs {}",
            previous.root, current.root
        ));
    }
    if previous.exclude_hash != current.exclude_hash {
        renderer.warning("exclusion patterns differ between snapshots");
    }
    let mode = |metadata: &HashMap<String, String>| {
        metadata
            .get("cross_filesystems")
            .cloned()
            .unwrap_or_else(|| "false".into())
    };
    if mode(previous_metadata) != mode(current_metadata) {
        renderer.warning("filesystem traversal modes differ between snapshots");
    }
    // Snapshots before schema 4 did not record their depth.
    let depth = |metadata: &HashMap<String, String>| {
        metadata
            .get("record_max_depth")
            .map(|depth| match depth.as_str() {
                "" => "unlimited".to_string(),
                depth => depth.to_string(),
            })
    };
    if let (Some(previous), Some(current)) = (depth(previous_metadata), depth(current_metadata)) {
        if previous != current {
            renderer.warning(&format!(
                "the snapshots store directories to different depths ({previous} and \
                 {current}); directories below the shallower depth appear as created or \
                 removed"
            ));
        }
    }
}

fn format_change_percent(record: &DiffRecord) -> String {
    match record.change_percent() {
        Some(percent) => format!("{percent:+.1}%"),
        None if record.current != 0 => "new".into(),
        None => String::new(),
    }
}

/// Render directory changes between two snapshots.
pub fn diff(args: &DiffArgs) -> Result<i32> {
    let mut renderer = renderer(&args.output);
    let renderer = &mut renderer;
    let pattern = args
        .ranking
        .pattern
        .as_deref()
        .map(PathPattern::new)
        .transpose()?;
    let service = QueryService::new(&args.profile.profile)?;
    let current_record = select_snapshot(&service, &args.comparison.to_snapshot, renderer)?;
    let previous_record = match &args.comparison.from_snapshot {
        Some(selector) => service.resolve(selector)?,
        None => service.previous_of(&current_record)?,
    };
    if previous_record.snapshot_id == current_record.snapshot_id {
        return Err(usage(format!(
            "--from and --to both name {}; compare two snapshots",
            current_record.snapshot_id
        )));
    }
    // The index lists snapshots in the order they were taken, which also
    // orders two taken within one second.
    let position = |id: &str| {
        service.index.load().ok().and_then(|index| {
            index
                .snapshots
                .iter()
                .position(|record| record.snapshot_id == id)
        })
    };
    if position(&previous_record.snapshot_id) > position(&current_record.snapshot_id) {
        return Err(usage(format!(
            "--from names {} from {}, which is newer than --to, {} from {}; swap them",
            previous_record.snapshot_id,
            format_local_time(previous_record.created_epoch as f64),
            current_record.snapshot_id,
            format_local_time(current_record.created_epoch as f64)
        )));
    }
    let scope = scope_in_root(&current_record.root, args.path.as_deref())?;
    let previous = service.open(&previous_record)?;
    let current = service.open(&current_record)?;
    warn_metadata_difference(
        renderer,
        &previous_record,
        &current_record,
        &previous.metadata,
        &current.metadata,
    );
    let filter = &DiffFilter {
        under: &scope,
        max_depth: args.ranking.max_depth as i64,
        top: args.ranking.top,
        min_size: args.min_size as i64,
        pattern: pattern.as_ref(),
        growth_only: args.comparison.growth_only,
        shrink_only: args.comparison.shrink_only,
        order: match args.comparison.order {
            Some(order) => order,
            None if args.comparison.shrink_only => DiffOrder::Shrink,
            None => DiffOrder::Growth,
        },
        metric: args.metric,
    };
    let rows = query::diff_ranking(&previous.connection, &current.connection, filter)?;
    let metric = filter.metric;

    let mut report = Report::new("diff", Some(&args.profile.profile));
    report.context("previous", previous_record.to_json());
    report.context("current", current_record.to_json());
    report.context("scope", scope.clone());
    report.context("metric", metric.column());
    report.context("order", filter.order.name());
    report.heading("Changes");
    let span = (current_record.created_epoch - previous_record.created_epoch).max(0);
    report.summary(
        "From",
        format!(
            "{}  {}",
            format_local_time(previous_record.created_epoch as f64),
            previous_record.snapshot_id
        ),
    );
    report.summary(
        "To",
        format!(
            "{}  {}  ({} later)",
            format_local_time(current_record.created_epoch as f64),
            current_record.snapshot_id,
            format_age(span)
        ),
    );
    report.summary("Scope", scope.clone());
    report.summary(
        "Metric",
        format!("{}, ordered by {}", metric.label(), filter.order.name()),
    );
    let mut table = Table::new(vec![
        Column::new("rank", "Rank", Align::Right),
        Column::table("", Align::Center),
        Column::new("change", "Change", Align::Right),
        Column::new("change_percent", "Change %", Align::Right),
        Column::new("current", "Current", Align::Right),
        Column::new("previous", "Previous", Align::Right),
        Column::path("path", "Path"),
    ])
    .empty("No changes in this scope.");
    for (index, item) in rows.iter().enumerate() {
        let change = item.change();
        let symbol = match change {
            change if change > 0 => "↑",
            change if change < 0 => "↓",
            _ => "→",
        };
        table.push(vec![
            Field::new(index + 1, (index + 1).to_string()),
            Field::shown(symbol),
            Field::new(change, format_metric(change, metric, true)),
            Field::new(item.change_percent(), format_change_percent(item)),
            Field::new(item.current, format_metric(item.current, metric, false)),
            Field::new(item.previous, format_metric(item.previous, metric, false)),
            Field::text(relative_to_scope(&item.path, &scope)),
        ]);
    }
    report.rows(table);
    report.render(renderer);
    Ok(0)
}
