//! Command implementations and the helpers they share.

pub mod browse;
pub mod check;
pub mod configuration;
pub mod diff;
pub mod doctor;
pub mod errors;
pub mod files;
pub mod forget;
pub mod history;
pub mod label;
pub mod list;
pub mod maintenance;
pub mod schedule;
pub mod show;
pub mod snapshot;
pub mod users;

use std::fs;
use std::path::PathBuf;

use crate::cli::{OutputOptions, TerminalOptions};
use crate::config::{ConfigRepository, ProfileConfig, ProfileOverrides};
use crate::error::{Result, usage};
use crate::model::{Metric, SnapshotRecord};
use crate::query::QueryService;
use crate::render::{Format, Paint, Renderer};
use crate::selector::Selector;
use crate::storage::{IndexRepository, ProfilePaths, profiles_with_state};
use crate::sys;
use crate::units::{format_age, format_bytes, format_local_time, format_size, group, group_signed};

/// Every profile that is configured or has a state directory, and
/// `default`, sorted.
pub fn known_profiles(config: &ConfigRepository) -> Result<Vec<String>> {
    let mut names = config.list_profiles()?;
    names.extend(profiles_with_state());
    names.sort();
    names.dedup();
    Ok(names)
}

/// Refuse a profile that is neither configured nor has a state directory,
/// so that a mistyped name does not scan the home directory or schedule a
/// snapshot of it; suggest the closest known name.
pub fn require_known_profile(config: &ConfigRepository, name: &str) -> Result<()> {
    if name == "default"
        || config.has_profile(name)?
        || ProfilePaths::for_profile(name)?.root.exists()
    {
        return Ok(());
    }
    let known = known_profiles(config)?;
    let closest = known
        .iter()
        .map(|known| (edit_distance(known, name), known))
        .filter(|(distance, _)| *distance <= 2)
        .min();
    let mut message = format!("unknown profile '{name}'");
    match closest {
        Some((_, closest)) => message.push_str(&format!("; did you mean '{closest}'?")),
        None => message.push('.'),
    }
    message.push_str(&format!(
        " Create it with 'qdu profile set {name} --path DIR' or 'qdu snapshot --profile {name} --path DIR' (known profiles: {})",
        known.join(", ")
    ));
    Err(usage(message))
}

/// The number of single-character insertions, deletions, and substitutions
/// that turn one name into the other.
fn edit_distance(left: &str, right: &str) -> usize {
    let right: Vec<char> = right.chars().collect();
    let mut previous: Vec<usize> = (0..=right.len()).collect();
    for (row, left_char) in left.chars().enumerate() {
        let mut current = vec![row + 1];
        for (column, right_char) in right.iter().enumerate() {
            let substitution = previous[column] + usize::from(left_char != *right_char);
            current.push(
                substitution
                    .min(previous[column + 1] + 1)
                    .min(current[column] + 1),
            );
        }
        previous = current;
    }
    previous[right.len()]
}

/// The snapshot a selector names. When `latest` passes over a newer
/// incomplete snapshot, a note says so, since the newest data is elsewhere.
pub fn select_snapshot(
    service: &QueryService,
    selector: &Selector,
    renderer: &mut Renderer,
) -> Result<SnapshotRecord> {
    let index = service.index.load()?;
    let record = selector.select(&index, false).map_err(usage)?;
    if selector.is_latest() {
        if let Some(newer) = index
            .latest_any
            .as_deref()
            .filter(|newer| *newer != record.snapshot_id)
        {
            renderer.note(&format!(
                "the newest snapshot, {newer}, is incomplete; this is the newest complete one. \
                 Select the other with --snapshot latest-any, and see 'qdu errors'."
            ));
        }
    }
    Ok(record)
}

/// A renderer for commands with `--format`.
pub fn renderer(output: &OutputOptions) -> Renderer {
    Renderer::new(output.format, output.terminal.style, output.terminal.color)
}

/// A renderer for commands that only print tables and messages.
pub fn terminal_renderer(terminal: &TerminalOptions) -> Renderer {
    Renderer::new(Format::Table, terminal.style, terminal.color)
}

