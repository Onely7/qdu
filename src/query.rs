//! Query snapshots through validated, domain-level operations.

use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap, HashSet};

use rusqlite::{Connection, OptionalExtension, Row, params};

use crate::error::{Error, Result, usage};
use crate::model::{
    DiffRecord, DirectoryRecord, FileRecord, Metric, OwnerDirectoryRecord, OwnerRecord,
    ScanErrorRecord, SnapshotRecord,
};
use crate::patterns::PathPattern;
use crate::selector::Selector;
use crate::storage::{
    IndexRepository, MaterializedSnapshot, ProfilePaths, READABLE_SCHEMA_VERSIONS,
    open_snapshot_database, snapshot_metadata,
};
use crate::sys;

const DIRECTORY_COLUMNS: &str = "path, depth, allocated_bytes, apparent_bytes, file_count, \
     directory_count, inode_count, latest_modified_epoch";

/// An open snapshot: its readable copy, connection, and metadata.
pub struct SnapshotContext {
    pub connection: Connection,
    pub metadata: HashMap<String, String>,
    // Keeps a temporary expansion of an archive alive while it is open.
    _materialized: MaterializedSnapshot,
}

impl SnapshotContext {
    pub fn metadata_value(&self, key: &str) -> Option<&str> {
        self.metadata.get(key).map(String::as_str)
    }

    pub fn has_owner_statistics(&self) -> bool {
        self.metadata_value("collect_users") == Some("true")
    }

    /// The first `limit` recorded scan failures, in the order of the scan.
    pub fn scan_errors(&self, limit: usize) -> Result<Vec<ScanErrorRecord>> {
        // Snapshots before schema 4 did not record the kind of a failure.
        let kind = match self.metadata_value("schema_version") {
            Some("2" | "3") => "'error'",
            _ => "kind",
        };
        let mut statement = self.connection.prepare(&format!(
            "SELECT path, operation, {kind}, message FROM scan_errors ORDER BY id LIMIT ?1"
        ))?;
        let records = statement
            .query_map([i64::try_from(limit).unwrap_or(i64::MAX)], |row| {
                Ok(ScanErrorRecord {
                    path: row.get(0)?,
                    operation: row.get(1)?,
                    kind: row.get(2)?,
                    message: row.get(3)?,
                })
            })?
            .collect::<rusqlite::Result<_>>()?;
        Ok(records)
    }
}

/// Resolve and open the snapshots of one profile.
pub struct QueryService {
    pub paths: ProfilePaths,
    pub index: IndexRepository,
}

impl QueryService {
    pub fn new(profile: &str) -> Result<Self> {
        let paths = ProfilePaths::for_profile(profile)?;
        Ok(QueryService {
            index: IndexRepository::new(paths.clone()),
            paths,
        })
    }

    pub fn resolve(&self, selector: &Selector) -> Result<SnapshotRecord> {
        self.index.resolve(selector, false)
    }

    pub fn previous_of(&self, record: &SnapshotRecord) -> Result<SnapshotRecord> {
        self.index.previous_of(record)
    }

    /// Open a snapshot for queries. A file that is not a snapshot database
    /// of a readable schema is a format error, which `qdu verify` and
    /// `qdu repair` explain.
    pub fn open(&self, record: &SnapshotRecord) -> Result<SnapshotContext> {
        let materialized = MaterializedSnapshot::open(&self.paths, record)?;
        let unreadable = |error: Error| {
            Error::Format(format!(
                "cannot read snapshot {}: {error}; run 'qdu verify' or 'qdu repair'",
                record.snapshot_id
            ))
        };
        let connection = open_snapshot_database(&materialized.path).map_err(unreadable)?;
        let metadata = snapshot_metadata(&connection).map_err(unreadable)?;
        let version = metadata.get("schema_version").map(String::as_str);
        if !version
            .and_then(|version| version.parse::<i64>().ok())
            .is_some_and(|version| READABLE_SCHEMA_VERSIONS.contains(&version))
        {
            return Err(Error::Format(format!(
                "snapshot {} has schema {}, which this qdu does not read",
                record.snapshot_id,
                version.unwrap_or("unknown")
            )));
        }
        Ok(SnapshotContext {
            connection,
            metadata,
            _materialized: materialized,
        })
    }
}

