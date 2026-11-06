//! Profile state paths, the snapshot index, and SQLite snapshot files.

use std::collections::HashMap;
use std::fs;
use std::io::{self, Read, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

use flate2::Compression;
use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use rusqlite::{Connection, OpenFlags};
use sha2::{Digest, Sha256};

use crate::config::{validate_profile_name, xdg_state_home};
use crate::error::{Error, PathContext, Result, usage};
use crate::model::{ProfileIndex, SnapshotRecord};
use crate::selector::Selector;
use crate::signals::Cleanup;
use crate::units::hex;

pub const INDEX_VERSION: u64 = 1;
pub const SNAPSHOT_SCHEMA_VERSION: i64 = 4;
/// Schema 2 added a parent column, three unused secondary indexes on
/// `directories`, and the `seen_inodes` table; schema 4 added the `kind`
/// column of `scan_errors`. The tables are otherwise identical, so every
/// query reads all three versions, and a failure from before schema 4 counts
/// as an error.
pub const READABLE_SCHEMA_VERSIONS: [i64; 3] = [2, 3, 4];

pub const SCHEMA_SQL: &str = "
CREATE TABLE metadata (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL
) WITHOUT ROWID;
CREATE TABLE directories (
    path TEXT PRIMARY KEY,
    depth INTEGER NOT NULL CHECK(depth >= 0),
    allocated_bytes INTEGER NOT NULL CHECK(allocated_bytes >= 0),
    apparent_bytes INTEGER NOT NULL CHECK(apparent_bytes >= 0),
    file_count INTEGER NOT NULL CHECK(file_count >= 0),
    directory_count INTEGER NOT NULL CHECK(directory_count >= 1),
    inode_count INTEGER NOT NULL CHECK(inode_count >= 1),
    latest_modified_epoch REAL NOT NULL CHECK(latest_modified_epoch >= 0)
) WITHOUT ROWID;
CREATE TABLE owners (
    uid INTEGER PRIMARY KEY,
    allocated_bytes INTEGER NOT NULL CHECK(allocated_bytes >= 0),
    apparent_bytes INTEGER NOT NULL CHECK(apparent_bytes >= 0),
    file_count INTEGER NOT NULL CHECK(file_count >= 0),
    directory_count INTEGER NOT NULL CHECK(directory_count >= 0),
    inode_count INTEGER NOT NULL CHECK(inode_count >= 0)
) WITHOUT ROWID;
CREATE TABLE owner_directories (
    uid INTEGER NOT NULL,
    path TEXT NOT NULL,
    depth INTEGER NOT NULL CHECK(depth >= 0),
    allocated_bytes INTEGER NOT NULL CHECK(allocated_bytes >= 0),
    apparent_bytes INTEGER NOT NULL CHECK(apparent_bytes >= 0),
    file_count INTEGER NOT NULL CHECK(file_count >= 0),
    directory_count INTEGER NOT NULL CHECK(directory_count >= 0),
    inode_count INTEGER NOT NULL CHECK(inode_count >= 0),
    PRIMARY KEY(uid, path)
) WITHOUT ROWID;
CREATE TABLE large_files (
    path TEXT PRIMARY KEY,
    uid INTEGER NOT NULL,
    allocated_bytes INTEGER NOT NULL CHECK(allocated_bytes >= 0),
    apparent_bytes INTEGER NOT NULL CHECK(apparent_bytes >= 0),
    modified_epoch REAL NOT NULL
) WITHOUT ROWID;
CREATE INDEX large_files_allocated_idx ON large_files(allocated_bytes DESC, path);
CREATE TABLE scan_errors (
    id INTEGER PRIMARY KEY,
    path TEXT NOT NULL,
    operation TEXT NOT NULL,
    kind TEXT NOT NULL CHECK(kind IN ('denied', 'error')),
    message TEXT NOT NULL
);
";

/// Create a private directory and its parents.
pub fn create_private_directory(path: &Path) -> Result<()> {
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(path)
        .at(path)
}

/// The profiles that have a state directory, by name.
pub fn profiles_with_state() -> Vec<String> {
    let Ok(entries) = fs::read_dir(xdg_state_home().join("qdu").join("profiles")) else {
        return Vec::new();
    };
    let mut names: Vec<String> = entries
        .flatten()
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
        .filter_map(|entry| entry.file_name().into_string().ok())
        .filter(|name| validate_profile_name(name).is_ok())
        .collect();
    names.sort();
    names
}

/// Owner-scoped state paths derived from one validated profile name.
#[derive(Clone, Debug)]
pub struct ProfilePaths {
    pub profile: String,
    pub root: PathBuf,
    pub snapshots: PathBuf,
    pub index: PathBuf,
    pub lock: PathBuf,
    pub temporary: PathBuf,
}

impl ProfilePaths {
    pub fn for_profile(profile: &str) -> Result<Self> {
        validate_profile_name(profile)?;
        let root = xdg_state_home().join("qdu").join("profiles").join(profile);
        Ok(ProfilePaths {
            profile: profile.to_string(),
            snapshots: root.join("snapshots"),
            index: root.join("index.json"),
            lock: root.join(".lock"),
            temporary: root.join("tmp"),
            root,
        })
    }

    /// Create the private profile, snapshot, and temporary directories.
    pub fn ensure(&self) -> Result<()> {
        create_private_directory(&self.root)?;
        create_private_directory(&self.snapshots)?;
        create_private_directory(&self.temporary)?;
        Ok(())
    }

    /// The disk space a snapshot file uses, or `None` when it is missing.
    pub fn stored_bytes(&self, record: &SnapshotRecord) -> Option<u64> {
        fs::metadata(self.snapshots.join(&record.filename))
            .ok()
            .map(|metadata| metadata.len())
    }
}

/// Load and atomically update one versioned profile index.
pub struct IndexRepository {
    pub paths: ProfilePaths,
}

impl IndexRepository {
    pub fn new(paths: ProfilePaths) -> Self {
        IndexRepository { paths }
    }

    /// Load and validate the index, or return an empty one.
    pub fn load(&self) -> Result<ProfileIndex> {
        let text = match fs::read_to_string(&self.paths.index) {
            Ok(text) => text,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Ok(ProfileIndex::empty(INDEX_VERSION, &self.paths.profile));
            }
            Err(error) => {
                return Err(Error::Format(format!(
                    "invalid profile index: {}: {error}",
                    self.paths.index.display()
                )));
            }
        };
        let index = serde_json::from_str(&text)
            .map_err(|error| error.to_string())
            .and_then(|value| ProfileIndex::from_json(&value))
            .map_err(|message| {
                Error::Format(format!(
                    "invalid profile index: {}: {message}",
                    self.paths.index.display()
                ))
            })?;
        if index.version != INDEX_VERSION {
            return Err(Error::Format(format!(
                "unsupported profile index version {}; expected {INDEX_VERSION}",
                index.version
            )));
        }
        if index.profile != self.paths.profile {
            return Err(Error::Format(
                "profile index belongs to a different profile".into(),
            ));
        }
        Ok(index)
    }

    /// Atomically save an index with owner-only permissions.
    pub fn save(&self, index: &ProfileIndex) -> Result<()> {
        self.paths.ensure()?;
        let temporary = self.paths.index.with_extension("tmp");
        let mut text = serde_json::to_string_pretty(&index.to_json())
            .map_err(|error| Error::Runtime(error.to_string()))?;
        text.push('\n');
        write_private_file(&temporary, text.as_bytes())?;
        fs::rename(&temporary, &self.paths.index).at(&self.paths.index)?;
        fsync_directory(&self.paths.root);
        Ok(())
    }

    /// Insert or replace a record and rebuild the latest selectors. A label
    /// of the record moves to it from any other snapshot.
    pub fn add(&self, record: SnapshotRecord) -> Result<ProfileIndex> {
        let current = self.load()?;
        let mut records: Vec<SnapshotRecord> = current
            .snapshots
            .iter()
            .filter(|item| item.snapshot_id != record.snapshot_id)
            .cloned()
            .collect();
        for other in &mut records {
            other.labels.retain(|label| !record.labels.contains(label));
        }
        records.push(record);
        let updated = current.with_records(records);
        self.save(&updated)?;
        Ok(updated)
    }

    /// Remove the selected records and rebuild the latest selectors.
    pub fn remove(&self, snapshot_ids: &[String]) -> Result<ProfileIndex> {
        let current = self.load()?;
        let records = current
            .snapshots
            .iter()
            .filter(|item| !snapshot_ids.contains(&item.snapshot_id))
            .cloned()
            .collect();
        let updated = current.with_records(records);
        self.save(&updated)?;
        Ok(updated)
    }

    /// The snapshot that a selector names; see [`Selector`].
    pub fn resolve(&self, selector: &Selector, include_incomplete: bool) -> Result<SnapshotRecord> {
        selector
            .select(&self.load()?, include_incomplete)
            .map_err(usage)
    }

    /// The newest complete snapshot before the given record.
    pub fn previous_of(&self, record: &SnapshotRecord) -> Result<SnapshotRecord> {
        let index = self.load()?;
        let before = index
            .snapshots
            .iter()
            .take_while(|item| item.snapshot_id != record.snapshot_id)
            .filter(|item| item.complete)
            .last();
        match before {
            Some(previous) => Ok(previous.clone()),
            None if index.snapshots.iter().filter(|item| item.complete).count() < 2 => Err(usage(
                "a comparison needs two complete snapshots, and this profile has fewer; \
                     run 'qdu snapshot' again later",
            )),
            None => Err(usage(format!(
                "no complete snapshot exists before {}",
                record.snapshot_id
            ))),
        }
    }
}