/// Load a profile as qdu uses it.
///
/// A profile's snapshots are all of one directory. Without a configured
/// `path`, the profile therefore scans the directory of its newest snapshot,
/// so that `qdu snapshot --path DIR` once is enough for later runs.
pub fn effective_profile(config: &ConfigRepository, name: &str) -> Result<ProfileConfig> {
    let mut profile = config.load_profile(name)?;
    if !config.configures_path(name)? {
        // An unreadable index is reported where it matters, not here.
        if let Ok(index) = IndexRepository::new(ProfilePaths::for_profile(name)?).load() {
            let newest = index.latest_any.as_deref().and_then(|id| {
                index
                    .snapshots
                    .iter()
                    .find(|record| record.snapshot_id == id)
            });
            if let Some(record) = newest {
                profile.root = PathBuf::from(&record.root);
            }
        }
    }
    Ok(profile)
}

/// Load a profile and apply command-line settings.
///
/// `--exclude` and `--exclude-from` add to the profile's patterns.
pub fn load_profile(
    config: &ConfigRepository,
    name: &str,
    mut overrides: ProfileOverrides,
    exclude: &[String],
    exclude_from: &[PathBuf],
) -> Result<ProfileConfig> {
    let profile = effective_profile(config, name)?;
    let mut added: Vec<String> = exclude.to_vec();
    for path in exclude_from {
        let text = fs::read_to_string(path).map_err(|error| {
            usage(format!(
                "cannot read exclusion file {}: {error}",
                path.display()
            ))
        })?;
        added.extend(
            text.lines()
                .map(str::trim)
                .filter(|line| !line.is_empty() && !line.starts_with('#'))
                .map(str::to_string),
        );
    }
    if !added.is_empty() {
        let mut excludes = profile.excludes.clone();
        for pattern in added {
            if !excludes.contains(&pattern) {
                excludes.push(pattern);
            }
        }
        overrides.excludes = Some(excludes);
    }
    overrides.apply(&profile)
}

/// Write a count with the singular or plural form of its noun.
pub fn count(number: u64, singular: &str, plural: &str) -> String {
    format!(
        "{} {}",
        group(i128::from(number)),
        if number == 1 { singular } else { plural }
    )
}

/// A stored file size, or `missing` for a file that is gone.
pub fn format_stored(size: Option<u64>) -> String {
    size.map(format_size).unwrap_or_else(|| "missing".into())
}

/// Whether a snapshot is complete, and what it could not read.
pub fn status_text(record: &SnapshotRecord) -> String {
    let denied = (record.denied_count > 0).then(|| {
        format!(
            "{} denied permission",
            count(record.denied_count, "path", "paths")
        )
    });
    match (record.complete, denied) {
        (true, None) => "complete".to_string(),
        (true, Some(denied)) => format!("complete; {denied}"),
        (false, None) => format!(
            "incomplete; {} unreadable",
            count(record.error_count, "path", "paths")
        ),
        (false, Some(denied)) => format!(
            "incomplete; {} unreadable, {denied}",
            count(record.error_count, "path", "paths")
        ),
    }
}

/// When a snapshot was taken, followed by its ID.
pub fn snapshot_text(record: &SnapshotRecord) -> String {
    let age = (sys::now_epoch() - record.created_epoch).max(0);
    format!(
        "{} ({} ago)  {}",
        format_local_time(record.created_epoch as f64),
        format_age(age),
        record.snapshot_id
    )
}

/// A metric value: bytes in IEC units, or an inode count.
pub fn format_metric(value: i64, metric: Metric, signed: bool) -> String {
    match metric {
        Metric::Inodes if signed && value != 0 => group_signed(i128::from(value)),
        Metric::Inodes => group(i128::from(value)),
        _ => format_bytes(value, signed),
    }
}

/// The color of a status word such as OK, WARN, or FAILED.
pub fn status_paint(status: &str) -> Option<Paint> {
    match status {
        "OK" => Some(Paint::Green),
        "WARN" | "BUSY" => Some(Paint::Yellow),
        "FAILED" => Some(Paint::Red),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edit_distances_count_single_changes() {
        assert_eq!(edit_distance("data", "data"), 0);
        assert_eq!(edit_distance("data", "dta"), 1);
        assert_eq!(edit_distance("data", "date"), 1);
        assert_eq!(edit_distance("scratch", "scrach"), 1);
        assert_eq!(edit_distance("", "abc"), 3);
    }
}
