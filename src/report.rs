//! One description of a command's output for its table, TSV, and JSON forms.
//!
//! A report has a context, such as the snapshot and the directory it
//! describes, and rows with a fixed set of columns. Each column has a key,
//! which names it in JSON and in the TSV header, and a title for tables.
//!
//! - JSON prints `format_version`, `command`, `profile`, the context, and
//!   the rows under `rows`.
//! - TSV prints a header of column keys and one line per row.
//! - A table prints a heading, the summary lines, the rows under their
//!   titles, and notes.
//!
//! Some columns appear only in tables, such as a bar chart or a formatted
//! date, and some only in TSV and JSON, such as raw epochs.

use serde_json::{Map, Value};

use crate::render::{Align, Cell, Format, KeyValue, Paint, Renderer, TableColumn};

/// The version of the JSON and TSV layout. It changes when a key is removed
/// or changes its meaning; new keys may appear without a change.
pub const FORMAT_VERSION: u64 = 1;

/// Where a column appears.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Shown {
    Everywhere,
    TableOnly,
    DataOnly,
}

/// One column of a report.
#[derive(Clone, Copy, Debug)]
pub struct Column {
    key: &'static str,
    title: &'static str,
    align: Align,
    shown: Shown,
    shrink: bool,
}

impl Column {
    /// A column in every format.
    pub const fn new(key: &'static str, title: &'static str, align: Align) -> Self {
        Column {
            key,
            title,
            align,
            shown: Shown::Everywhere,
            shrink: false,
        }
    }

    /// A path column, which a table may shorten in the middle to fit a
    /// terminal.
    pub const fn path(key: &'static str, title: &'static str) -> Self {
        Column {
            key,
            title,
            align: Align::Left,
            shown: Shown::Everywhere,
            shrink: true,
        }
    }

    /// A column that only tables show.
    pub const fn table(title: &'static str, align: Align) -> Self {
        Column {
            key: "",
            title,
            align,
            shown: Shown::TableOnly,
            shrink: false,
        }
    }

    /// A column that only TSV and JSON show.
    pub const fn data(key: &'static str) -> Self {
        Column {
            key,
            title: "",
            align: Align::Left,
            shown: Shown::DataOnly,
            shrink: false,
        }
    }

    fn in_table(&self) -> bool {
        self.shown != Shown::DataOnly
    }

    fn in_data(&self) -> bool {
        self.shown != Shown::TableOnly
    }
}

/// One cell: the value that TSV and JSON hold, and the text and color that
/// a table shows.
#[derive(Clone, Debug)]
pub struct Field {
    value: Value,
    text: String,
    paint: Option<Paint>,
}

impl Field {
    pub fn new(value: impl Into<Value>, text: impl Into<String>) -> Self {
        Field {
            value: value.into(),
            text: text.into(),
            paint: None,
        }
    }

    /// Text that every format shows as it is.
    pub fn text(text: impl Into<String>) -> Self {
        let text = text.into();
        Field::new(text.clone(), text)
    }

    /// A value for a column that only TSV and JSON show.
    pub fn data(value: impl Into<Value>) -> Self {
        Field::new(value, "")
    }

    /// Text for a column that only tables show.
    pub fn shown(text: impl Into<String>) -> Self {
        Field::new(Value::Null, text)
    }

    /// The same field in a color, where tables use color.
    pub fn painted(mut self, paint: Option<Paint>) -> Self {
        self.paint = paint;
        self
    }
}

/// The rows of a table with their columns.
pub struct Table {
    heading: Option<String>,
    columns: Vec<Column>,
    rows: Vec<Vec<Field>>,
    empty: &'static str,
}

impl Table {
    pub fn new(columns: Vec<Column>) -> Self {
        Table {
            heading: None,
            columns,
            rows: Vec::new(),
            empty: "No matching entries.",
        }
    }

    pub fn heading(mut self, heading: impl Into<String>) -> Self {
        self.heading = Some(heading.into());
        self
    }

