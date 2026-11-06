//! Snapshot records shared by storage, queries, and commands.

use serde_json::{Map, Value, json};

/// Trusted index metadata for one stored snapshot file.
#[derive(Clone, Debug, PartialEq)]
pub struct SnapshotRecord {
    pub snapshot_id: String,
    pub filename: String,
    pub created_epoch: i64,
    pub created_at: String,
    pub root: String,
    pub host: String,
    /// No failure other than a denied permission.
    pub complete: bool,
    pub error_count: u64,
    /// Paths that could not be read for lack of permission.
    pub denied_count: u64,
    pub allocated_bytes: u64,
    pub apparent_bytes: u64,
    pub directory_count: u64,
    pub file_count: u64,
    pub inode_count: u64,
    pub collect_users: bool,
    pub large_file_limit: u64,
    pub exclude_hash: String,
    pub sha256: String,
    pub archived: bool,
    /// Names given with `--label` or `qdu label`; each names one snapshot
    /// of the profile, which retention then keeps.
    pub labels: Vec<String>,
}

/// Whether a file name can only refer to a snapshot inside its directory.
pub fn is_safe_snapshot_filename(filename: &str) -> bool {
    !filename.is_empty()
        && !filename.contains('/')
        && !filename.contains('\\')
        && (filename.ends_with(".sqlite3") || filename.ends_with(".sqlite3.gz"))
}

impl SnapshotRecord {
    /// The JSON object shown by `qdu list` and the reports, in display order.
    pub fn to_json(&self) -> Value {
        json!({
            "snapshot_id": self.snapshot_id,
            "filename": self.filename,
            "created_epoch": self.created_epoch,
            "created_at": self.created_at,
            "root": self.root,
            "host": self.host,
            "complete": self.complete,
            "error_count": self.error_count,
            "denied_count": self.denied_count,
            "allocated_bytes": self.allocated_bytes,
            "apparent_bytes": self.apparent_bytes,
            "directory_count": self.directory_count,
            "file_count": self.file_count,
            "inode_count": self.inode_count,
            "collect_users": self.collect_users,
            "large_file_limit": self.large_file_limit,
            "exclude_hash": self.exclude_hash,
            "sha256": self.sha256,
            "archived": self.archived,
            "labels": self.labels,
        })
    }

    /// Decode and validate a record read from the profile index.
    pub fn from_json(value: &Value) -> Result<Self, String> {
        let object = value
            .as_object()
            .ok_or("snapshot record must be an object")?;
        let text = |key: &str| -> Result<String, String> {
            match object.get(key) {
                Some(Value::String(text)) => Ok(text.clone()),
                Some(Value::Number(number)) => Ok(number.to_string()),
                Some(_) => Err(format!("{key} must be a string")),
                None => Err(format!("missing {key}")),
            }
        };
        let integer = |key: &str| -> Result<i64, String> {
            match object.get(key) {
                Some(Value::Number(number)) => number
                    .as_i64()
                    .or_else(|| number.as_f64().map(|value| value as i64))
                    .ok_or_else(|| format!("{key} must be an integer")),
                Some(Value::String(text)) => text
                    .trim()
                    .parse()
                    .map_err(|_| format!("{key} must be an integer")),
                Some(_) => Err(format!("{key} must be an integer")),
                None => Err(format!("missing {key}")),
            }
        };
        let count = |key: &str| -> Result<u64, String> {
            u64::try_from(integer(key)?).map_err(|_| format!("{key} must be non-negative"))
        };
        let flag = |key: &str, default: Option<bool>| -> Result<bool, String> {
            match object.get(key) {
                Some(Value::Bool(value)) => Ok(*value),
                Some(Value::Number(number)) => Ok(number.as_f64() != Some(0.0)),
                Some(Value::Null) => Ok(false),
                Some(_) => Err(format!("{key} must be a boolean")),
                None => default.ok_or_else(|| format!("missing {key}")),
            }
        };
        let record = SnapshotRecord {
            snapshot_id: text("snapshot_id")?,
            filename: text("filename")?,
            created_epoch: integer("created_epoch")?,
            created_at: text("created_at")?,
            root: text("root")?,
            host: text("host")?,
            complete: flag("complete", None)?,
            error_count: count("error_count")?,
            // Indexes from before failures were classified have none.
            denied_count: match object.get("denied_count") {
                Some(_) => count("denied_count")?,
                None => 0,
            },
            allocated_bytes: count("allocated_bytes")?,
            apparent_bytes: count("apparent_bytes")?,
            directory_count: count("directory_count")?,
            file_count: count("file_count")?,
            inode_count: count("inode_count")?,
            collect_users: flag("collect_users", None)?,
            large_file_limit: count("large_file_limit")?,
            exclude_hash: text("exclude_hash")?,
            sha256: text("sha256")?,
            archived: flag("archived", Some(false))?,
            labels: match object.get("labels") {
                None | Some(Value::Null) => Vec::new(),
                Some(Value::Array(items)) => items
                    .iter()
                    .map(|item| {
                        item.as_str()
                            .map(str::to_string)
                            .ok_or_else(|| "labels must be strings".to_string())
                    })
                    .collect::<Result<_, _>>()?,
                Some(_) => return Err("labels must be a list".into()),
            },
        };
        if !is_safe_snapshot_filename(&record.filename) {
            return Err("snapshot filename must be a safe SQLite snapshot basename".into());
        }
        Ok(record)
    }
}