fn directory_from_row(row: &Row) -> rusqlite::Result<DirectoryRecord> {
    Ok(DirectoryRecord {
        path: row.get(0)?,
        depth: row.get(1)?,
        allocated_bytes: row.get(2)?,
        apparent_bytes: row.get(3)?,
        file_count: row.get(4)?,
        directory_count: row.get(5)?,
        inode_count: row.get(6)?,
        latest_modified_epoch: row.get(7)?,
    })
}

/// The stored record of one directory, if the snapshot has it.
pub fn directory(connection: &Connection, path: &str) -> Result<Option<DirectoryRecord>> {
    Ok(connection
        .query_row(
            &format!("SELECT {DIRECTORY_COLUMNS} FROM directories WHERE path = ?1"),
            [path],
            directory_from_row,
        )
        .optional()?)
}

/// The allocated bytes stored for one directory, if present.
pub fn directory_allocation(connection: &Connection, path: &str) -> Result<Option<i64>> {
    Ok(directory(connection, path)?.map(|record| record.allocated_bytes))
}

/// Stored paths compare as UTF-8 bytes, so every descendant of `a/b` sorts in
/// `["a/b/", "a/b0")`: `0` is the character right after `/`. Unlike LIKE,
/// this range is case-sensitive and treats `%` and `_` literally.
fn scope_bounds(scope: &str) -> (String, String) {
    (format!("{scope}/"), format!("{scope}0"))
}

/// Filters for a directory ranking.
pub struct RankingFilter<'a> {
    pub under: &'a str,
    pub max_depth: i64,
    pub top: usize,
    pub min_size: i64,
    pub pattern: Option<&'a PathPattern>,
    pub metric: Metric,
    pub modified_before_epoch: Option<f64>,
}

/// Return the scope record and its ranked descendants.
pub fn directory_ranking(
    connection: &Connection,
    filter: &RankingFilter,
) -> Result<(DirectoryRecord, Vec<DirectoryRecord>)> {
    let scope = normalize_relative_path(Some(filter.under))?;
    let scope_record = directory(connection, &scope)?.ok_or_else(|| {
        usage(format!(
            "directory is not present in snapshot: {}",
            filter.under
        ))
    })?;
    let column = filter.metric.column();
    let lower_depth = scope_record.depth;
    let upper_depth = scope_record.depth + filter.max_depth;
    let mut sql = format!("SELECT {DIRECTORY_COLUMNS} FROM directories WHERE ");
    if scope != "." {
        sql.push_str("path >= ?5 AND path < ?6 AND ");
    }
    sql.push_str(&format!(
        "depth > ?1 AND depth <= ?2 AND {column} >= ?3 \
         AND (?4 IS NULL OR latest_modified_epoch <= ?4) ORDER BY {column} DESC, path"
    ));
    let (lower, upper) = scope_bounds(&scope);
    let mut statement = connection.prepare(&sql)?;
    let mut rows = if scope == "." {
        statement.query(params![
            lower_depth,
            upper_depth,
            filter.min_size,
            filter.modified_before_epoch
        ])?
    } else {
        statement.query(params![
            lower_depth,
            upper_depth,
            filter.min_size,
            filter.modified_before_epoch,
            lower,
            upper
        ])?
    };
    let mut results = Vec::new();
    while let Some(row) = rows.next()? {
        let record = directory_from_row(row)?;
        if let Some(pattern) = filter.pattern {
            if !pattern.matches(&relative_to_scope(&record.path, &scope)) {
                continue;
            }
        }
        results.push(record);
        if results.len() >= filter.top {
            break;
        }
    }
    Ok((scope_record, results))
}

/// How a diff is ranked.
#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
pub enum DiffOrder {
    Growth,
    Shrink,
    Absolute,
    Current,
}

