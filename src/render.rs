//! Render results as tables, TSV, JSON, and diagnostics.

use std::io::{self, Write};

use serde_json::Value;
use unicode_width::UnicodeWidthChar;

use crate::sys;

/// Output format chosen with `--format`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
pub enum Format {
    Table,
    Tsv,
    Json,
}

/// Table drawing chosen with `--style`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
pub enum Style {
    Auto,
    Rich,
    Plain,
}

/// Color policy chosen with `--color`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
pub enum ColorChoice {
    Auto,
    Always,
    Never,
}

/// Cell alignment in a table column.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Align {
    Left,
    Right,
    Center,
}

/// A palette style for one table cell.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Paint {
    Red,
    Green,
    Yellow,
    Blue,
    Orange,
}

impl Paint {
    fn code(self) -> &'static str {
        match self {
            Paint::Red => "\x1b[31m",
            Paint::Green => "\x1b[32m",
            Paint::Yellow => "\x1b[33m",
            Paint::Blue => "\x1b[34m",
            Paint::Orange => "\x1b[38;5;208m",
        }
    }
}

/// How a table column is laid out.
#[derive(Clone, Copy, Debug)]
pub struct TableColumn<'a> {
    pub title: &'a str,
    pub align: Align,
    /// Whether the column may be shortened in the middle to fit a terminal,
    /// as a path can; identifiers and numbers are never shortened.
    pub shrink: bool,
}

/// A table cell: display text and an optional style.
#[derive(Clone, Debug)]
pub struct Cell {
    pub text: String,
    pub paint: Option<Paint>,
}

impl Cell {
    pub fn painted(text: impl Into<String>, paint: Option<Paint>) -> Self {
        Cell {
            text: text.into(),
            paint,
        }
    }
}

impl<T: Into<String>> From<T> for Cell {
    fn from(text: T) -> Self {
        Cell {
            text: text.into(),
            paint: None,
        }
    }
}

/// Writes deterministic machine output or terminal-friendly reports.
pub struct Renderer {
    pub format: Format,
    rich: bool,
    color: bool,
    /// Whether warnings on standard error are colored.
    warning_color: bool,
    /// Whether standard output is a terminal, whose width tables fit.
    terminal: bool,
    out: io::BufWriter<io::Stdout>,
}

const RESET: &str = "\x1b[0m";
const BOLD: &str = "\x1b[1m";

fn locale_is_utf8() -> bool {
    for variable in ["LC_ALL", "LC_CTYPE", "LANG"] {
        if let Ok(value) = std::env::var(variable) {
            if !value.is_empty() {
                let value = value.to_ascii_lowercase();
                return value.contains("utf-8") || value.contains("utf8") || value == "c.utf8";
            }
        }
    }
    true
}

impl Renderer {
    pub fn new(format: Format, style: Style, color: ColorChoice) -> Self {
        let terminal = sys::is_terminal(libc::STDOUT_FILENO);
        let rich = format == Format::Table
            && match style {
                Style::Rich => true,
                Style::Plain => false,
                Style::Auto => terminal && locale_is_utf8(),
            };
        // https://no-color.org: a non-empty NO_COLOR turns automatic color off.
        let colored = |terminal: bool| match color {
            ColorChoice::Always => true,
            ColorChoice::Never => false,
            ColorChoice::Auto => {
                terminal && std::env::var_os("NO_COLOR").is_none_or(|value| value.is_empty())
            }
        };
        Renderer {
            format,
            rich,
            color: colored(terminal),
            warning_color: colored(sys::is_terminal(libc::STDERR_FILENO)),
            terminal,
            out: io::BufWriter::new(io::stdout()),
        }
    }

    fn write(&mut self, text: &str) {
        // A closed pipe, as in `qdu show | head`, simply ends the output.
        let _ = self.out.write_all(text.as_bytes());
    }

    pub fn flush(&mut self) {
        let _ = self.out.flush();
    }

    /// Write one pretty-printed JSON value.
    pub fn json(&mut self, value: &Value) {
        let text = serde_json::to_string_pretty(value).unwrap_or_default();
        self.write(&text);
        self.write("\n");
    }