/// Write a new file readable and writable by its owner only.
pub fn write_private_file(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut handle = fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)
        .at(path)?;
    handle.write_all(bytes).at(path)?;
    handle.sync_all().at(path)?;
    Ok(())
}

/// Flush a directory entry change to disk; failures are ignored.
pub fn fsync_directory(path: &Path) {
    if let Ok(directory) = fs::File::open(path) {
        let _ = directory.sync_all();
    }
}

/// Create an empty snapshot database tuned for one bulk load.
pub fn create_snapshot_database(path: &Path) -> Result<Connection> {
    if let Some(parent) = path.parent() {
        create_private_directory(parent)?;
    }
    let connection = Connection::open(path)?;
    connection.execute_batch(
        "PRAGMA journal_mode = OFF; PRAGMA synchronous = OFF; PRAGMA temp_store = FILE;",
    )?;
    connection.execute_batch(SCHEMA_SQL)?;
    connection.execute(
        "INSERT INTO metadata(key, value) VALUES ('schema_version', ?1)",
        [SNAPSHOT_SCHEMA_VERSION.to_string()],
    )?;
    Ok(connection)
}

/// Store metadata entries in a snapshot database, replacing earlier values.
pub fn write_metadata(connection: &Connection, entries: &[(&str, String)]) -> Result<()> {
    let mut statement =
        connection.prepare("INSERT OR REPLACE INTO metadata(key, value) VALUES (?1, ?2)")?;
    for (key, value) in entries {
        statement.execute(rusqlite::params![key, value])?;
    }
    Ok(())
}