    pub fn empty(mut self, message: &'static str) -> Self {
        self.empty = message;
        self
    }

    pub fn push(&mut self, row: Vec<Field>) {
        debug_assert_eq!(row.len(), self.columns.len());
        self.rows.push(row);
    }

    fn render(&self, renderer: &mut Renderer) {
        if let Some(heading) = &self.heading {
            renderer.heading(heading);
        }
        let shown: Vec<usize> = (0..self.columns.len())
            .filter(|&index| self.columns[index].in_table())
            .collect();
        let columns: Vec<TableColumn> = shown
            .iter()
            .map(|&index| {
                let column = &self.columns[index];
                TableColumn {
                    title: column.title,
                    align: column.align,
                    shrink: column.shrink,
                }
            })
            .collect();
        let rows: Vec<Vec<Cell>> = self
            .rows
            .iter()
            .map(|row| {
                shown
                    .iter()
                    .map(|&index| Cell::painted(row[index].text.clone(), row[index].paint))
                    .collect()
            })
            .collect();
        renderer.table_with(&columns, &rows, self.empty);
    }

    fn data_columns(&self) -> impl Iterator<Item = (usize, &Column)> {
        self.columns
            .iter()
            .enumerate()
            .filter(|(_, column)| column.in_data())
    }

    fn json_rows(&self) -> Value {
        Value::Array(
            self.rows
                .iter()
                .map(|row| {
                    Value::Object(
                        self.data_columns()
                            .map(|(index, column)| {
                                (column.key.to_string(), row[index].value.clone())
                            })
                            .collect(),
                    )
                })
                .collect(),
        )
    }

    fn tsv(&self, renderer: &mut Renderer) {
        let headers: Vec<&str> = self.data_columns().map(|(_, column)| column.key).collect();
        let rows: Vec<Vec<String>> = self
            .rows
            .iter()
            .map(|row| {
                self.data_columns()
                    .map(|(index, _)| tsv_text(&row[index].value))
                    .collect()
            })
            .collect();
        renderer.tsv(&headers, &rows);
    }
}