    /// Write escaped tab-separated headers and rows.
    pub fn tsv(&mut self, headers: &[&str], rows: &[Vec<String>]) {
        let mut text = headers.join("\t");
        text.push('\n');
        for row in rows {
            let values: Vec<String> = row.iter().map(|value| tsv_value(value)).collect();
            text.push_str(&values.join("\t"));
            text.push('\n');
        }
        self.write(&text);
    }

    /// Write a plain line.
    pub fn message(&mut self, text: &str) {
        self.write(text);
        self.write("\n");
    }

    fn paint(&self, text: &str, paint: Option<Paint>) -> String {
        match paint {
            Some(paint) if self.color => format!("{}{text}{RESET}", paint.code()),
            _ => text.to_string(),
        }
    }

    /// Write a note to standard error, such as why a snapshot was chosen.
    pub fn note(&mut self, text: &str) {
        self.flush();
        let _ = writeln!(io::stderr(), "qdu: note: {}", display_safe(text));
    }

    /// Write a warning to standard error.
    pub fn warning(&mut self, text: &str) {
        self.flush();
        let text = format!("qdu: warning: {}", display_safe(text));
        let line = if self.warning_color {
            format!("{}{text}{RESET}", Paint::Yellow.code())
        } else {
            text
        };
        let _ = writeln!(io::stderr(), "{line}");
    }

    /// Write a heading, bold only on a rich terminal.
    pub fn heading(&mut self, text: &str) {
        if self.rich && self.color {
            let line = format!("{BOLD}{text}{RESET}\n");
            self.write(&line);
        } else {
            self.message(text);
        }
    }

    /// Write aligned label-value lines; a list value takes a line per item.
    pub fn key_values(&mut self, values: &[(&str, KeyValue)]) {
        let Some(width) = values.iter().map(|(label, _)| display_width(label)).max() else {
            return;
        };
        let mut text = String::new();
        for (label, value) in values {
            let items: Vec<&str> = match value {
                KeyValue::One(item) => vec![item.as_str()],
                KeyValue::Many(items) if items.is_empty() => vec![""],
                KeyValue::Many(items) => items.iter().map(String::as_str).collect(),
            };
            for (index, item) in items.iter().enumerate() {
                let head = if index == 0 {
                    pad(label, width)
                } else {
                    " ".repeat(width)
                };
                let item = display_safe(item);
                if item.is_empty() {
                    text.push_str(head.trim_end());
                } else {
                    text.push_str(&head);
                    text.push_str("  ");
                    text.push_str(&item);
                }
                text.push('\n');
            }
        }
        self.write(&text);
    }

    /// Write a table. On a terminal that is too narrow, the columns that may
    /// shrink lose their middle; other output is never shortened.
    pub fn table_with(&mut self, columns: &[TableColumn], rows: &[Vec<Cell>], empty_message: &str) {
        if rows.is_empty() {
            self.message(empty_message);
            return;
        }
        let rows: Vec<Vec<Cell>> = rows
            .iter()
            .map(|row| {
                row.iter()
                    .map(|cell| Cell::painted(display_safe(&cell.text), cell.paint))
                    .collect()
            })
            .collect();
        let mut widths: Vec<usize> = columns
            .iter()
            .map(|column| display_width(column.title))
            .collect();
        for row in &rows {
            for (index, cell) in row.iter().enumerate() {
                widths[index] = widths[index].max(display_width(&cell.text));
            }
        }
        if self.terminal {
            let overhead = if self.rich {
                3 * columns.len() + 1
            } else {
                2 * columns.len().saturating_sub(1)
            };
            let total: usize = widths.iter().sum::<usize>() + overhead;
            shrink_to_fit(
                &mut widths,
                columns,
                total.saturating_sub(sys::terminal_columns(120)),
            );
        }
        if self.rich {
            self.rich_table(columns, &rows, &widths);
        } else {
            self.plain_table(columns, &rows, &widths);
        }
    }