/// Open an existing snapshot for queries only.
pub fn open_snapshot_database(path: &Path) -> Result<Connection> {
    let connection = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    connection.execute_batch("PRAGMA query_only = ON;")?;
    Ok(connection)
}

/// Load all snapshot metadata values.
pub fn snapshot_metadata(connection: &Connection) -> Result<HashMap<String, String>> {
    let mut statement = connection.prepare("SELECT key, value FROM metadata")?;
    let rows = statement.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })?;
    let mut metadata = HashMap::new();
    for row in rows {
        let (key, value) = row?;
        metadata.insert(key, value);
    }
    Ok(metadata)
}

/// A readable database path; archived snapshots are expanded temporarily.
pub struct MaterializedSnapshot {
    pub path: PathBuf,
    _temporary: Option<TemporaryFile>,
}

impl MaterializedSnapshot {
    pub fn open(paths: &ProfilePaths, record: &SnapshotRecord) -> Result<Self> {
        let source = paths.snapshots.join(&record.filename);
        if !source.exists() {
            return Err(Error::Format(format!(
                "snapshot file is missing: {}",
                source.display()
            )));
        }
        if !record.archived {
            return Ok(MaterializedSnapshot {
                path: source,
                _temporary: None,
            });
        }
        let temporary = decompress_snapshot(&source)?;
        Ok(MaterializedSnapshot {
            path: temporary.path.clone(),
            _temporary: Some(temporary),
        })
    }
}

/// A private temporary file removed when dropped or on interruption.
pub struct TemporaryFile {
    pub path: PathBuf,
    _cleanup: Cleanup,
}

impl TemporaryFile {
    /// Create an empty owner-only file in the system temporary directory.
    pub fn create(prefix: &str, suffix: &str) -> Result<(Self, fs::File)> {
        let directory = std::env::temp_dir();
        for attempt in 0..100u32 {
            let name = format!("{prefix}{}{suffix}", unique_token(attempt));
            let path = directory.join(name);
            let created = Cleanup::create(&path, |path| {
                fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .mode(0o600)
                    .open(path)
            });
            match created {
                Ok((cleanup, file)) => {
                    return Ok((
                        TemporaryFile {
                            path,
                            _cleanup: cleanup,
                        },
                        file,
                    ));
                }
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(error.into()),
            }
        }
        Err(Error::Runtime(format!(
            "cannot create a temporary file in {}",
            directory.display()
        )))
    }
}

