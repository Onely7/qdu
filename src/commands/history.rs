//! Follow one directory through a profile's snapshots.

use std::path::Path;

use serde_json::{Value, json};

use crate::cli::HistoryArgs;
use crate::commands::{format_metric, renderer};
use crate::config::ConfigRepository;
use crate::error::{Result, usage};
use crate::model::{Metric, SnapshotRecord};
use crate::query::{self, QueryService, scope_in_root};
use crate::render::Align;
use crate::report::{Column, Field, Report, Table};
use crate::sys;
use crate::units::{format_age, format_local_time, format_size};

const WEEK: f64 = 7.0 * 86_400.0;

/// The value of a metric for a whole snapshot, from its index record.
fn root_value(record: &SnapshotRecord, metric: Metric) -> i64 {
    let value = match metric {
        Metric::Allocated => record.allocated_bytes,
        Metric::Apparent => record.apparent_bytes,
        Metric::Inodes => record.inode_count,
    };
    i64::try_from(value).unwrap_or(i64::MAX)
}

/// The least-squares slope of `value` over time, per week; `None` for fewer
/// than two points or a single moment.
fn weekly_growth(points: &[(i64, i64)]) -> Option<f64> {
    if points.len() < 2 {
        return None;
    }
    let count = points.len() as f64;
    let mean_time = points.iter().map(|(time, _)| *time as f64).sum::<f64>() / count;
    let mean_value = points.iter().map(|(_, value)| *value as f64).sum::<f64>() / count;
    let (mut covariance, mut variance) = (0.0, 0.0);
    for (time, value) in points {
        let time = *time as f64 - mean_time;
        covariance += time * (*value as f64 - mean_value);
        variance += time * time;
    }
    (variance > 0.0).then(|| covariance / variance * WEEK)
}

/// When a growing value reaches `limit`, from its latest point.
fn reached_at(latest: (i64, i64), limit: u64, growth_per_week: f64) -> Option<i64> {
    let remaining = limit as f64 - latest.1 as f64;
    if growth_per_week <= 0.0 || remaining <= 0.0 {
        return None;
    }
    let seconds = remaining / growth_per_week * WEEK;
    (seconds < 1e12).then(|| latest.0 + seconds as i64)
}

/// When a forecast moment comes, roughly, and its date.
fn when_text(epoch: i64) -> String {
    let seconds = (epoch - sys::now_epoch()).max(0) as f64;
    let weeks = seconds / WEEK;
    let rough = if weeks < 2.0 {
        format!("in about {}", format_age(seconds as i64))
    } else if weeks < 104.0 {
        format!("in about {} weeks", weeks.round())
    } else if weeks < 52.0 * 100.0 {
        format!("in about {} years", (weeks / 52.0).round())
    } else {
        return "not within a century".to_string();
    };
    format!("{rough} ({})", &format_local_time(epoch as f64)[..10])
}