    fn rich_table(&mut self, columns: &[TableColumn], rows: &[Vec<Cell>], widths: &[usize]) {
        let rule = |left: &str, middle: &str, right: &str| -> String {
            let parts: Vec<String> = widths.iter().map(|width| "─".repeat(width + 2)).collect();
            format!("{left}{}{right}\n", parts.join(middle))
        };
        let mut text = rule("┌", "┬", "┐");
        let (bold, reset) = if self.color { (BOLD, RESET) } else { ("", "") };
        text.push('│');
        let header_cells: Vec<String> = columns
            .iter()
            .zip(widths)
            .map(|(column, width)| {
                format!(
                    " {bold}{}{reset} ",
                    fit(column.title, *width, Align::Center)
                )
            })
            .collect();
        text.push_str(&header_cells.join("│"));
        text.push_str("│\n");
        text.push_str(&rule("├", "┼", "┤"));
        for row in rows {
            let cells: Vec<String> = row
                .iter()
                .zip(widths)
                .zip(columns)
                .map(|((cell, width), column)| {
                    format!(
                        " {} ",
                        self.paint(&fit_column(&cell.text, *width, column), cell.paint)
                    )
                })
                .collect();
            text.push('│');
            text.push_str(&cells.join("│"));
            text.push_str("│\n");
        }
        text.push_str(&rule("└", "┴", "┘"));
        self.write(&text);
    }

    fn plain_table(&mut self, columns: &[TableColumn], rows: &[Vec<Cell>], widths: &[usize]) {
        // Headers line up with their column, as in df or ps.
        let header_cells: Vec<Cell> = columns
            .iter()
            .map(|column| Cell::from(column.title))
            .collect();
        let mut text = self.plain_line(&header_cells, widths, columns);
        let rules: Vec<String> = widths.iter().map(|width| "-".repeat(*width)).collect();
        text.push_str(&rules.join("  "));
        text.push('\n');
        for row in rows {
            text.push_str(&self.plain_line(row, widths, columns));
        }
        self.write(&text);
    }

    fn plain_line(&self, cells: &[Cell], widths: &[usize], columns: &[TableColumn]) -> String {
        let last = widths.len().saturating_sub(1);
        let parts: Vec<String> = cells
            .iter()
            .zip(widths)
            .zip(columns)
            .enumerate()
            .map(|(index, ((cell, width), column))| {
                let mut text = fit_column(&cell.text, *width, column);
                // An unpadded last column leaves no trailing blanks.
                if index == last && column.align == Align::Left {
                    text.truncate(text.trim_end_matches(' ').len());
                }
                self.paint(&text, cell.paint)
            })
            .collect();
        let mut line = parts.join("  ");
        line.push('\n');
        line
    }
}

impl Drop for Renderer {
    fn drop(&mut self) {
        self.flush();
    }
}

/// A value in a label-value list.
pub enum KeyValue {
    One(String),
    Many(Vec<String>),
}

impl From<String> for KeyValue {
    fn from(value: String) -> Self {
        KeyValue::One(value)
    }
}

impl From<&str> for KeyValue {
    fn from(value: &str) -> Self {
        KeyValue::One(value.to_string())
    }
}

/// Redraw one status line in place on standard error.
pub struct ProgressLine {
    visible: bool,
}

impl ProgressLine {
    pub fn new() -> Self {
        ProgressLine { visible: false }
    }

    /// Show `text`, then as much of the end of `detail` as fits.
    pub fn update(&mut self, text: &str, detail: &str) {
        // One column stays free so the cursor never wraps to a new line.
        let width = sys::terminal_columns_of(libc::STDERR_FILENO, 80).saturating_sub(1);
        let mut stderr = io::stderr();
        let _ = write!(stderr, "\r{}\x1b[K", progress_line(text, detail, width));
        let _ = stderr.flush();
        self.visible = true;
    }

    /// Erase the status line if one is shown.
    pub fn clear(&mut self) {
        if self.visible {
            let mut stderr = io::stderr();
            let _ = write!(stderr, "\r\x1b[K");
            let _ = stderr.flush();
            self.visible = false;
        }
    }
}

impl Default for ProgressLine {
    fn default() -> Self {
        Self::new()
    }
}

/// `text`, then as much of the end of `detail` as fits in `width` columns.
fn progress_line(text: &str, detail: &str, width: usize) -> String {
    let width = width.max(20);
    let mut line = display_safe(text);
    if !detail.is_empty() {
        let room = width as isize - display_width(&line) as isize - 2;
        if room >= 8 {
            let tail = truncate_left(&display_safe(detail), room as usize);
            line = format!("{line}: {tail}");
        }
    }
    truncate(&line, width)
}