/// A failure recorded by a scan.
#[derive(Clone, Debug)]
pub struct ScanErrorRecord {
    pub path: String,
    pub operation: String,
    /// `denied` for a lack of permission, otherwise `error`.
    pub kind: String,
    pub message: String,
}

/// Ordered snapshot history and latest selectors for one profile.
#[derive(Clone, Debug)]
pub struct ProfileIndex {
    pub version: u64,
    pub profile: String,
    pub latest_complete: Option<String>,
    pub latest_any: Option<String>,
    pub snapshots: Vec<SnapshotRecord>,
}

impl ProfileIndex {
    pub fn empty(version: u64, profile: &str) -> Self {
        ProfileIndex {
            version,
            profile: profile.to_string(),
            latest_complete: None,
            latest_any: None,
            snapshots: Vec::new(),
        }
    }

    /// The directory of the newest snapshot, which later snapshots must share.
    pub fn recorded_root(&self) -> Option<&str> {
        let id = self.latest_any.as_deref()?;
        self.snapshots
            .iter()
            .find(|item| item.snapshot_id == id)
            .map(|item| item.root.as_str())
    }

    /// The JSON stored on disk, with keys sorted as the Python version wrote them.
    pub fn to_json(&self) -> Value {
        fn sorted(value: Value) -> Value {
            match value {
                Value::Object(object) => {
                    let mut entries: Vec<(String, Value)> = object.into_iter().collect();
                    entries.sort_by(|left, right| left.0.cmp(&right.0));
                    Value::Object(
                        entries
                            .into_iter()
                            .map(|(key, value)| (key, sorted(value)))
                            .collect::<Map<String, Value>>(),
                    )
                }
                Value::Array(items) => Value::Array(items.into_iter().map(sorted).collect()),
                other => other,
            }
        }
        sorted(json!({
            "version": self.version,
            "profile": self.profile,
            "latest_complete": self.latest_complete,
            "latest_any": self.latest_any,
            "snapshots": self.snapshots.iter().map(SnapshotRecord::to_json).collect::<Vec<_>>(),
        }))
    }