impl DiffOrder {
    pub fn name(self) -> &'static str {
        match self {
            DiffOrder::Growth => "growth",
            DiffOrder::Shrink => "shrink",
            DiffOrder::Absolute => "absolute",
            DiffOrder::Current => "current",
        }
    }

    fn key(self, record: &DiffRecord) -> i64 {
        match self {
            DiffOrder::Growth => record.change(),
            DiffOrder::Shrink => -record.change(),
            DiffOrder::Absolute => record.change().abs(),
            DiffOrder::Current => record.current,
        }
    }
}

/// Filters for a diff ranking.
pub struct DiffFilter<'a> {
    pub under: &'a str,
    pub max_depth: i64,
    pub top: usize,
    pub min_size: i64,
    pub pattern: Option<&'a PathPattern>,
    pub growth_only: bool,
    pub shrink_only: bool,
    pub order: DiffOrder,
    pub metric: Metric,
}

fn scope_depth(connection: &Connection, scope: &str) -> Result<Option<i64>> {
    Ok(connection
        .query_row(
            "SELECT depth FROM directories WHERE path = ?1",
            [scope],
            |row| row.get(0),
        )
        .optional()?)
}

fn directory_stream(
    connection: &Connection,
    scope: &str,
    scope_depth: i64,
    max_depth: i64,
) -> Result<Vec<(String, i64, i64, i64)>> {
    let mut sql = "SELECT path, allocated_bytes, apparent_bytes, inode_count FROM directories \
                   WHERE depth > ?1 AND depth <= ?2"
        .to_string();
    if scope != "." {
        sql.push_str(" AND path >= ?3 AND path < ?4");
    }
    sql.push_str(" ORDER BY path");
    let mut statement = connection.prepare(&sql)?;
    let map = |row: &Row| -> rusqlite::Result<(String, i64, i64, i64)> {
        Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
    };
    let rows = if scope == "." {
        statement
            .query_map(params![scope_depth, scope_depth + max_depth], map)?
            .collect::<rusqlite::Result<Vec<_>>>()?
    } else {
        let (lower, upper) = scope_bounds(scope);
        statement
            .query_map(
                params![scope_depth, scope_depth + max_depth, lower, upper],
                map,
            )?
            .collect::<rusqlite::Result<Vec<_>>>()?
    };
    Ok(rows)
}

/// Merge two path-ordered streams and keep the highest-ranked changes.
///
/// Paths whose value did not change are left out.
pub fn diff_ranking(
    previous: &Connection,
    current: &Connection,
    filter: &DiffFilter,
) -> Result<Vec<DiffRecord>> {
    let scope = normalize_relative_path(Some(filter.under))?;
    let previous_depth = scope_depth(previous, &scope)?;
    let current_depth = scope_depth(current, &scope)?;
    let depth = current_depth.or(previous_depth).ok_or_else(|| {
        usage(format!(
            "directory is absent from both snapshots: {}",
            filter.under
        ))
    })?;
    let value = |row: &(String, i64, i64, i64)| match filter.metric {
        Metric::Allocated => row.1,
        Metric::Apparent => row.2,
        Metric::Inodes => row.3,
    };
    let previous_rows = directory_stream(previous, &scope, depth, filter.max_depth)?;
    let current_rows = directory_stream(current, &scope, depth, filter.max_depth)?;
    let mut heap: BinaryHeap<Reverse<RankedChange>> = BinaryHeap::new();
    let (mut left, mut right) = (0, 0);
    while left < previous_rows.len() || right < current_rows.len() {
        let record = match (previous_rows.get(left), current_rows.get(right)) {
            (Some(old), Some(new)) if old.0 == new.0 => {
                left += 1;
                right += 1;
                DiffRecord {
                    path: old.0.clone(),
                    previous: value(old),
                    current: value(new),
                }
            }
            (Some(old), Some(new)) if old.0 < new.0 => {
                left += 1;
                DiffRecord {
                    path: old.0.clone(),
                    previous: value(old),
                    current: 0,
                }
            }
            (Some(old), None) => {
                left += 1;
                DiffRecord {
                    path: old.0.clone(),
                    previous: value(old),
                    current: 0,
                }
            }
            (_, Some(new)) => {
                right += 1;
                DiffRecord {
                    path: new.0.clone(),
                    previous: 0,
                    current: value(new),
                }
            }
            (None, None) => {
                return Err(Error::Format(
                    "snapshot directory streams are inconsistent".into(),
                ));
            }
        };
        let change = record.change();
        if change == 0
            || record.previous.max(record.current) < filter.min_size
            || (filter.growth_only && change <= 0)
            || (filter.shrink_only && change >= 0)
        {
            continue;
        }
        if let Some(pattern) = filter.pattern {
            if !pattern.matches(&relative_to_scope(&record.path, &scope)) {
                continue;
            }
        }
        // A larger key ranks higher; among equal keys, the path that sorts
        // first does.
        let item = Reverse((
            filter.order.key(&record),
            Reverse(record.path),
            record.previous,
            record.current,
        ));
        if heap.len() < filter.top {
            heap.push(item);
        } else if heap
            .peek()
            .is_some_and(|smallest| (item.0.0, &item.0.1) > (smallest.0.0, &smallest.0.1))
        {
            heap.pop();
            heap.push(item);
        }
    }
    let mut ranked: Vec<_> = heap.into_iter().map(|item| item.0).collect();
    ranked.sort_by(|left, right| (right.0, &right.1).cmp(&(left.0, &left.1)));
    Ok(ranked
        .into_iter()
        .map(|(_, Reverse(path), previous, current)| DiffRecord {
            path,
            previous,
            current,
        })
        .collect())
}