fn truncate_left(value: &str, width: usize) -> String {
    if display_width(value) <= width {
        return value.to_string();
    }
    let reversed: String = value.chars().rev().collect();
    truncate(&reversed, width).chars().rev().collect()
}

fn tsv_value(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('\t', "\\t")
        .replace('\r', "\\r")
        .replace('\n', "\\n")
}

/// Escape characters that would corrupt a terminal or a table: C0 and C1
/// control characters, which can start escape sequences, and the controls
/// that reorder bidirectional text, which can make a name read as another.
pub fn display_safe(value: &str) -> String {
    let mut result = String::with_capacity(value.len());
    for character in value.chars() {
        match character {
            '\n' => result.push_str("\\n"),
            '\r' => result.push_str("\\r"),
            '\t' => result.push_str("\\t"),
            character if character.is_control() || is_bidi_control(character) => {
                result.push_str(&format!("\\u{:04x}", character as u32));
            }
            character => result.push(character),
        }
    }
    result
}

fn is_bidi_control(character: char) -> bool {
    matches!(
        character,
        '\u{061c}' | '\u{200e}' | '\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}'
    )
}

fn char_width(character: char) -> usize {
    if character.is_control() {
        return 0;
    }
    character.width().unwrap_or(0)
}

/// Terminal columns, ignoring ANSI sequences and zero-width characters.
pub fn display_width(value: &str) -> usize {
    let mut width = 0;
    let mut characters = value.chars().peekable();
    while let Some(character) = characters.next() {
        if character == '\x1b' && characters.peek() == Some(&'[') {
            characters.next();
            for following in characters.by_ref() {
                if following.is_ascii_alphabetic() {
                    break;
                }
            }
            continue;
        }
        width += char_width(character);
    }
    width
}

/// Fit text within terminal columns, ending with an ellipsis when shortened.
pub fn truncate(value: &str, width: usize) -> String {
    if display_width(value) <= width {
        return value.to_string();
    }
    if width <= 1 {
        return "…".chars().take(width).collect();
    }
    let target = width - 1;
    let mut used = 0;
    let mut result = String::new();
    for character in value.chars() {
        let character_width = char_width(character);
        if used + character_width > target {
            break;
        }
        result.push(character);
        used += character_width;
    }
    result.push('…');
    result
}

/// Narrow the columns that may shrink by `excess` columns in total, widest
/// first and none below 12.
fn shrink_to_fit(widths: &mut [usize], columns: &[TableColumn], mut excess: usize) {
    const MINIMUM: usize = 12;
    while excess > 0 {
        let widest = widths
            .iter()
            .enumerate()
            .filter(|(index, width)| columns[*index].shrink && **width > MINIMUM)
            .max_by_key(|(_, width)| **width)
            .map(|(index, _)| index);
        let Some(index) = widest else {
            return;
        };
        widths[index] -= 1;
        excess -= 1;
    }
}

/// Text fitted to a column: shortened in the middle if the column may
/// shrink, then padded.
fn fit_column(value: &str, width: usize, column: &TableColumn) -> String {
    if column.shrink {
        fit(&truncate_middle(value, width), width, column.align)
    } else {
        fit(value, width, column.align)
    }
}

/// Fit text within terminal columns by replacing its middle with an
/// ellipsis, keeping more of the end, where a path names its file.
pub fn truncate_middle(value: &str, width: usize) -> String {
    if display_width(value) <= width {
        return value.to_string();
    }
    if width <= 2 {
        return truncate(value, width);
    }
    let head_width = (width - 1) / 3;
    let head = truncate_plain(value, head_width);
    let tail = truncate_left_plain(value, width - 1 - display_width(&head));
    format!("{head}…{tail}")
}

/// The longest start of `value` within `width` columns.
fn truncate_plain(value: &str, width: usize) -> String {
    let mut used = 0;
    let mut result = String::new();
    for character in value.chars() {
        let character_width = char_width(character);
        if used + character_width > width {
            break;
        }
        result.push(character);
        used += character_width;
    }
    result
}

/// The longest end of `value` within `width` columns.
fn truncate_left_plain(value: &str, width: usize) -> String {
    let reversed: String = value.chars().rev().collect();
    truncate_plain(&reversed, width).chars().rev().collect()
}