/// A short token that differs between calls and processes.
///
/// `attempt` tells apart tokens drawn in quick succession, as when a name
/// is retried after a collision. The token names files; it protects nothing.
pub fn unique_token(attempt: u32) -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_nanos())
        .unwrap_or(0);
    let mut digest = Sha256::new();
    digest.update(nanos.to_le_bytes());
    digest.update(std::process::id().to_le_bytes());
    digest.update(attempt.to_le_bytes());
    let mut random = [0u8; 16];
    if let Ok(mut source) = fs::File::open("/dev/urandom") {
        let _ = source.read_exact(&mut random);
    }
    digest.update(random);
    hex(&digest.finalize()[..6])
}

/// Expand a gzipped snapshot into a private file in `$TMPDIR`.
///
/// The copy goes to the system temporary directory, usually local, rather
/// than next to the state, which may sit on a slow or nearly full network
/// home directory.
pub fn decompress_snapshot(source: &Path) -> Result<TemporaryFile> {
    let (temporary, mut target) = TemporaryFile::create("qdu-", ".sqlite3")?;
    let input = fs::File::open(source).at(source)?;
    let mut decoder = GzDecoder::new(io::BufReader::with_capacity(1 << 20, input));
    if let Err(error) = io::copy(&mut decoder, &mut target) {
        let name = source
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        return Err(Error::Format(format!(
            "cannot decompress snapshot {name}: {error}"
        )));
    }
    target.flush()?;
    Ok(temporary)
}

/// The lowercase SHA-256 digest of a file's bytes.
pub fn sha256_file(path: &Path) -> Result<String> {
    let mut file = fs::File::open(path).at(path)?;
    let mut digest = Sha256::new();
    let mut buffer = vec![0u8; 1 << 20];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
    }
    Ok(hex(&digest.finalize()))
}

/// Verify SQLite integrity, schema version, and stored row invariants.
pub fn validate_database(path: &Path) -> Result<HashMap<String, String>> {
    let check = || -> Result<HashMap<String, String>> {
        let connection = open_snapshot_database(path)?;
        let integrity: String =
            connection.query_row("PRAGMA integrity_check", [], |row| row.get(0))?;
        if !integrity.eq_ignore_ascii_case("ok") {
            return Err(Error::Verification(format!(
                "SQLite integrity check failed: {integrity}"
            )));
        }
        let metadata = snapshot_metadata(&connection)?;
        let version: i64 = metadata
            .get("schema_version")
            .and_then(|value| value.parse().ok())
            .unwrap_or(0);
        if !READABLE_SCHEMA_VERSIONS.contains(&version) {
            let readable: Vec<String> = READABLE_SCHEMA_VERSIONS
                .iter()
                .map(i64::to_string)
                .collect();
            return Err(Error::Verification(format!(
                "unsupported snapshot schema {version}; expected one of {}",
                readable.join(", ")
            )));
        }
        let count: i64 =
            connection.query_row("SELECT COUNT(*) FROM directories", [], |row| row.get(0))?;
        let stored = metadata
            .get("stored_directory_count")
            .map(String::as_str)
            .unwrap_or("-1");
        if stored.parse::<i64>().ok() != Some(count) {
            return Err(Error::Verification(format!(
                "directory count mismatch: metadata={stored} actual={count}"
            )));
        }
        Ok(metadata)
    };
    check().map_err(|error| match error {
        Error::Verification(_) => error,
        other => Error::Verification(format!("cannot read snapshot database: {other}")),
    })
}

/// Atomically gzip a snapshot and flush its directory.
pub fn gzip_snapshot(source: &Path, destination: &Path) -> Result<()> {
    let mut temporary_name = destination.as_os_str().to_owned();
    temporary_name.push(".tmp");
    let temporary_path = PathBuf::from(temporary_name);
    let mut input = io::BufReader::with_capacity(1 << 20, fs::File::open(source).at(source)?);
    let (temporary, output) = Cleanup::create(&temporary_path, |path| {
        fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(path)
    })
    .at(&temporary_path)?;
    let mut encoder = GzEncoder::new(
        io::BufWriter::with_capacity(1 << 20, output),
        Compression::new(6),
    );
    io::copy(&mut input, &mut encoder).at(temporary.path())?;
    let writer = encoder.finish().at(temporary.path())?;
    let file = writer
        .into_inner()
        .map_err(|error| error.into_error())
        .at(temporary.path())?;
    file.sync_all().at(temporary.path())?;
    fs::rename(temporary.path(), destination).at(destination)?;
    temporary.keep();
    if let Some(parent) = destination.parent() {
        fsync_directory(parent);
    }
    Ok(())
}