/// A change ranked by its key, then by path in reverse, with the previous
/// and current values.
type RankedChange = (i64, Reverse<String>, i64, i64);

/// Owner metrics that owner rankings support.
pub fn owner_column(metric: Metric) -> &'static str {
    match metric {
        Metric::Inodes => "inode_count",
        _ => "allocated_bytes",
    }
}

/// The largest owner of each of `paths` by `metric`, with that owner's
/// value, from the owner statistics recorded down to user_max_depth.
///
/// The table's key starts with the owner, so a lookup by path would read
/// the whole table each time; one pass over the depths in question serves
/// every path at once.
pub fn top_owners(
    connection: &Connection,
    paths: &HashSet<&str>,
    depths: (i64, i64),
    metric: Metric,
) -> Result<HashMap<String, (u32, i64)>> {
    let column = metric.column();
    let mut statement = connection.prepare(&format!(
        "SELECT path, uid, {column} FROM owner_directories WHERE depth >= ?1 AND depth <= ?2"
    ))?;
    let mut rows = statement.query(params![depths.0, depths.1])?;
    let mut best: HashMap<String, (u32, i64)> = HashMap::new();
    while let Some(row) = rows.next()? {
        let path = row.get_ref(0)?.as_str().map_err(rusqlite::Error::from)?;
        if !paths.contains(path) {
            continue;
        }
        let uid = row.get::<_, i64>(1)? as u32;
        let value: i64 = row.get(2)?;
        // As ORDER BY value DESC, uid: the largest value, then the lowest uid.
        match best.get_mut(path) {
            Some(known) if (value, Reverse(uid)) > (known.1, Reverse(known.0)) => {
                *known = (uid, value);
            }
            Some(_) => {}
            None => {
                best.insert(path.to_string(), (uid, value));
            }
        }
    }
    Ok(best)
}

/// Ranked owners, each owner's largest directories, and the metric total.
pub struct OwnerRanking {
    pub owners: Vec<OwnerRecord>,
    pub directories: HashMap<u32, Vec<OwnerDirectoryRecord>>,
    pub total: i64,
}