/// A value as TSV text: numbers and booleans as JSON writes them, text as
/// it is, and nothing for null.
fn tsv_text(value: &Value) -> String {
    match value {
        Value::Null => String::new(),
        Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}

/// A command's output.
pub struct Report {
    command: &'static str,
    profile: Option<String>,
    context: Map<String, Value>,
    heading: Option<String>,
    summary: Vec<(&'static str, KeyValue)>,
    /// Tables that a table shows between the summary and the rows.
    before: Vec<Table>,
    tables: Vec<Table>,
    /// The rows that TSV and JSON print; tables may lay them out otherwise.
    data: Option<Table>,
    /// Whether the rows are only for TSV, as when JSON and tables show
    /// them in the context and the summary.
    tsv_only: bool,
    notes: Vec<String>,
}

impl Report {
    pub fn new(command: &'static str, profile: Option<&str>) -> Self {
        Report {
            command,
            profile: profile.map(str::to_string),
            context: Map::new(),
            heading: None,
            summary: Vec::new(),
            before: Vec::new(),
            tables: Vec::new(),
            data: None,
            tsv_only: false,
            notes: Vec::new(),
        }
    }

    /// Add a context value, which JSON prints before the rows.
    pub fn context(&mut self, key: &str, value: impl Into<Value>) {
        self.context.insert(key.to_string(), value.into());
    }

    /// Set the heading of a table.
    pub fn heading(&mut self, heading: impl Into<String>) {
        self.heading = Some(heading.into());
    }

    /// Add a line to the summary that a table shows above its rows.
    pub fn summary(&mut self, label: &'static str, value: impl Into<KeyValue>) {
        self.summary.push((label, value.into()));
    }

    /// Add a table that a table shows above the rows, such as a status.
    pub fn table_before(&mut self, table: Table) {
        self.before.push(table);
    }

    /// Set the rows of every format.
    pub fn rows(&mut self, table: Table) {
        self.data = Some(table);
    }

    /// Set rows that only TSV prints.
    pub fn rows_for_tsv(&mut self, table: Table) {
        self.data = Some(table);
        self.tsv_only = true;
    }

    /// Set the rows of TSV and JSON, and lay them out as `tables` instead.
    pub fn rows_shown_as(&mut self, data: Table, tables: Vec<Table>) {
        self.data = Some(data);
        self.tables = tables;
    }

    /// Add a line that a table shows below its rows.
    pub fn note(&mut self, text: impl Into<String>) {
        self.notes.push(text.into());
    }

    /// Print the report in the renderer's format.
    pub fn render(self, renderer: &mut Renderer) {
        match renderer.format {
            Format::Json => {
                let mut object = Map::new();
                object.insert("format_version".into(), FORMAT_VERSION.into());
                object.insert("command".into(), self.command.into());
                object.insert(
                    "profile".into(),
                    self.profile.map_or(Value::Null, Value::String),
                );
                object.extend(self.context);
                if let Some(data) = self.data.as_ref().filter(|_| !self.tsv_only) {
                    object.insert("rows".into(), data.json_rows());
                }
                renderer.json(&Value::Object(object));
            }
            Format::Tsv => match &self.data {
                Some(data) => data.tsv(renderer),
                None => {
                    // A report without rows prints its summary as rows.
                    let rows: Vec<Vec<String>> = self
                        .summary
                        .iter()
                        .map(|(label, value)| vec![(*label).to_string(), key_value_text(value)])
                        .collect();
                    renderer.tsv(&["name", "value"], &rows);
                }
            },
            Format::Table => {
                if let Some(heading) = &self.heading {
                    renderer.heading(heading);
                }
                let rows = self.data.as_ref().filter(|_| !self.tsv_only);
                if !self.summary.is_empty() {
                    renderer.key_values(&self.summary);
                    if rows.is_some() || !self.tables.is_empty() || !self.before.is_empty() {
                        renderer.message("");
                    }
                }
                for table in &self.before {
                    table.render(renderer);
                    renderer.message("");
                }
                if self.tables.is_empty() {
                    if let Some(rows) = rows {
                        rows.render(renderer);
                    }
                } else {
                    for (index, table) in self.tables.iter().enumerate() {
                        if index > 0 {
                            renderer.message("");
                        }
                        table.render(renderer);
                    }
                }
                for note in &self.notes {
                    renderer.message(note);
                }
            }
        }
    }
}

fn key_value_text(value: &KeyValue) -> String {
    match value {
        KeyValue::One(text) => text.clone(),
        KeyValue::Many(items) => items.join("\n"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn sample() -> Table {
        let mut table = Table::new(vec![
            Column::new("rank", "Rank", Align::Right),
            Column::table("Bar", Align::Left),
            Column::data("epoch"),
            Column::path("path", "Path"),
        ]);
        table.push(vec![
            Field::new(1, "1"),
            Field::shown("███"),
            Field::data(1.5),
            Field::text("a\tb"),
        ]);
        table
    }

    #[test]
    fn data_rows_hold_the_data_columns_in_order() {
        let rows = sample().json_rows();
        assert_eq!(rows, json!([{"rank": 1, "epoch": 1.5, "path": "a\tb"}]));
        let keys: Vec<&str> = rows[0]
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(keys, ["rank", "epoch", "path"]);
    }

    #[test]
    fn tsv_text_writes_values_plainly() {
        assert_eq!(tsv_text(&Value::Null), "");
        assert_eq!(tsv_text(&json!(12)), "12");
        assert_eq!(tsv_text(&json!(0.25)), "0.25");
        assert_eq!(tsv_text(&json!(true)), "true");
        assert_eq!(tsv_text(&json!("x")), "x");
    }
}
