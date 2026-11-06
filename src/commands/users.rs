//! Rank the owners of a snapshot's files and their directories.

use serde_json::Value;

use crate::cli::UsersArgs;
use crate::commands::{renderer, select_snapshot};
use crate::error::{Result, usage};
use crate::model::Metric;
use crate::query::{self, QueryService, scope_in_root};
use crate::render::Align;
use crate::report::{Column, Field, Report, Table};
use crate::units::{format_bytes, group, percent};

/// Owner usage: one row per owner and one per directory of an owner, which
/// a table lays out as a ranking of owners and one table per owner.
fn owner_report(report: &mut Report, ranking: query::OwnerRanking, metric: Metric) {
    let query::OwnerRanking {
        owners,
        directories,
        total,
    } = ranking;
    let inodes = metric == Metric::Inodes;
    let share = |part: i64, whole: i64| {
        if whole > 0 {
            part as f64 * 100.0 / whole as f64
        } else {
            0.0
        }
    };
    let measured = |allocated: i64, inode_count: i64| if inodes { inode_count } else { allocated };
    let mut data = Table::new(vec![
        Column::data("type"),
        Column::data("rank"),
        Column::data("uid"),
        Column::data("user"),
        Column::data("path"),
        Column::data("depth"),
        Column::data("allocated_bytes"),
        Column::data("apparent_bytes"),
        Column::data("inode_count"),
        Column::data("file_count"),
        Column::data("directory_count"),
        Column::data("share_percent"),
    ]);
    let (value_title, other_title) = if inodes {
        ("Inodes", "Size")
    } else {
        ("Size", "Inodes")
    };
    let mut ranking_table = Table::new(vec![
        Column::table("Rank", Align::Right),
        Column::table("User", Align::Left),
        Column::table("UID", Align::Right),
        Column::table(value_title, Align::Right),
        Column::table("Share", Align::Right),
        Column::table(other_title, Align::Right),
    ])
    .heading("User usage");
    let mut tables = Vec::new();
    for (index, owner) in owners.iter().enumerate() {
        let rank = index + 1;
        let value = measured(owner.allocated_bytes, owner.inode_count);
        let size = format_bytes(owner.allocated_bytes, false);
        let count = group(i128::from(owner.inode_count));
        let (value_text, other_text) = if inodes { (count, size) } else { (size, count) };
        data.push(vec![
            Field::data("user"),
            Field::data(rank),
            Field::data(owner.uid),
            Field::data(owner.username.clone()),
            Field::data(Value::Null),
            Field::data(Value::Null),
            Field::data(owner.allocated_bytes),
            Field::data(owner.apparent_bytes),
            Field::data(owner.inode_count),
            Field::data(owner.file_count),
            Field::data(owner.directory_count),
            Field::data(share(value, total)),
        ]);
        ranking_table.push(vec![
            Field::shown(rank.to_string()),
            Field::shown(owner.username.clone()),
            Field::shown(owner.uid.to_string()),
            Field::shown(value_text),
            Field::shown(percent(value, total)),
            Field::shown(other_text),
        ]);
        let Some(items) = directories
            .get(&owner.uid)
            .filter(|items| !items.is_empty())
        else {
            continue;
        };
        let mut owner_table = Table::new(vec![
            Column::table(value_title, Align::Right),
            Column::table("Share", Align::Right),
            Column::table(other_title, Align::Right),
            Column::path("", "Path"),
        ])
        .heading(format!("{}: top directories", owner.username));
        for item in items {
            let part = measured(item.allocated_bytes, item.inode_count);
            let size = format_bytes(item.allocated_bytes, false);
            let count = group(i128::from(item.inode_count));
            let (value_text, other_text) = if inodes { (count, size) } else { (size, count) };
            data.push(vec![
                Field::data("directory"),
                Field::data(rank),
                Field::data(owner.uid),
                Field::data(owner.username.clone()),
                Field::data(item.path.clone()),
                Field::data(item.depth),
                Field::data(item.allocated_bytes),
                Field::data(item.apparent_bytes),
                Field::data(item.inode_count),
                Field::data(item.file_count),
                Field::data(item.directory_count),
                Field::data(share(part, value)),
            ]);
            owner_table.push(vec![
                Field::shown(value_text),
                Field::shown(percent(part, value)),
                Field::shown(other_text),
                Field::shown(item.path.clone()),
            ]);
        }
        tables.push(owner_table);
    }
    tables.insert(0, ranking_table);
    report.rows_shown_as(data, tables);
}

/// Render owner usage from a snapshot.
pub fn users(args: &UsersArgs) -> Result<i32> {
    let mut renderer = renderer(&args.output);
    let service = QueryService::new(&args.profile.profile)?;
    let record = select_snapshot(&service, &args.selector.snapshot, &mut renderer)?;
    let context = service.open(&record)?;
    if !context.has_owner_statistics() {
        return Err(usage(
            "this snapshot has no owner statistics; use 'qdu snapshot --with-users'",
        ));
    }
    let scope = scope_in_root(&record.root, args.path.as_deref())?;
    let shown = args.path.as_deref().unwrap_or(".");
    let depth = if scope == "." {
        0
    } else {
        scope.split('/').count() as u32
    };
    // Owner statistics stop at the depth the snapshot was taken with.
    let reach: u32 = context
        .metadata_value("user_max_depth")
        .and_then(|value| value.parse().ok())
        .unwrap_or(0);
    if depth > reach {
        return Err(usage(format!(
            "owner statistics of this snapshot reach depth {reach}, but {shown} is at depth \
             {depth}; take snapshots with --user-max-depth {depth} to rank its owners"
        )));
    }
    let ranking = query::owners(
        &context.connection,
        &scope,
        args.top,
        args.dirs as usize,
        args.metric,
    )?;
    if ranking.owners.is_empty() && scope != "." {
        return Err(usage(format!(
            "directory is not present in snapshot: {shown}"
        )));
    }
    let mut report = Report::new("users", Some(&args.profile.profile));
    report.context("snapshot", record.to_json());
    report.context("scope", scope.clone());
    report.context("metric", args.metric.column());
    owner_report(&mut report, ranking, args.metric);
    report.render(&mut renderer);
    Ok(0)
}
