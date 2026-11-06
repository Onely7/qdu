//! List the paths that a snapshot could not read.

use crate::cli::ErrorsArgs;
use crate::commands::{renderer, status_text};
use crate::error::Result;
use crate::query::QueryService;
use crate::render::Align;
use crate::report::{Column, Field, Report, Table};

/// Render the scan errors stored in a snapshot.
pub fn errors(args: &ErrorsArgs) -> Result<i32> {
    let mut renderer = renderer(&args.output);
    let service = QueryService::new(&args.profile.profile)?;
    let record = service.index.resolve(&args.snapshot, true)?;
    let context = service.open(&record)?;
    let values = context.scan_errors(args.top)?;
    let mut report = Report::new("errors", Some(&args.profile.profile));
    report.context("snapshot", record.to_json());
    report.context("error_count", record.error_count);
    report.context("denied_count", record.denied_count);
    report.heading("Snapshot scan errors");
    report.summary("Snapshot", record.snapshot_id.clone());
    report.summary("Status", status_text(&record));
    let mut table = Table::new(vec![
        Column::path("path", "Path"),
        Column::new("operation", "Operation", Align::Left),
        Column::new("kind", "Kind", Align::Left),
        Column::new("message", "Message", Align::Left),
    ])
    .empty("No scan errors.");
    for error in values {
        table.push(vec![
            Field::text(error.path),
            Field::text(error.operation),
            Field::text(error.kind),
            Field::text(error.message),
        ]);
    }
    report.rows(table);
    report.render(&mut renderer);
    Ok(0)
}
