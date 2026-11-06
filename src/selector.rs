//! The ways a command names a snapshot of its profile.

use crate::model::{ProfileIndex, SnapshotRecord};
use crate::sys;
use crate::units::{format_age, format_local_time, parse_duration};

/// A snapshot as named on the command line.
#[derive(Clone, Debug, PartialEq)]
pub enum Selector {
    /// The newest complete snapshot: `latest`.
    Latest,
    /// The newest snapshot, complete or not: `latest-any`.
    LatestAny,
    /// The Nth complete snapshot before the newest one: `latest~N`, and
    /// `previous` for 1.
    Back(usize),
    /// The snapshot with a label: `@NAME`.
    Label(String),
    /// The newest complete snapshot at least this many seconds old: `30d`.
    Age(u64),
    /// The newest complete snapshot taken on or before a local date:
    /// `2026-01-31`. Holds the end of that day.
    Date { text: String, end_epoch: i64 },
    /// A snapshot ID, a snapshot file name, or a unique prefix of an ID.
    Id(String),
}

impl Selector {
    pub fn parse(text: &str) -> Result<Self, String> {
        let text = text.trim();
        Ok(match text {
            "" => return Err("name a snapshot, such as latest".into()),
            "latest" => Selector::Latest,
            "latest-any" => Selector::LatestAny,
            "previous" => Selector::Back(1),
            _ => {
                if let Some(count) = text.strip_prefix("latest~") {
                    let count = count
                        .parse()
                        .map_err(|_| format!("use latest~N with a number N, not {text}"))?;
                    Selector::Back(count)
                } else if let Some(label) = text.strip_prefix('@') {
                    validate_label(label)?;
                    Selector::Label(label.to_string())
                } else if let Some(end_epoch) = date_end(text)? {
                    Selector::Date {
                        text: text.to_string(),
                        end_epoch,
                    }
                } else if is_duration(text) {
                    Selector::Age(parse_duration(text).map_err(|error| error.to_string())?)
                } else {
                    Selector::Id(text.to_string())
                }
            }
        })
    }

    /// Whether the selector names the newest complete snapshot, which a newer
    /// incomplete one may follow.
    pub fn is_latest(&self) -> bool {
        matches!(self, Selector::Latest)
    }

    /// The selected record. `include_incomplete` makes `latest~N` count
    /// every snapshot back from the newest one.
    pub fn select(
        &self,
        index: &ProfileIndex,
        include_incomplete: bool,
    ) -> Result<SnapshotRecord, String> {
        if index.snapshots.is_empty() {
            return Err("no snapshots are available; run 'qdu snapshot' first".into());
        }
        let complete = |record: &&SnapshotRecord| record.complete;
        let by_id = |id: &str| {
            index
                .snapshots
                .iter()
                .find(|record| record.snapshot_id == id)
        };
        match self {
            Selector::Latest => index
                .latest_complete
                .as_deref()
                .and_then(by_id)
                .cloned()
                .ok_or_else(|| {
                    "no complete snapshot is available; select an incomplete one with latest-any"
                        .into()
                }),
            Selector::LatestAny => index
                .latest_any
                .as_deref()
                .and_then(by_id)
                .cloned()
                .ok_or_else(|| "no snapshot is available".into()),
            Selector::Back(count) => {
                let candidates: Vec<&SnapshotRecord> = if include_incomplete {
                    index.snapshots.iter().rev().collect()
                } else {
                    index.snapshots.iter().rev().filter(complete).collect()
                };
                candidates.get(*count).map(|record| (*record).clone()).ok_or_else(|| {
                    let kind = if include_incomplete { "" } else { "complete " };
                    match candidates.len() {
                        0 => format!("no {kind}snapshot is available"),
                        1 => format!("only one {kind}snapshot is available"),
                        found => format!(
                            "only {found} {kind}snapshots are available, so latest~{count} does not exist"
                        ),
                    }
                })
            }
            Selector::Label(label) => index
                .snapshots
                .iter()
                .find(|record| record.labels.iter().any(|item| item == label))
                .cloned()
                .ok_or_else(|| {
                    let mut labels: Vec<&str> = index
                        .snapshots
                        .iter()
                        .flat_map(|record| record.labels.iter().map(String::as_str))
                        .collect();
                    labels.sort_unstable();
                    if labels.is_empty() {
                        format!("no snapshot is labeled {label}; none has a label")
                    } else {
                        format!(
                            "no snapshot is labeled {label}; labels: {}",
                            labels.join(", ")
                        )
                    }
                }),
            Selector::Age(seconds) => {
                let cutoff =
                    sys::now_epoch().saturating_sub(i64::try_from(*seconds).unwrap_or(i64::MAX));
                newest_until(index, cutoff).ok_or_else(|| {
                    format!(
                        "no complete snapshot is {} old or older",
                        format_age(i64::try_from(*seconds).unwrap_or(i64::MAX))
                    ) + &oldest_note(index)
                })
            }
            Selector::Date { text, end_epoch } => {
                newest_until(index, *end_epoch).ok_or_else(|| {
                    format!("no complete snapshot was taken on or before {text}")
                        + &oldest_note(index)
                })
            }
            Selector::Id(text) => {
                if let Some(record) = index
                    .snapshots
                    .iter()
                    .find(|record| record.snapshot_id == *text || record.filename == *text)
                {
                    return Ok(record.clone());
                }
                let matching: Vec<&SnapshotRecord> = index
                    .snapshots
                    .iter()
                    .filter(|record| record.snapshot_id.starts_with(text.as_str()))
                    .collect();
                match matching.as_slice() {
                    [record] => Ok((*record).clone()),
                    [] => Err(format!("snapshot not found: {text}")),
                    _ => Err(format!("snapshot selector is ambiguous: {text}")),
                }
            }
        }
    }
}