/// Show how the value of one directory changed over the snapshots.
pub fn history(args: &HistoryArgs, config: &ConfigRepository) -> Result<i32> {
    let mut renderer = renderer(&args.output);
    let service = QueryService::new(&args.profile.profile)?;
    let index = service.index.load()?;
    let mut records: Vec<&SnapshotRecord> = index
        .snapshots
        .iter()
        .filter(|record| record.complete)
        .collect();
    let Some(newest) = records.last() else {
        return Err(usage(
            "no complete snapshot is available; run 'qdu snapshot' first",
        ));
    };
    let root = newest.root.clone();
    let scope = scope_in_root(&root, args.path.as_deref())?;
    let skip = records.len().saturating_sub(args.top);
    records.drain(..skip);
    let metric = args.metric;

    let mut points: Vec<(i64, i64)> = Vec::new();
    let mut table = Table::new(vec![
        Column::new("created_at", "Taken", Align::Left),
        Column::data("created_epoch"),
        Column::new(
            metric.column(),
            if metric == Metric::Inodes {
                "Inodes"
            } else {
                "Size"
            },
            Align::Right,
        ),
        Column::new("change", "Change", Align::Right),
        Column::new("snapshot_id", "Snapshot", Align::Left),
    ]);
    let mut previous: Option<i64> = None;
    for record in &records {
        let value = if scope == "." {
            Some(root_value(record, metric))
        } else {
            let context = service.open(record)?;
            query::directory(&context.connection, &scope)?.map(|directory| metric.of(&directory))
        };
        let change = value
            .zip(previous)
            .map(|(value, previous)| value - previous);
        table.push(vec![
            Field::new(
                record.created_at.clone(),
                format_local_time(record.created_epoch as f64),
            ),
            Field::data(record.created_epoch),
            match value {
                Some(value) => Field::new(value, format_metric(value, metric, false)),
                None => Field::new(Value::Null, "absent"),
            },
            match change {
                Some(change) => Field::new(change, format_metric(change, metric, true)),
                None => Field::new(Value::Null, ""),
            },
            Field::text(record.snapshot_id.clone()),
        ]);
        if let Some(value) = value {
            points.push((record.created_epoch, value));
        }
        previous = value;
    }

    let growth = weekly_growth(&points);
    let latest = points.last().copied();
    // A limit on the whole profile root applies to its allocation.
    let profile = config.load_profile(&args.profile.profile)?;
    let limit = profile
        .capacity_limit_bytes
        .filter(|_| scope == "." && metric == Metric::Allocated && profile.capacity_user.is_none());
    let limit_at = limit
        .zip(growth)
        .zip(latest)
        .and_then(|((limit, growth), latest)| reached_at(latest, limit, growth));
    // Free space comes from the filesystem on the host that took the
    // snapshots, and as the newest snapshot recorded it on any other host.
    let available = match records.last() {
        Some(newest) if metric == Metric::Allocated && newest.host != sys::hostname() => {
            let context = service.open(newest)?;
            context
                .metadata_value("filesystem_available_bytes")
                .and_then(|value| value.parse::<u64>().ok())
        }
        Some(_) if metric == Metric::Allocated => sys::statvfs(Path::new(&root))
            .ok()
            .map(|usage| usage.available_bytes),
        _ => None,
    };
    let disk_full_at =
        available
            .zip(growth)
            .zip(latest)
            .and_then(|((available, growth), latest)| {
                reached_at(latest, latest.1.max(0) as u64 + available, growth)
            });

    let mut report = Report::new("history", Some(&args.profile.profile));
    report.context("root", root.clone());
    report.context("scope", scope.clone());
    report.context("metric", metric.column());
    report.context("growth_per_week", growth);
    report.context(
        "forecast",
        json!({
            "capacity_limit_bytes": limit,
            "capacity_limit_reached_epoch": limit_at,
            "filesystem_full_epoch": disk_full_at,
        }),
    );
    report.heading("History");
    report.summary("Root", root);
    report.summary("Scope", scope);
    report.summary(
        "Growth",
        match growth {
            Some(growth) => format!(
                "{} per week over {} snapshots",
                format_metric(growth.round() as i64, metric, true),
                points.len()
            ),
            None => "needs two snapshots with this directory".to_string(),
        },
    );
    if let Some(limit) = limit {
        report.summary(
            "Limit",
            match limit_at {
                Some(epoch) => format!("{} reached {}", format_size(limit), when_text(epoch)),
                None if latest.is_some_and(|latest| latest.1 >= limit as i64) => {
                    format!("{} already exceeded", format_size(limit))
                }
                None => format!("{} not reached at this rate", format_size(limit)),
            },
        );
    }
    if let Some(epoch) = disk_full_at {
        report.summary("Disk", format!("full {} at this rate", when_text(epoch)));
    }
    report.rows(table);
    report.render(&mut renderer);
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn growth_is_the_weekly_slope() {
        let week = WEEK as i64;
        assert_eq!(weekly_growth(&[(0, 10)]), None);
        assert_eq!(weekly_growth(&[(5, 10), (5, 20)]), None);
        let growth = weekly_growth(&[(0, 100), (week, 200), (2 * week, 300)]).unwrap();
        assert!((growth - 100.0).abs() < 1e-6, "{growth}");
        assert_eq!(reached_at((0, 100), 300, 100.0), Some(2 * week));
        assert_eq!(reached_at((0, 100), 50, 100.0), None);
        assert_eq!(reached_at((0, 100), 300, -1.0), None);
    }
}