/// Rank owners by `metric` with up to `directories_per_user` directories each.
pub fn owners(
    connection: &Connection,
    scope: &str,
    top: usize,
    directories_per_user: usize,
    metric: Metric,
) -> Result<OwnerRanking> {
    let column = owner_column(metric);
    // The root's owners have their own table; a directory's come from the
    // owner statistics recorded down to user_max_depth.
    let source = if scope == "." {
        "owners"
    } else {
        "owner_directories"
    };
    // Every statement binds the scope, which the owners table ignores.
    let condition = |index: usize| {
        if scope == "." {
            format!("?{index} = '.'")
        } else {
            format!("path = ?{index}")
        }
    };
    let mut statement = connection.prepare(&format!(
        "SELECT uid, allocated_bytes, apparent_bytes, file_count, directory_count, inode_count \
         FROM {source} WHERE {} ORDER BY {column} DESC, uid LIMIT ?1",
        condition(2)
    ))?;
    let owners = statement
        .query_map(params![top as i64, scope], |row| {
            let uid: i64 = row.get(0)?;
            Ok(OwnerRecord {
                uid: uid as u32,
                username: sys::username_for_uid(uid as u32),
                allocated_bytes: row.get(1)?,
                apparent_bytes: row.get(2)?,
                file_count: row.get(3)?,
                directory_count: row.get(4)?,
                inode_count: row.get(5)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let total: i64 = connection.query_row(
        &format!(
            "SELECT COALESCE(SUM({column}), 0) FROM {source} WHERE {}",
            condition(1)
        ),
        params![scope],
        |row| row.get(0),
    )?;
    let mut grouped = HashMap::new();
    if directories_per_user > 0 {
        let (lower, upper) = scope_bounds(scope);
        let within = if scope == "." {
            "path <> '.'"
        } else {
            "path >= ?3 AND path < ?4"
        };
        let mut statement = connection.prepare(&format!(
            "SELECT path, depth, allocated_bytes, apparent_bytes, file_count, directory_count, \
             inode_count FROM owner_directories WHERE uid = ?1 AND {within} \
             ORDER BY {column} DESC, path LIMIT ?2"
        ))?;
        for owner in &owners {
            let limit = directories_per_user as i64;
            let found = if scope == "." {
                statement.query_map(params![owner.uid, limit], owner_directory_from_row)
            } else {
                statement.query_map(
                    params![owner.uid, limit, lower, upper],
                    owner_directory_from_row,
                )
            };
            let rows = found?.collect::<rusqlite::Result<Vec<_>>>()?;
            grouped.insert(owner.uid, rows);
        }
    }
    Ok(OwnerRanking {
        owners,
        directories: grouped,
        total,
    })
}

fn owner_directory_from_row(row: &rusqlite::Row) -> rusqlite::Result<OwnerDirectoryRecord> {
    Ok(OwnerDirectoryRecord {
        path: row.get(0)?,
        depth: row.get(1)?,
        allocated_bytes: row.get(2)?,
        apparent_bytes: row.get(3)?,
        file_count: row.get(4)?,
        directory_count: row.get(5)?,
        inode_count: row.get(6)?,
    })
}

/// Filters for stored large-file candidates.
pub struct FileFilter<'a> {
    pub top: usize,
    pub under: &'a str,
    pub min_size: i64,
    pub uid: Option<u32>,
    pub modified_before_epoch: Option<f64>,
}

/// Stored large-file candidates that satisfy the filters.
pub fn large_files(connection: &Connection, filter: &FileFilter) -> Result<Vec<FileRecord>> {
    let scope = normalize_relative_path(Some(filter.under))?;
    let (lower, upper) = scope_bounds(&scope);
    let mut statement = connection.prepare(
        "SELECT path, uid, allocated_bytes, apparent_bytes, modified_epoch FROM large_files \
         WHERE allocated_bytes >= ?1 AND (?2 = '.' OR (path >= ?3 AND path < ?4)) \
         AND (?5 IS NULL OR uid = ?5) AND (?6 IS NULL OR modified_epoch <= ?6) \
         ORDER BY allocated_bytes DESC, path LIMIT ?7",
    )?;
    let files = statement
        .query_map(
            params![
                filter.min_size,
                scope,
                lower,
                upper,
                filter.uid,
                filter.modified_before_epoch,
                filter.top as i64
            ],
            |row| {
                let uid: i64 = row.get(1)?;
                Ok(FileRecord {
                    path: row.get(0)?,
                    uid: uid as u32,
                    username: sys::username_for_uid(uid as u32),
                    allocated_bytes: row.get(2)?,
                    apparent_bytes: row.get(3)?,
                    modified_epoch: row.get(4)?,
                })
            },
        )?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(files)
}

/// The bytes one owner holds in a snapshot, or 0 for an owner it does not
/// know.
pub fn owner_allocation(connection: &Connection, uid: u32) -> Result<i64> {
    Ok(connection
        .query_row(
            "SELECT allocated_bytes FROM owners WHERE uid = ?1",
            [uid],
            |row| row.get(0),
        )
        .optional()?
        .unwrap_or(0))
}

/// Visit every stored directory, largest first, with its allocated bytes,
/// until `visit` returns false.
pub fn visit_directories_by_size(
    connection: &Connection,
    mut visit: impl FnMut(&str, i64) -> bool,
) -> Result<()> {
    let mut statement = connection.prepare(
        "SELECT path, allocated_bytes FROM directories ORDER BY allocated_bytes DESC, path",
    )?;
    let mut rows = statement.query([])?;
    while let Some(row) = rows.next()? {
        let path: String = row.get(0)?;
        if !visit(&path, row.get(1)?) {
            break;
        }
    }
    Ok(())
}

/// Normalize a root-relative path and reject parent traversal.
pub fn normalize_relative_path(value: Option<&str>) -> Result<String> {
    let mut parts = Vec::new();
    for part in value.unwrap_or(".").split('/') {
        match part {
            "" | "." => {}
            // Rejected anywhere: after a symbolic link, "link/.." leaves the
            // root. A backslash stays part of the name, as in "\xff".
            ".." => return Err(usage("relative paths must stay inside the snapshot root")),
            part => parts.push(part),
        }
    }
    if parts.is_empty() {
        Ok(".".into())
    } else {
        Ok(parts.join("/"))
    }
}

/// The root-relative form of a directory given on the command line: a path
/// relative to the snapshot root, or an absolute path inside it.
pub fn scope_in_root(root: &str, path: Option<&str>) -> Result<String> {
    let Some(path) = path.filter(|path| path.starts_with('/')) else {
        return normalize_relative_path(path);
    };
    // The root was recorded without symbolic links; resolve the argument the
    // same way while it exists.
    let resolved = std::fs::canonicalize(path)
        .ok()
        .and_then(|resolved| resolved.to_str().map(str::to_string))
        .unwrap_or_else(|| path.to_string());
    let root = root.trim_end_matches('/');
    let inside = if resolved == root || (root.is_empty() && resolved == "/") {
        Some("")
    } else {
        resolved
            .strip_prefix(root)
            .and_then(|rest| rest.strip_prefix('/'))
    };
    match inside {
        Some(relative) => normalize_relative_path(Some(relative)),
        None => Err(usage(format!(
            "{path} is outside the snapshot root {}",
            if root.is_empty() { "/" } else { root }
        ))),
    }
}

/// Render a stored root-relative path relative to a display scope.
pub fn relative_to_scope(path: &str, scope: &str) -> String {
    if scope == "." {
        return path.to_string();
    }
    if path == scope {
        return ".".into();
    }
    match path
        .strip_prefix(scope)
        .and_then(|rest| rest.strip_prefix('/'))
    {
        Some(rest) => rest.to_string(),
        None => path.to_string(),
    }
}

/// Resolve a decimal UID or a local account name.
pub fn resolve_user(value: &str) -> Result<u32> {
    if !value.is_empty() && value.chars().all(|character| character.is_ascii_digit()) {
        return value
            .parse()
            .map_err(|_| usage(format!("unknown user: {value}")));
    }
    sys::uid_for_username(value).ok_or_else(|| usage(format!("unknown user: {value}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relative_paths_stay_inside_the_root() {
        assert_eq!(normalize_relative_path(Some("./a/b/")).unwrap(), "a/b");
        assert_eq!(normalize_relative_path(None).unwrap(), ".");
        assert_eq!(normalize_relative_path(Some("/a//./b")).unwrap(), "a/b");
        assert_eq!(normalize_relative_path(Some("a\\xff")).unwrap(), "a\\xff");
        for escaping in ["..", "../x", "a/../../b", "link/..", "a/./.."] {
            assert!(
                normalize_relative_path(Some(escaping)).is_err(),
                "{escaping}"
            );
        }
        assert_eq!(relative_to_scope("a/b/c", "a/b"), "c");
        assert_eq!(relative_to_scope("a/b", "a/b"), ".");
        assert_eq!(relative_to_scope("a/bc", "a/b"), "a/bc");
    }
}