/// The newest complete snapshot taken at or before an epoch.
fn newest_until(index: &ProfileIndex, epoch: i64) -> Option<SnapshotRecord> {
    index
        .snapshots
        .iter()
        .rev()
        .find(|record| record.complete && record.created_epoch <= epoch)
        .cloned()
}

fn oldest_note(index: &ProfileIndex) -> String {
    index
        .snapshots
        .iter()
        .find(|record| record.complete)
        .map(|record| {
            format!(
                "; the oldest is from {}",
                format_local_time(record.created_epoch as f64)
            )
        })
        .unwrap_or_default()
}

fn is_duration(text: &str) -> bool {
    let Some(last) = text.chars().last() else {
        return false;
    };
    let number = &text[..text.len() - last.len_utf8()];
    "smhdwSMHDW".contains(last)
        && !number.is_empty()
        && number
            .chars()
            .all(|character| character.is_ascii_digit() || character == '.')
}

/// The last second of a local `YYYY-MM-DD` date, or `None` for text of
/// another shape.
fn date_end(text: &str) -> Result<Option<i64>, String> {
    let bytes = text.as_bytes();
    let shaped = bytes.len() == 10
        && bytes[4] == b'-'
        && bytes[7] == b'-'
        && bytes
            .iter()
            .enumerate()
            .all(|(index, byte)| index == 4 || index == 7 || byte.is_ascii_digit());
    if !shaped {
        return Ok(None);
    }
    let invalid = || format!("{text} is not a valid date");
    let year: i32 = text[..4].parse().map_err(|_| invalid())?;
    let month: u32 = text[5..7].parse().map_err(|_| invalid())?;
    let day: u32 = text[8..].parse().map_err(|_| invalid())?;
    let days_in_month = match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if (year % 4 == 0 && year % 100 != 0) || year % 400 == 0 => 29,
        2 => 28,
        _ => return Err(invalid()),
    };
    if day == 0 || day > days_in_month {
        return Err(invalid());
    }
    // The next day's local midnight, one second earlier; mktime normalizes
    // the day after the last of a month.
    sys::local_epoch(year, month, day + 1)
        .map(|midnight| Some(midnight - 1))
        .ok_or_else(invalid)
}

/// Labels follow the rules of profile names: letters, digits, `.`, `_`,
/// and `-`, starting with a letter or digit, at most 64 characters.
pub fn validate_label(label: &str) -> Result<(), String> {
    let mut characters = label.chars();
    let valid = label.len() <= 64
        && characters
            .next()
            .is_some_and(|first| first.is_ascii_alphanumeric())
        && characters.all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '.' | '_' | '-')
        });
    if valid {
        Ok(())
    } else {
        Err(format!(
            "invalid label {label:?}: use up to 64 letters, digits, '.', '_', or '-', \
             starting with a letter or digit"
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selectors_parse_by_their_shape() {
        let parse = |text| Selector::parse(text).unwrap();
        assert_eq!(parse("latest"), Selector::Latest);
        assert_eq!(parse("latest-any"), Selector::LatestAny);
        assert_eq!(parse("previous"), Selector::Back(1));
        assert_eq!(parse("latest~3"), Selector::Back(3));
        assert_eq!(
            parse("@before-cleanup"),
            Selector::Label("before-cleanup".into())
        );
        assert_eq!(parse("30d"), Selector::Age(30 * 86_400));
        assert_eq!(parse("2w"), Selector::Age(14 * 86_400));
        assert!(matches!(parse("2026-01-31"), Selector::Date { .. }));
        assert_eq!(
            parse("20261102T235238Z"),
            Selector::Id("20261102T235238Z".into())
        );
        assert!(Selector::parse("latest~x").is_err());
        assert!(Selector::parse("2026-02-30").is_err());
        assert!(Selector::parse("@").is_err());
    }
}