    /// Decode an index, skipping entries that are not objects as before.
    pub fn from_json(value: &Value) -> Result<Self, String> {
        let object = value.as_object().ok_or("index root must be an object")?;
        let version = object
            .get("version")
            .and_then(Value::as_u64)
            .ok_or("missing version")?;
        let profile = object
            .get("profile")
            .and_then(Value::as_str)
            .ok_or("missing profile")?
            .to_string();
        let selector = |key: &str| -> Option<String> {
            object.get(key).and_then(|value| match value {
                Value::String(text) => Some(text.clone()),
                Value::Null => None,
                other => Some(other.to_string()),
            })
        };
        let snapshots = match object.get("snapshots") {
            None => Vec::new(),
            Some(Value::Array(items)) => items
                .iter()
                .filter(|item| item.is_object())
                .map(SnapshotRecord::from_json)
                .collect::<Result<Vec<_>, _>>()?,
            Some(_) => return Err("snapshots must be a list".into()),
        };
        Ok(ProfileIndex {
            version,
            profile,
            latest_complete: selector("latest_complete"),
            latest_any: selector("latest_any"),
            snapshots,
        })
    }

    /// A chronologically ordered index with the latest selectors rebuilt.
    pub fn with_records(&self, mut records: Vec<SnapshotRecord>) -> Self {
        records.sort_by_key(|record| record.created_epoch);
        let latest_any = records.last().map(|record| record.snapshot_id.clone());
        let latest_complete = records
            .iter()
            .rev()
            .find(|record| record.complete)
            .map(|record| record.snapshot_id.clone());
        ProfileIndex {
            version: self.version,
            profile: self.profile.clone(),
            latest_complete,
            latest_any,
            snapshots: records,
        }
    }
}

/// Aggregated usage and freshness for one snapshot directory.
#[derive(Clone, Debug, Default)]
pub struct DirectoryRecord {
    pub path: String,
    pub depth: i64,
    pub allocated_bytes: i64,
    pub apparent_bytes: i64,
    pub file_count: i64,
    pub directory_count: i64,
    pub inode_count: i64,
    pub latest_modified_epoch: f64,
}

/// A directory metric that queries can rank or compare by.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Metric {
    Allocated,
    Apparent,
    Inodes,
}

impl Metric {
    /// The column name, which is also the name used in JSON and TSV output.
    pub fn column(self) -> &'static str {
        match self {
            Metric::Allocated => "allocated_bytes",
            Metric::Apparent => "apparent_bytes",
            Metric::Inodes => "inode_count",
        }
    }

    /// The short label shown in report headers.
    pub fn label(self) -> &'static str {
        match self {
            Metric::Allocated => "allocated",
            Metric::Apparent => "apparent",
            Metric::Inodes => "inodes",
        }
    }

    pub fn of(self, record: &DirectoryRecord) -> i64 {
        match self {
            Metric::Allocated => record.allocated_bytes,
            Metric::Apparent => record.apparent_bytes,
            Metric::Inodes => record.inode_count,
        }
    }
}

/// Snapshot-wide usage attributed to one file owner.
#[derive(Clone, Debug)]
pub struct OwnerRecord {
    pub uid: u32,
    pub username: String,
    pub allocated_bytes: i64,
    pub apparent_bytes: i64,
    pub file_count: i64,
    pub directory_count: i64,
    pub inode_count: i64,
}

/// Usage attributed to one owner below a recorded directory.
#[derive(Clone, Debug)]
pub struct OwnerDirectoryRecord {
    pub path: String,
    pub depth: i64,
    pub allocated_bytes: i64,
    pub apparent_bytes: i64,
    pub file_count: i64,
    pub directory_count: i64,
    pub inode_count: i64,
}

/// Size, owner, and modification time of one large-file candidate.
#[derive(Clone, Debug)]
pub struct FileRecord {
    pub path: String,
    pub uid: u32,
    pub username: String,
    pub allocated_bytes: i64,
    pub apparent_bytes: i64,
    pub modified_epoch: f64,
}

/// One directory metric at two snapshot points; absent paths count as 0.
#[derive(Clone, Debug)]
pub struct DiffRecord {
    pub path: String,
    pub previous: i64,
    pub current: i64,
}

impl DiffRecord {
    pub fn change(&self) -> i64 {
        self.current - self.previous
    }

    /// Change relative to the previous value, or `None` for a new path.
    pub fn change_percent(&self) -> Option<f64> {
        (self.previous > 0).then(|| self.change() as f64 * 100.0 / self.previous as f64)
    }
}