/// Truncate and pad text to an exact display width.
pub fn fit(value: &str, width: usize, alignment: Align) -> String {
    let shortened = truncate(value, width);
    let missing = width.saturating_sub(display_width(&shortened));
    match alignment {
        Align::Right => format!("{}{shortened}", " ".repeat(missing)),
        Align::Center => {
            let left = missing / 2;
            format!(
                "{}{shortened}{}",
                " ".repeat(left),
                " ".repeat(missing - left)
            )
        }
        Align::Left => format!("{shortened}{}", " ".repeat(missing)),
    }
}

/// Pad text on the right to an exact display width.
pub fn pad(value: &str, width: usize) -> String {
    fit(value, width, Align::Left)
}

/// A clamped proportional block bar.
pub fn bar(numerator: i64, denominator: i64) -> String {
    const WIDTH: usize = 12;
    if denominator <= 0 {
        return "░".repeat(WIDTH);
    }
    let ratio = (numerator as f64 / denominator as f64).clamp(0.0, 1.0);
    let filled = ((ratio * WIDTH as f64).round_ties_even() as usize).min(WIDTH);
    format!("{}{}", "█".repeat(filled), "░".repeat(WIDTH - filled))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn widths_count_wide_characters_and_skip_escapes() {
        assert_eq!(display_width("abc"), 3);
        assert_eq!(display_width("日本"), 4);
        assert_eq!(display_width("\x1b[31mred\x1b[0m"), 3);
        assert_eq!(display_width("e\u{301}"), 1);
    }

    #[test]
    fn truncation_and_padding_respect_widths() {
        assert_eq!(truncate("abcdef", 4), "abc…");
        assert_eq!(truncate("日本語", 4), "日…");
        assert_eq!(fit("ab", 5, Align::Right), "   ab");
        assert_eq!(fit("ab", 5, Align::Center), " ab  ");
        assert_eq!(display_safe("a\tb\nc\u{1}"), "a\\tb\\nc\\u0001");
        assert_eq!(display_safe("x\u{9b}31m"), "x\\u009b31m");
        assert_eq!(display_safe("a\u{202e}b"), "a\\u202eb");
        assert_eq!(display_safe("日本"), "日本");
    }

    #[test]
    fn middle_truncation_keeps_both_ends() {
        assert_eq!(truncate_middle("short", 10), "short");
        let shown = truncate_middle("data/projects/alpha/checkpoints/run3", 20);
        assert_eq!(display_width(&shown), 20);
        assert!(shown.starts_with("data/p"), "{shown}");
        assert!(
            shown.ends_with("checkpoints/run3") || shown.ends_with("oints/run3"),
            "{shown}"
        );
        assert!(shown.contains('…'), "{shown}");
        assert_eq!(display_width(&truncate_middle("日本語のパス名です", 9)), 9);
    }

    #[test]
    fn shrinking_takes_from_the_widest_shrinkable_column() {
        let column = |shrink| TableColumn {
            title: "",
            align: Align::Left,
            shrink,
        };
        let columns = [column(false), column(true), column(true)];
        let mut widths = [40, 30, 20];
        shrink_to_fit(&mut widths, &columns, 15);
        assert_eq!(widths, [40, 18, 17]);
        let mut widths = [40, 14, 13];
        shrink_to_fit(&mut widths, &columns, 15);
        assert_eq!(widths, [40, 12, 12]);
    }

    #[test]
    fn progress_lines_keep_the_end_of_the_path() {
        let shown = progress_line(
            "Scanning 12 entries",
            "very/long/path/to/the/current/dir",
            40,
        );
        assert_eq!(display_width(&shown), 40);
        assert!(shown.starts_with("Scanning 12 entries: …"), "{shown}");
        assert!(shown.ends_with("current/dir"), "{shown}");
        assert_eq!(
            progress_line("Writing snapshot", "", 40),
            "Writing snapshot"
        );
        assert_eq!(progress_line("a\tb", "", 40), "a\\tb");
    }

    #[test]
    fn bars_round_half_to_even() {
        assert_eq!(bar(1, 8), "██░░░░░░░░░░");
        assert_eq!(bar(5, 0), "░".repeat(12));
        assert_eq!(bar(9, 4), "█".repeat(12));
    }
}
