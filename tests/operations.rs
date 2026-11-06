//! Profiles, configuration, retention, integrity, locking, and scheduling.

mod support;

use std::fs;
use std::io::Write;
use std::os::unix::io::AsRawFd;

use serde_json::{Value, json};
use support::{Qdu, Terminal, run_on_terminal};

/// Snapshot schema 2 as released in the Python qdu 0.1.0: directories had
/// a parent column and three secondary indexes.
const SCHEMA_2_SQL: &str = "
CREATE TABLE metadata (key TEXT PRIMARY KEY, value TEXT NOT NULL) WITHOUT ROWID;
CREATE TABLE directories (
    path TEXT PRIMARY KEY, parent TEXT NOT NULL, depth INTEGER NOT NULL,
    allocated_bytes INTEGER NOT NULL, apparent_bytes INTEGER NOT NULL,
    file_count INTEGER NOT NULL, directory_count INTEGER NOT NULL,
    inode_count INTEGER NOT NULL, latest_modified_epoch REAL NOT NULL
) WITHOUT ROWID;
CREATE INDEX directories_allocated_idx ON directories(allocated_bytes DESC, path);
CREATE INDEX directories_inode_idx ON directories(inode_count DESC, path);
CREATE INDEX directories_modified_idx ON directories(latest_modified_epoch, path);
CREATE TABLE owners (
    uid INTEGER PRIMARY KEY, allocated_bytes INTEGER NOT NULL,
    apparent_bytes INTEGER NOT NULL, file_count INTEGER NOT NULL,
    directory_count INTEGER NOT NULL, inode_count INTEGER NOT NULL
) WITHOUT ROWID;
CREATE TABLE owner_directories (
    uid INTEGER NOT NULL, path TEXT NOT NULL, depth INTEGER NOT NULL,
    allocated_bytes INTEGER NOT NULL, apparent_bytes INTEGER NOT NULL,
    file_count INTEGER NOT NULL, directory_count INTEGER NOT NULL,
    inode_count INTEGER NOT NULL, PRIMARY KEY(uid, path)
) WITHOUT ROWID;
CREATE TABLE large_files (
    path TEXT PRIMARY KEY, uid INTEGER NOT NULL, allocated_bytes INTEGER NOT NULL,
    apparent_bytes INTEGER NOT NULL, modified_epoch REAL NOT NULL
) WITHOUT ROWID;
CREATE TABLE scan_errors (
    id INTEGER PRIMARY KEY, path TEXT NOT NULL, operation TEXT NOT NULL,
    message TEXT NOT NULL
);
";

fn item<'a>(items: &'a Value, name: &str) -> &'a Value {
    items
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["name"] == name)
        .unwrap_or_else(|| panic!("no item {name}"))
}

fn metadata(database: &std::path::Path, key: &str) -> String {
    rusqlite::Connection::open(database)
        .unwrap()
        .query_row("SELECT value FROM metadata WHERE key = ?1", [key], |row| {
            row.get(0)
        })
        .unwrap()
}

fn archived(qdu: &Qdu) -> Vec<bool> {
    qdu.records()
        .iter()
        .map(|record| record["archived"].as_bool().unwrap())
        .collect()
}

fn stored_files(qdu: &Qdu) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(qdu.snapshots_dir())
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .collect();
    names.sort();
    names
}

fn indexed_files(qdu: &Qdu) -> Vec<String> {
    let mut names: Vec<String> = qdu
        .records()
        .iter()
        .map(|record| record["filename"].as_str().unwrap().to_string())
        .collect();
    names.sort();
    names
}

#[test]
fn rejects_profile_names_that_can_escape_the_state_directory() {
    let qdu = Qdu::new();
    let absolute = qdu.base.join("escape");
    for name in [
        "../escape",
        absolute.to_str().unwrap(),
        "nested/name",
        "nested\\name",
    ] {
        let run = qdu
            .run(&["profile", "remove", name, "--delete-data"])
            .status(2);
        assert!(
            run.stderr.contains("invalid profile name") && run.stderr.contains("up to 64"),
            "{name}: {}",
            run.stderr
        );
    }
}

#[test]
fn cross_filesystems_snapshot_option_is_recorded() {
    let qdu = Qdu::new();
    qdu.write_file("data/file", 1024);
    qdu.snapshot(&["--cross-filesystems"]).ok();
    assert_eq!(
        metadata(&qdu.latest_database(), "cross_filesystems"),
        "true"
    );
    assert_eq!(
        qdu.json(&["show"])["filesystem"]["mode"],
        "cross-filesystems"
    );
}

#[test]
fn profile_can_persist_cross_filesystems() {
    let qdu = Qdu::new();
    let root = qdu.root.to_str().unwrap();
    qdu.run(&[
        "profile",
        "set",
        "shared",
        "--path",
        root,
        "--cross-filesystems",
    ])
    .ok();
    assert_eq!(
        qdu.json(&["profile", "show", "shared"])["settings"]["cross_filesystems"],
        true
    );
    let latest = |qdu: &Qdu| {
        let index: Value = serde_json::from_str(
            &fs::read_to_string(qdu.profile_dir("shared").join("index.json")).unwrap(),
        )
        .unwrap();
        let filename = index["snapshots"].as_array().unwrap().last().unwrap()["filename"]
            .as_str()
            .unwrap()
            .to_string();
        qdu.profile_dir("shared").join("snapshots").join(filename)
    };
    qdu.run(&["snapshot", "--profile", "shared", "--quiet"])
        .ok();
    assert_eq!(metadata(&latest(&qdu), "cross_filesystems"), "true");
    qdu.run(&[
        "snapshot",
        "--profile",
        "shared",
        "--one-file-system",
        "--quiet",
    ])
    .ok();
    assert_eq!(metadata(&latest(&qdu), "cross_filesystems"), "false");
}

#[test]
fn profile_and_config_commands() {
    let qdu = Qdu::new();
    qdu.run(&["config", "init"]).ok();
    let root = qdu.root.to_str().unwrap();
    qdu.run(&[
        "profile",
        "set",
        "data",
        "--path",
        root,
        "--exclude",
        ".git",
    ])
    .ok();
    let payload = qdu.json(&["profile", "show", "data"])["settings"].clone();
    assert_eq!(payload["name"], "data");
    assert!(
        payload["exclude"]
            .as_array()
            .unwrap()
            .contains(&json!(".git"))
    );
    let listing = qdu.run(&["profile", "list", "--style", "plain"]).ok();
    assert!(listing.stdout.contains("data"));
    let again = qdu.run(&["config", "init"]).ok();
    assert!(
        again.stdout.starts_with("Already has [defaults]: "),
        "{}",
        again.stdout
    );
    let path = qdu.run(&["config", "path"]).ok();
    assert_eq!(path.stdout.trim_end(), qdu.config_file().to_str().unwrap());
}

/// A file written by `profile set` or a first snapshot has no [defaults]
/// section; `config init` adds the starter one and keeps the profiles.
#[test]
fn config_init_adds_defaults_to_an_existing_file() {
    let qdu = Qdu::new();
    let root = qdu.root.to_str().unwrap();
    qdu.run(&["profile", "set", "data", "--path", root]).ok();
    let run = qdu.run(&["config", "init"]).ok();
    assert!(
        run.stdout.starts_with("Added a [defaults] section to "),
        "{}",
        run.stdout
    );
    let text = fs::read_to_string(qdu.config_file()).unwrap();
    assert!(text.starts_with("[defaults]"), "{text}");
    assert!(text.contains("[profile:data]"), "{text}");
    let settings = qdu.json(&["profile", "show", "data"])["settings"].clone();
    assert_eq!(settings["path"], root);
    assert_eq!(settings["exclude"], json!([".git", "node_modules"]));
    let again = qdu.run(&["config", "init"]).ok();
    assert!(again.stdout.starts_with("Already has [defaults]: "));
    assert_eq!(fs::read_to_string(qdu.config_file()).unwrap(), text);
}

#[test]
fn profile_settings_read_like_the_configuration_file() {
    let qdu = Qdu::new();
    let root = qdu.root.to_str().unwrap();
    qdu.run(&[
        "profile",
        "set",
        "data",
        "--path",
        root,
        "--exclude",
        "cache/",
        "--exclude",
        "*.tmp",
        "--capacity-limit",
        "1.5T",
    ])
    .ok();
    let text = fs::read_to_string(qdu.config_file()).unwrap();
    assert!(text.contains("capacity_limit = 1.5TiB"), "{text}");
    let named = qdu.run(&["profile", "show", "data"]).ok().stdout;
    // Without a name, the profile is the one QDU_PROFILE names.
    let current = qdu
        .command(&["profile", "show"])
        .env("QDU_PROFILE", "data")
        .output()
        .unwrap();
    assert_eq!(String::from_utf8_lossy(&current.stdout), named);
    {
        let output = named;
        let lines: Vec<&str> = output.lines().collect();
        for expected in [
            "exclude            cache/",
            "                   *.tmp",
            "record_max_depth   unlimited",
            "collect_users      false",
            "capacity_limit     1.5TiB",
            "capacity_user      not set",
        ] {
            assert!(
                lines.contains(&expected),
                "{expected:?} missing in\n{output}"
            );
        }
        for unexpected in ["None", "False", "["] {
            assert!(!output.contains(unexpected), "{output}");
        }
    }
    let tsv = qdu
        .run(&["profile", "show", "data", "--format", "tsv"])
        .ok()
        .stdout;
    let rows: Vec<Vec<&str>> = tsv.lines().map(|line| line.split('\t').collect()).collect();
    assert_eq!(rows[0], ["setting", "value"]);
    assert!(rows.contains(&vec!["exclude", "cache/"]));
    assert!(rows.contains(&vec!["exclude", "*.tmp"]));
    assert!(rows.contains(&vec!["capacity_user", ""]));
}

#[test]
fn profile_list_counts_snapshots() {
    let mut qdu = Qdu::new();
    qdu.env("COLUMNS", "400"); // keep the temporary root whole
    qdu.write_file("data", 1024);
    qdu.snapshot(&[]).ok();
    qdu.snapshot(&[]).ok();
    let output = qdu
        .run(&["profile", "list", "--style", "plain"])
        .ok()
        .stdout;
    let lines: Vec<&str> = output.lines().collect();
    assert_eq!(lines.len(), 3, "{output}");
    assert_eq!(
        lines[0].split_whitespace().collect::<Vec<_>>(),
        ["Profile", "Snapshots", "Latest", "Capacity", "Root"]
    );
    let pattern =
        regex::Regex::new(r"^default +2 +\d{4}-\d\d-\d\d \d\d:\d\d \(\d+s ago\) +not set +")
            .unwrap();
    assert!(pattern.is_match(lines[2]), "{}", lines[2]);
    // Without a configured path, the profile scans its snapshots' root.
    assert!(
        lines[2].ends_with(qdu.root.to_str().unwrap()),
        "{}",
        lines[2]
    );
    assert!(!lines.iter().any(|line| line.ends_with(' ')));
}

#[test]
fn schema_2_snapshots_remain_readable() {
    let qdu = Qdu::new();
    qdu.write_file("data/file", 1024);
    qdu.snapshot(&[]).ok();
    let current = qdu.latest_database();
    let legacy = current.with_file_name("legacy.tmp");
    {
        let source = rusqlite::Connection::open(&current).unwrap();
        let target = rusqlite::Connection::open(&legacy).unwrap();
        target.execute_batch(SCHEMA_2_SQL).unwrap();
        let mut statement = source.prepare("SELECT key, value FROM metadata").unwrap();
        let rows = statement
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .unwrap();
        for row in rows {
            let (key, value) = row.unwrap();
            let value = if key == "schema_version" {
                "2".to_string()
            } else {
                value
            };
            target
                .execute("INSERT INTO metadata VALUES (?1, ?2)", [key, value])
                .unwrap();
        }
        let mut statement = source
            .prepare(
                "SELECT path, depth, allocated_bytes, apparent_bytes, file_count, \
                 directory_count, inode_count, latest_modified_epoch FROM directories",
            )
            .unwrap();
        let mut rows = statement.query([]).unwrap();
        while let Some(row) = rows.next().unwrap() {
            let path: String = row.get(0).unwrap();
            let parent = path
                .rsplit_once('/')
                .map_or(".", |(parent, _)| parent)
                .to_string();
            target
                .execute(
                    "INSERT INTO directories VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                    rusqlite::params![
                        path,
                        parent,
                        row.get::<_, i64>(1).unwrap(),
                        row.get::<_, i64>(2).unwrap(),
                        row.get::<_, i64>(3).unwrap(),
                        row.get::<_, i64>(4).unwrap(),
                        row.get::<_, i64>(5).unwrap(),
                        row.get::<_, i64>(6).unwrap(),
                        row.get::<_, f64>(7).unwrap(),
                    ],
                )
                .unwrap();
        }
    }
    fs::rename(&legacy, &current).unwrap();
    qdu.run(&["repair"]).ok();
    qdu.write_file("data/file", 65536);
    qdu.snapshot(&[]).ok();
    qdu.run(&["verify", "--all"]).ok();
    let previous = qdu.json(&["show", "data", "--snapshot", "previous"]);
    assert_eq!(previous["scope"], "data");
    assert!(qdu.json(&["diff"])["rows"][0]["change"].as_i64().unwrap() > 0);
}

#[test]
fn profile_add_keeps_omitted_settings_and_defaults() {
    let qdu = Qdu::new();
    qdu.run(&["config", "init"]).ok();
    let text = fs::read_to_string(qdu.config_file()).unwrap();
    fs::write(
        qdu.config_file(),
        text.replace("keep_snapshots = 52", "keep_snapshots = 7"),
    )
    .unwrap();
    let root = qdu.root.to_str().unwrap().to_string();
    let profile = |options: &[&str]| {
        let mut arguments = vec!["profile", "set", "data", "--path", root.as_str()];
        arguments.extend(options);
        qdu.run(&arguments).ok();
        qdu.json(&["profile", "show", "data"])["settings"].clone()
    };
    let created = profile(&["--with-users"]);
    assert_eq!(created["keep_snapshots"], 7);
    assert_eq!(created["exclude"], json!([".git", "node_modules"]));
    assert_eq!(created["collect_users"], true);
    let updated = profile(&["--file-top", "5"]);
    assert_eq!(updated["collect_users"], true);
    assert_eq!(updated["large_file_limit"], 5);
    assert_eq!(updated["keep_snapshots"], 7);
    assert_eq!(profile(&["--without-users"])["collect_users"], false);
    assert_eq!(
        profile(&["--capacity-limit", "1GiB"])["capacity_limit_bytes"],
        1u64 << 30
    );
    assert!(profile(&["--no-capacity-limit"])["capacity_limit_bytes"].is_null());
    let run = qdu
        .run(&[
            "profile",
            "set",
            "data",
            "--path",
            &root,
            "--capacity-user",
            "x",
        ])
        .status(2);
    assert!(run.stderr.contains("capacity_user"), "{}", run.stderr);
    assert!(
        !fs::read_to_string(qdu.config_file())
            .unwrap()
            .contains("capacity_user = x")
    );
}

#[test]
fn profile_list_shows_valid_profiles_next_to_an_invalid_one() {
    let qdu = Qdu::new();
    let root = qdu.root.to_str().unwrap();
    qdu.run(&["profile", "set", "good", "--path", root]).ok();
    let mut file = fs::OpenOptions::new()
        .append(true)
        .open(qdu.config_file())
        .unwrap();
    file.write_all(b"\n[profile:bad]\nkeep_snapshots = many\n")
        .unwrap();
    let run = qdu.run(&["profile", "list", "--style", "plain"]).status(2);
    assert!(run.stdout.contains("good"));
    assert!(run.stdout.contains(root));
    assert!(run.stdout.contains("(invalid configuration)"));
    assert!(
        run.stderr
            .contains("invalid configuration for profile 'bad'"),
        "{}",
        run.stderr
    );
}

#[test]
fn an_empty_profile_variable_means_the_default_profile() {
    let qdu = Qdu::new();
    qdu.write_file("data", 1024);
    qdu.snapshot(&[]).ok();
    let output = qdu
        .command(&["list", "--format", "json"])
        .env("QDU_PROFILE", "")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let listed: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(listed["profile"], "default");
}

#[test]
fn removing_a_profile_says_where_its_snapshots_went() {
    let qdu = Qdu::new();
    let root = qdu.root.to_str().unwrap();
    qdu.run(&["snapshot", "--profile", "kept", "--path", root, "--quiet"])
        .ok();
    let run = qdu.run(&["profile", "remove", "kept"]).ok();
    assert!(
        run.stdout.contains("its snapshots remain in"),
        "{}",
        run.stdout
    );
    let run = qdu
        .run(&["profile", "remove", "kept", "--delete-data"])
        .ok();
    assert!(
        run.stdout.contains("its snapshots were deleted"),
        "{}",
        run.stdout
    );
    assert!(!qdu.profile_dir("kept").exists());
}

#[test]
fn profile_set_changes_only_the_given_settings() {
    let qdu = Qdu::new();
    let root = qdu.root.to_str().unwrap();
    let run = qdu
        .run(&["profile", "set", "data", "--with-users"])
        .status(2);
    assert!(run.stderr.contains("--path"), "{}", run.stderr);
    // `add` is another name for `set`.
    qdu.run(&["profile", "add", "data", "--path", root]).ok();
    qdu.run(&["profile", "set", "data", "--with-users"]).ok();
    let settings = qdu.json(&["profile", "show", "data"])["settings"].clone();
    assert_eq!(settings["path"], root);
    assert_eq!(settings["collect_users"], true);
}

#[test]
fn profile_remove_can_delete_the_data() {
    let qdu = Qdu::new();
    let root = qdu.root.to_str().unwrap();
    qdu.run(&["profile", "set", "data", "--path", root]).ok();
    qdu.run(&["snapshot", "--profile", "data", "--quiet"]).ok();
    assert!(qdu.profile_dir("data").exists());
    let run = qdu
        .run(&["profile", "remove", "data", "--delete-data"])
        .ok();
    assert_eq!(run.stdout, "Profile removed: data, with its snapshots\n");
    assert!(!qdu.profile_dir("data").exists());
    let again = qdu.run(&["profile", "remove", "data"]).ok();
    assert_eq!(again.stdout, "Profile was not configured: data\n");
}

/// Lock a file the way qdu does, holding the lock until the file closes.
fn hold_lock(path: &std::path::Path) -> fs::File {
    let file = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)
        .unwrap();
    // SAFETY: flock only reads the descriptor that `file` owns.
    assert_eq!(
        unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) },
        0
    );
    file
}

#[test]
fn removing_a_busy_profile_changes_nothing() {
    let qdu = Qdu::new();
    let root = qdu.root.to_str().unwrap();
    qdu.run(&["profile", "set", "data", "--path", root]).ok();
    qdu.run(&["snapshot", "--profile", "data", "--quiet"]).ok();
    let lock = hold_lock(&qdu.profile_dir("data").join(".lock"));
    qdu.run(&["profile", "remove", "data", "--delete-data"])
        .status(5);
    drop(lock);
    assert!(qdu.profile_dir("data").exists());
    let config = fs::read_to_string(qdu.config_file()).unwrap();
    assert!(config.contains("[profile:data]"), "{config}");
}

/// A process that waited for the lock of a file that was removed in the
/// meantime does not take that lock as the profile's.
#[test]
fn a_lock_on_a_removed_lock_file_is_not_trusted() {
    let qdu = Qdu::new();
    qdu.write_file("data", 1024);
    let directory = qdu.profile_dir("default");
    fs::create_dir_all(&directory).unwrap();
    let path = directory.join(".lock");
    let old = hold_lock(&path);
    let root = qdu.root.to_str().unwrap().to_string();
    let child = qdu
        .command(&["snapshot", "--path", &root, "--quiet"])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    std::thread::sleep(std::time::Duration::from_millis(300));
    fs::remove_file(&path).unwrap();
    let current = hold_lock(&path);
    drop(old);
    let output = child.wait_with_output().unwrap();
    drop(current);
    assert_eq!(
        output.status.code(),
        Some(5),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn completions_are_printed_for_each_shell() {
    let qdu = Qdu::new();
    for (shell, marker) in [
        ("bash", "complete -F"),
        ("zsh", "#compdef qdu"),
        ("fish", "complete -c qdu"),
    ] {
        let run = qdu.run(&["completions", shell]).ok();
        assert!(
            run.stdout.contains(marker),
            "{shell}: {}",
            &run.stdout[..200.min(run.stdout.len())]
        );
        assert!(run.stdout.contains("snapshot"), "{shell}");
    }
    qdu.run(&["completions", "tcsh"]).status(2);
}

#[test]
fn repair_leaves_what_qdu_did_not_write() {
    let qdu = Qdu::new();
    qdu.write_file("data", 1024);
    qdu.snapshot(&[]).ok();
    let temporary = qdu.profile_dir("default").join("tmp");
    fs::write(temporary.join("left.sqlite3.tmp"), b"partial").unwrap();
    fs::create_dir(temporary.join("someone-else")).unwrap();
    let run = qdu.run(&["repair"]).ok();
    assert!(
        run.stdout
            .contains("remove temporary file: left.sqlite3.tmp"),
        "{}",
        run.stdout
    );
    assert!(run.stdout.contains("leave someone-else"), "{}", run.stdout);
    assert!(temporary.join("someone-else").exists());
    assert!(!temporary.join("left.sqlite3.tmp").exists());
}

#[test]
fn forget_removes_the_chosen_snapshots() {
    let qdu = Qdu::new();
    qdu.write_file("data", 1024);
    for _ in 0..3 {
        qdu.snapshot(&[]).ok();
    }
    let ids = qdu.snapshot_ids();
    let dry = qdu.run(&["forget", "latest~2", "--dry-run"]).ok();
    assert!(dry.stdout.contains(&ids[0]), "{}", dry.stdout);
    assert_eq!(qdu.snapshot_ids(), ids);
    let run = qdu.run(&["forget", "latest~2", &ids[1], "latest~2"]).ok();
    assert_eq!(run.stdout.matches("Removed").count(), 2, "{}", run.stdout);
    assert_eq!(qdu.snapshot_ids(), [ids[2].clone()]);
    let files: Vec<_> = fs::read_dir(qdu.snapshots_dir()).unwrap().collect();
    assert_eq!(files.len(), 1);
    qdu.run(&["forget", "@nothing"]).status(2);
    qdu.run(&["forget"]).status(2);
    qdu.run(&["verify", "--all"]).ok();
}

#[test]
fn a_file_that_is_not_a_snapshot_is_a_format_error() {
    let qdu = Qdu::new();
    qdu.write_file("data", 1024);
    qdu.snapshot(&[]).ok();
    let database = qdu.latest_database();
    fs::write(&database, b"not a database at all, just text\n".repeat(100)).unwrap();
    let run = qdu.run(&["show"]).status(4);
    assert!(run.stderr.contains("qdu verify"), "{}", run.stderr);
}

#[test]
fn verify_detects_corruption() {
    let qdu = Qdu::new();
    qdu.write_file("data", 1024);
    qdu.snapshot(&[]).ok();
    qdu.run(&["verify", "--all"]).ok();
    let mut file = fs::OpenOptions::new()
        .append(true)
        .open(qdu.latest_database())
        .unwrap();
    file.write_all(b"corruption").unwrap();
    let run = qdu.run(&["verify", "--all"]).status(6);
    assert!(
        run.stderr.to_lowercase().contains("failed verification"),
        "{}",
        run.stderr
    );
}

#[test]
fn repair_rebuilds_a_missing_index() {
    let qdu = Qdu::new();
    qdu.write_file("data", 1024);
    qdu.snapshot(&[]).ok();
    fs::remove_file(qdu.index_path()).unwrap();
    qdu.run(&["repair"]).ok();
    assert_eq!(qdu.records().len(), 1);
}

#[test]
fn compact_archives_and_archived_snapshots_remain_readable() {
    let qdu = Qdu::new();
    qdu.write_file("data", 1024);
    qdu.snapshot(&[]).ok();
    qdu.write_file("data", 8192);
    qdu.snapshot(&[]).ok();
    let first = qdu.snapshot_ids()[0].clone();
    let options = ["compact", "--older-than", "0s", "--keep-latest", "1"];
    let dry = qdu.run(&[&options[..], &["--dry-run"]].concat()).ok();
    assert_eq!(
        dry.stdout,
        format!("Would compress 1 snapshot(s):\n  {first}\n")
    );
    assert_eq!(archived(&qdu), [false, false]);
    let run = qdu.run(&options).ok();
    assert!(
        run.stdout.contains("Compressed 1 snapshot(s)"),
        "{}",
        run.stdout
    );
    assert_eq!(archived(&qdu), [true, false]);
    let again = qdu.run(&options).ok();
    assert!(
        again.stdout.starts_with("Nothing to compress"),
        "{}",
        again.stdout
    );
    let previous = qdu.json(&["show", "--snapshot", "previous"]);
    assert_eq!(previous["snapshot"]["snapshot_id"], first.as_str());
}

#[test]
fn interrupted_compaction_keeps_the_index_consistent() {
    let qdu = Qdu::new();
    for size in [1024, 2048, 4096] {
        qdu.write_file("data", size);
        qdu.snapshot(&[]).ok();
    }
    // A directory where the second archive would be written makes it fail.
    let second = qdu.records()[1]["filename"].as_str().unwrap().to_string();
    let obstacle = qdu.snapshots_dir().join(format!("{second}.gz.tmp"));
    fs::create_dir(&obstacle).unwrap();
    let options = ["compact", "--older-than", "0s", "--keep-latest", "1"];
    let run = qdu.run(&options).status(1);
    assert!(run.stderr.contains(".gz.tmp"), "{}", run.stderr);
    qdu.run(&["verify", "--all"]).ok();
    fs::remove_dir(&obstacle).unwrap();
    qdu.run(&options).ok();
    assert_eq!(archived(&qdu), [true, true, false]);
    assert_eq!(stored_files(&qdu), indexed_files(&qdu));
    qdu.run(&["verify", "--all"]).ok();
}

#[test]
fn repair_removes_an_archive_left_by_an_interrupted_compaction() {
    let qdu = Qdu::new();
    qdu.write_file("data", 1024);
    qdu.snapshot(&[]).ok();
    let filename = qdu.records()[0]["filename"].as_str().unwrap().to_string();
    let source = qdu.snapshots_dir().join(&filename);
    let archive = qdu.snapshots_dir().join(format!("{filename}.gz"));
    {
        let mut encoder = flate2::write::GzEncoder::new(
            fs::File::create(&archive).unwrap(),
            flate2::Compression::default(),
        );
        encoder.write_all(&fs::read(&source).unwrap()).unwrap();
        encoder.finish().unwrap();
    }
    let dry = qdu.run(&["repair", "--dry-run"]).ok();
    assert!(
        dry.stdout
            .contains(&format!("remove duplicate archive: {filename}.gz")),
        "{}",
        dry.stdout
    );
    qdu.run(&["repair"]).ok();
    let records = qdu.records();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0]["filename"], filename.as_str());
    assert_eq!(records[0]["archived"], false);
    assert_eq!(stored_files(&qdu), [filename]);
}

#[test]
fn pruning_survives_a_file_that_cannot_be_deleted() {
    let qdu = Qdu::new();
    qdu.write_file("data", 1024);
    qdu.snapshot(&["--keep-snapshots", "1"]).ok();
    let oldest = qdu.records()[0]["filename"].as_str().unwrap().to_string();
    // A non-empty directory under the snapshot's name cannot be unlinked.
    let path = qdu.snapshots_dir().join(&oldest);
    fs::remove_file(&path).unwrap();
    fs::create_dir(&path).unwrap();
    fs::write(path.join("keep"), b"x").unwrap();
    qdu.snapshot(&["--keep-snapshots", "1"]).ok();
    assert!(!indexed_files(&qdu).contains(&oldest));
    qdu.run(&["verify", "--all"]).ok();
}

#[test]
fn snapshots_compress_older_snapshots_automatically() {
    let qdu = Qdu::new();
    qdu.write_file("data", 1024);
    for _ in 0..4 {
        qdu.snapshot(&["--compact-after", "0s"]).ok();
    }
    assert_eq!(archived(&qdu), [true, true, false, false]);
    assert_eq!(stored_files(&qdu), indexed_files(&qdu));
    let root = qdu.root.to_str().unwrap();
    let run = qdu
        .run(&["snapshot", "--path", root, "--compact-after", "0s"])
        .ok();
    assert!(
        run.stdout.contains("compressed 1 older snapshot(s)"),
        "{}",
        run.stdout
    );
    qdu.snapshot(&["--compact-after", "off"]).ok();
    assert_eq!(archived(&qdu), [true, true, true, false, false, false]);
    qdu.run(&["verify", "--all"]).ok();
}

#[test]
fn retention_defaults_and_invalid_compaction_ages() {
    let qdu = Qdu::new();
    let settings = qdu.json(&["profile", "show"])["settings"].clone();
    assert_eq!(settings["keep_snapshots"], 52);
    assert_eq!(settings["compact_after"], "7d");
    qdu.snapshot(&["--compact-after", "soon"]).status(2);
}

#[test]
fn archives_expand_privately_and_corrupt_archives_are_reported() {
    let mut qdu = Qdu::new();
    qdu.write_file("data/file", 1024);
    qdu.snapshot(&[]).ok();
    qdu.write_file("data/file", 8192);
    qdu.snapshot(&[]).ok();
    qdu.run(&["compact", "--older-than", "0s", "--keep-latest", "1"])
        .ok();
    let temporary = qdu.base.join("tmp");
    fs::create_dir(&temporary).unwrap();
    qdu.env("TMPDIR", temporary.to_str().unwrap());
    qdu.json(&["diff"]);
    assert_eq!(fs::read_dir(&temporary).unwrap().count(), 0);
    let profile_temporary = qdu.profile_dir("default").join("tmp");
    assert_eq!(fs::read_dir(profile_temporary).unwrap().count(), 0);
    let archive = qdu
        .snapshots_dir()
        .join(qdu.records()[0]["filename"].as_str().unwrap());
    let bytes = fs::read(&archive).unwrap();
    fs::write(&archive, &bytes[..100]).unwrap();
    let run = qdu.run(&["verify", "--all", "--format", "json"]).status(6);
    let statuses: Vec<String> = run.json()["rows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["status"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(statuses, ["FAILED", "OK"]);
    let repair = qdu.run(&["repair"]).ok();
    assert!(
        repair.stdout.contains("skip invalid snapshot"),
        "{}",
        repair.stdout
    );
}

/// A fake crontab that keeps the table in `$CRONTAB_FILE`.
fn fake_crontab(qdu: &mut Qdu) -> std::path::PathBuf {
    let crontab = qdu.base.join("crontab.txt");
    qdu.script(
        "crontab",
        "if [ \"$1\" = \"-l\" ]; then\n\
         \x20 [ -f \"$CRONTAB_FILE\" ] || { echo \"no crontab for test\" >&2; exit 1; }\n\
         \x20 cat \"$CRONTAB_FILE\"\n\
         else cat > \"$CRONTAB_FILE\"; fi\n",
    );
    let path = qdu.path_with_scripts();
    qdu.env("PATH", path);
    qdu.env("CRONTAB_FILE", crontab.to_str().unwrap());
    crontab
}

#[test]
fn unknown_profiles_are_refused_with_a_suggestion() {
    let mut qdu = Qdu::new();
    let crontab = fake_crontab(&mut qdu);
    let root = qdu.root.to_str().unwrap();
    qdu.run(&["profile", "set", "data", "--path", root]).ok();
    for command in [
        &["show"][..],
        &["snapshot"][..],
        &["schedule", "--install"][..],
    ] {
        let run = qdu
            .run(&[command, &["--profile", "dta"][..]].concat())
            .status(2);
        assert!(
            run.stderr.contains("unknown profile 'dta'"),
            "{}",
            run.stderr
        );
        assert!(
            run.stderr.contains("did you mean 'data'?"),
            "{}",
            run.stderr
        );
    }
    assert!(!crontab.exists());
    let run = qdu.run(&["profile", "show", "dta"]).status(2);
    assert!(
        run.stderr.contains("did you mean 'data'?"),
        "{}",
        run.stderr
    );
    // Without a close name, the message only says how to create one.
    let run = qdu.run(&["show", "--profile", "unrelated"]).status(2);
    assert!(
        run.stderr
            .contains("unknown profile 'unrelated'. Create it with 'qdu profile set unrelated"),
        "{}",
        run.stderr
    );
    // A new profile may start with a snapshot of a given directory.
    qdu.run(&["snapshot", "--profile", "fresh", "--path", root, "--quiet"])
        .ok();
    // A profile with snapshots but no configuration is still listed.
    qdu.run(&["profile", "remove", "fresh"]).ok();
    let listed = qdu.json(&["profile", "list"]);
    let names: Vec<&str> = listed["rows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| row["profile"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["data", "default", "fresh"]);
    qdu.run(&["show", "--profile", "fresh"]).ok();
}

#[test]
fn other_crontab_lines_keep_their_bytes() {
    let mut qdu = Qdu::new();
    let crontab = fake_crontab(&mut qdu);
    let other = b"0 1 * * * echo caf\xe9 # latin-1\n".to_vec();
    fs::write(&crontab, &other).unwrap();
    qdu.run(&["schedule", "--qdu", "/opt/qdu", "--install"])
        .ok();
    assert!(fs::read(&crontab).unwrap().starts_with(&other));
    qdu.run(&["schedule", "--remove"]).ok();
    assert_eq!(fs::read(&crontab).unwrap(), other);
}

#[test]
fn removing_a_profile_removes_its_schedule() {
    let mut qdu = Qdu::new();
    let crontab = fake_crontab(&mut qdu);
    let root = qdu.root.to_str().unwrap();
    qdu.run(&["profile", "set", "data", "--path", root]).ok();
    qdu.run(&[
        "schedule",
        "--profile",
        "data",
        "--qdu",
        "/opt/qdu",
        "--install",
    ])
    .ok();
    assert!(fs::read_to_string(&crontab).unwrap().contains("# qdu:data"));
    let run = qdu.run(&["profile", "remove", "data"]).ok();
    assert!(run.stdout.contains("scheduled snapshot"), "{}", run.stdout);
    assert!(!fs::read_to_string(&crontab).unwrap().contains("# qdu:data"));
}

#[test]
fn schedule_prints_installs_and_removes_crontab_entries() {
    let mut qdu = Qdu::new();
    let crontab = qdu.base.join("crontab.txt");
    fs::write(&crontab, "0 1 * * * /usr/bin/true\n").unwrap();
    qdu.script(
        "crontab",
        "if [ \"$1\" = \"-l\" ]; then\n\
         \x20 [ -f \"$CRONTAB_FILE\" ] || { echo \"no crontab for test\" >&2; exit 1; }\n\
         \x20 cat \"$CRONTAB_FILE\"\n\
         else cat > \"$CRONTAB_FILE\"; fi\n",
    );
    let path = qdu.path_with_scripts();
    qdu.env("PATH", path);
    qdu.env("CRONTAB_FILE", crontab.to_str().unwrap());
    let executable = qdu.base.join("qdu%bin");
    let executable = executable.to_str().unwrap();
    let printed = qdu.run(&["schedule", "--qdu", executable]).ok();
    let entry = printed.stdout.trim().lines().last().unwrap().to_string();
    assert!(entry.starts_with("15 3 * * 1 XDG_CONFIG_HOME="), "{entry}");
    assert!(
        entry.contains(" snapshot --profile default --quiet >>"),
        "{entry}"
    );
    assert!(entry.contains("qdu\\%bin"), "{entry}");
    assert!(entry.ends_with("2>&1 # qdu:default"), "{entry}");
    assert!(!fs::read_to_string(&crontab).unwrap().contains("qdu"));
    for every in ["daily", "weekly"] {
        qdu.run(&[
            "schedule",
            "--qdu",
            executable,
            "--every",
            every,
            "--at",
            "04:30",
            "--install",
        ])
        .ok();
    }
    let text = fs::read_to_string(&crontab).unwrap();
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), 2, "{text}");
    assert_eq!(lines[0], "0 1 * * * /usr/bin/true");
    assert!(lines[1].starts_with("30 4 * * 1 "), "{}", lines[1]);
    let schedule = |qdu: &Qdu| item(&qdu.json(&["doctor"])["rows"], "Schedule").clone();
    assert_eq!(
        schedule(&qdu),
        json!({"name": "Schedule", "status": "OK", "detail": "cron 30 4 * * 1"})
    );
    qdu.run(&["schedule", "--remove"]).ok();
    assert_eq!(
        fs::read_to_string(&crontab).unwrap(),
        "0 1 * * * /usr/bin/true\n"
    );
    assert_eq!(schedule(&qdu)["status"], "INFO");
    fs::remove_file(&crontab).unwrap();
    let run = qdu.run(&["schedule", "--remove"]).ok();
    assert!(
        run.stdout.contains("No scheduled snapshot"),
        "{}",
        run.stdout
    );
}

#[test]
fn schedule_uses_the_running_executable_by_default() {
    let qdu = Qdu::new();
    let run = qdu.run(&["schedule"]).ok();
    assert!(
        run.stdout.contains(env!("CARGO_BIN_EXE_qdu")),
        "{}",
        run.stdout
    );
    assert_eq!(run.stderr, "");
}

#[test]
fn schedule_keeps_the_link_on_path_and_warns_about_uv_cache() {
    let qdu = Qdu::new();
    for directory in ["bin", "cache/archive-v0/abc/bin"] {
        let path = qdu.base.join(directory);
        fs::create_dir_all(&path).unwrap();
        std::os::unix::fs::symlink(env!("CARGO_BIN_EXE_qdu"), path.join("qdu")).unwrap();
    }
    let run = |program: &std::path::Path| {
        std::process::Command::new(program)
            .arg("schedule")
            .env("PATH", qdu.path_with_scripts())
            .env("HOME", &qdu.home)
            .env("XDG_STATE_HOME", &qdu.state)
            .env("XDG_CONFIG_HOME", &qdu.config)
            .output()
            .unwrap()
    };
    // The link that `uv tool install` and pipx put on PATH stays in the
    // entry, whether qdu was started by name or by path.
    let linked = qdu.base.join("bin/qdu");
    for program in [std::path::Path::new("qdu"), linked.as_path()] {
        let output = run(program);
        let entry = String::from_utf8_lossy(&output.stdout);
        assert!(
            entry.contains(&format!(" {} snapshot", linked.display())),
            "{entry}"
        );
        assert!(output.stderr.is_empty());
    }
    let output = run(&qdu.base.join("cache/archive-v0/abc/bin/qdu"));
    let warning = String::from_utf8_lossy(&output.stderr);
    assert!(warning.contains("uv tool install qdu"), "{warning}");
}

#[test]
fn check_returns_the_alert_exit_code() {
    let qdu = Qdu::new();
    qdu.write_file("data", 1024);
    qdu.snapshot(&[]).ok();
    let run = qdu
        .run(&["check", "--disk-usage-over", "0", "--format", "json"])
        .status(10);
    assert_eq!(run.json()["passed"], false);
    assert_eq!(run.stderr, "");
    qdu.run(&["check"]).status(2);
}

#[test]
fn the_profile_lock_is_shared_between_processes() {
    let qdu = Qdu::new();
    qdu.write_file("data", 1024);
    let directory = qdu.profile_dir("default");
    fs::create_dir_all(&directory).unwrap();
    let mut lock = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(true)
        .open(directory.join(".lock"))
        .unwrap();
    // SAFETY: flock only reads the descriptor that `lock` owns.
    assert_eq!(
        unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) },
        0
    );
    let started = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let holder = json!({"pid": std::process::id(), "host": "testhost",
                        "started_epoch": started, "command": "test holder"});
    writeln!(lock, "{holder}").unwrap();
    let run = qdu.snapshot(&[]).status(5);
    let pattern = regex::Regex::new(&format!(
        r"another qdu operation is using this profile: test holder \(pid {} on testhost since \d{{4}}-\d\d-\d\d \d\d:\d\d\)",
        std::process::id()
    ))
    .unwrap();
    assert!(pattern.is_match(&run.stderr), "{}", run.stderr);
    let doctor = qdu.json(&["doctor"]);
    let lock_item = item(&doctor["rows"], "Lock");
    assert_eq!(lock_item["status"], "BUSY");
    assert!(
        lock_item["detail"]
            .as_str()
            .unwrap()
            .starts_with("test holder (pid ")
    );
    drop(lock);
    qdu.snapshot(&[]).ok();
}

#[test]
fn growth_check_and_exclusion_difference_warning() {
    let qdu = Qdu::new();
    qdu.write_file("data/file", 1024);
    qdu.snapshot(&["--exclude", ".git"]).ok();
    qdu.write_file("data/file", 32768);
    qdu.snapshot(&["--exclude", "node_modules"]).ok();
    let run = qdu.run(&["diff", "--format", "json"]).ok();
    assert!(
        run.stderr.contains("exclusion patterns differ"),
        "{}",
        run.stderr
    );
    let check = qdu
        .run(&["check", "data", "--growth-over", "1B", "--format", "json"])
        .status(10);
    assert_eq!(check.json()["passed"], false);
}

#[test]
fn growth_check_requires_the_path_in_one_snapshot() {
    let qdu = Qdu::new();
    qdu.write_file("data/file", 1024);
    qdu.snapshot(&[]).ok();
    qdu.write_file("created/file", 32768);
    qdu.snapshot(&[]).ok();
    let run = qdu
        .run(&["check", "no/such/dir", "--growth-over", "1B"])
        .status(2);
    assert!(
        run.stderr.contains("absent from both snapshots"),
        "{}",
        run.stderr
    );
    let check = qdu
        .run(&[
            "check",
            "created",
            "--growth-over",
            "1B",
            "--format",
            "json",
        ])
        .status(10);
    assert_eq!(check.json()["passed"], false);
}

#[test]
fn snapshot_age_check_alerts_on_stale_snapshots() {
    let qdu = Qdu::new();
    qdu.write_file("data", 1024);
    qdu.snapshot(&[]).ok();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    for (days, expected) in [(1u64, 0), (10, 10)] {
        let mut index = qdu.index();
        index["snapshots"][0]["created_epoch"] = json!(now - days * 86400);
        qdu.write_index(&index);
        let run = qdu
            .run(&["check", "--snapshot-age-over", "8d", "--format", "json"])
            .status(expected);
        let check = run.json()["rows"][0].clone();
        assert_eq!(check["name"], "snapshot_age");
        assert_eq!(check["observed"], days * 86400);
        assert_eq!(check["threshold"], 8 * 86400);
        assert_eq!(check["unit"], "seconds");
        assert_eq!(check["observed_text"], format!("{days}d"));
        assert_eq!(check["threshold_text"], "8d");
    }
    let run = qdu.run(&["check", "--snapshot-age-over", "soon"]).status(2);
    assert!(run.stderr.contains("invalid duration"), "{}", run.stderr);
}

#[test]
fn metadata_left_by_a_crashed_process_does_not_block() {
    let qdu = Qdu::new();
    let directory = qdu.profile_dir("default");
    fs::create_dir_all(&directory).unwrap();
    fs::write(
        directory.join(".lock"),
        "{\"pid\": 999999, \"host\": \"stale\"}\n",
    )
    .unwrap();
    qdu.write_file("data", 1024);
    qdu.snapshot(&[]).ok();
    let doctor = qdu.json(&["doctor"]);
    assert_eq!(
        item(&doctor["rows"], "Lock"),
        &json!({"name": "Lock", "status": "OK", "detail": "not held"})
    );
}

#[test]
fn repair_dry_run_preserves_the_index() {
    let qdu = Qdu::new();
    qdu.write_file("data", 1024);
    qdu.snapshot(&[]).ok();
    let before = fs::read(qdu.index_path()).unwrap();
    let run = qdu.run(&["repair", "--dry-run"]).ok();
    assert_eq!(fs::read(qdu.index_path()).unwrap(), before);
    assert!(
        run.stdout.contains("dry run; nothing was changed"),
        "{}",
        run.stdout
    );
    assert!(!run.stdout.contains("lock"));
}

#[test]
fn repair_removes_partial_files() {
    let qdu = Qdu::new();
    qdu.write_file("data", 1024);
    qdu.snapshot(&[]).ok();
    let filename = qdu.records()[0]["filename"].as_str().unwrap().to_string();
    let partial = qdu.snapshots_dir().join(format!("{filename}.gz.tmp"));
    fs::write(&partial, b"partial").unwrap();
    let dry = qdu.run(&["repair", "--dry-run"]).ok();
    assert!(
        dry.stdout
            .contains(&format!("remove temporary file: {filename}.gz.tmp")),
        "{}",
        dry.stdout
    );
    assert!(!dry.stdout.contains("skip invalid"), "{}", dry.stdout);
    assert!(partial.exists());
    qdu.run(&["repair"]).ok();
    assert!(!partial.exists());
    assert_eq!(stored_files(&qdu), [filename]);
}

#[test]
fn a_lock_released_within_two_seconds_is_waited_for() {
    let qdu = Qdu::new();
    qdu.write_file("data", 1024);
    let directory = qdu.profile_dir("default");
    fs::create_dir_all(&directory).unwrap();
    let lock = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(true)
        .open(directory.join(".lock"))
        .unwrap();
    // SAFETY: flock only reads the descriptor that `lock` owns.
    assert_eq!(
        unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) },
        0
    );
    let root = qdu.root.to_str().unwrap().to_string();
    let child = qdu
        .command(&["snapshot", "--path", &root, "--quiet"])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    std::thread::sleep(std::time::Duration::from_millis(300));
    drop(lock);
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn inode_usage_is_checked() {
    let qdu = Qdu::new();
    qdu.write_file("data", 1024);
    qdu.snapshot(&[]).ok();
    let run = qdu
        .run(&["check", "--inode-usage-over", "100", "--format", "json"])
        .ok();
    let check = run.json()["rows"][0].clone();
    assert_eq!(check["name"], "inode_usage");
    assert_eq!(check["passed"], true);
    qdu.run(&["check", "--inode-usage-over", "101"]).status(2);
}

#[test]
fn schedule_entries_follow_the_period() {
    let qdu = Qdu::new();
    let entry = |options: &[&str]| {
        let arguments = [&["schedule", "--qdu", "/opt/qdu"][..], options].concat();
        qdu.run(&arguments)
            .ok()
            .stdout
            .trim()
            .lines()
            .last()
            .unwrap()
            .to_string()
    };
    assert!(entry(&[]).starts_with("15 3 * * 1 "));
    assert!(entry(&["--weekday", "sun"]).starts_with("15 3 * * 0 "));
    assert!(entry(&["--every", "daily", "--at", "23:05"]).starts_with("5 23 * * * "));
    assert!(entry(&["--every", "monthly"]).starts_with("15 3 1 * * "));
    qdu.run(&["schedule", "--at", "24:00"]).status(2);
    for every in ["daily", "monthly"] {
        let run = qdu
            .run(&["schedule", "--every", every, "--weekday", "sun"])
            .status(2);
        assert!(run.stderr.contains("--every weekly"), "{}", run.stderr);
    }
}

#[test]
fn configuration_values_are_read_strictly() {
    let qdu = Qdu::new();
    fs::create_dir_all(qdu.config_file().parent().unwrap()).unwrap();
    fs::write(qdu.config_file(), "[defaults]\ncollect_users = maybe\n").unwrap();
    let run = qdu.run(&["profile", "show"]).status(2);
    assert!(run.stderr.contains("use true or false"), "{}", run.stderr);
    fs::write(qdu.config_file(), "[defaults]\ncollect_users = Yes\n").unwrap();
    let settings = qdu.json(&["profile", "show"])["settings"].clone();
    assert_eq!(settings["collect_users"], true);
    let root = qdu.root.to_str().unwrap();
    for pattern in ["#tmp", " cache"] {
        let run = qdu
            .run(&[
                "profile",
                "set",
                "data",
                "--path",
                root,
                "--exclude",
                pattern,
            ])
            .status(2);
        assert!(run.stderr.contains("exclusion pattern"), "{}", run.stderr);
    }
}

#[test]
fn malformed_configuration_is_a_usage_error() {
    let qdu = Qdu::new();
    fs::create_dir_all(qdu.config_file().parent().unwrap()).unwrap();
    fs::write(
        qdu.config_file(),
        "[defaults]\nkeep_snapshots = not-a-number\n",
    )
    .unwrap();
    let run = qdu.run(&["snapshot"]).status(2);
    assert!(
        run.stderr.contains("invalid configuration"),
        "{}",
        run.stderr
    );
    // doctor reports it and goes on, without creating the state directory.
    let doctor = qdu.run(&["doctor", "--format", "json"]).status(1);
    let config = item(&doctor.json()["rows"], "Config").clone();
    assert_eq!(config["status"], "FAILED");
    assert!(
        config["detail"]
            .as_str()
            .unwrap()
            .contains("invalid configuration")
    );
    assert!(!qdu.profile_dir("default").exists());
    // So it does for a named profile, which it cannot look up.
    fs::write(qdu.config_file(), "[profile:data]\npath\n").unwrap();
    let doctor = qdu
        .run(&["doctor", "--profile", "data", "--format", "json"])
        .status(1);
    assert_eq!(item(&doctor.json()["rows"], "Config")["status"], "FAILED");
}

#[test]
fn read_only_commands_create_no_state() {
    let qdu = Qdu::new();
    qdu.run(&["doctor"]).ok();
    qdu.run(&["compact", "--dry-run"]).ok();
    qdu.run(&["list"]).ok();
    assert!(!qdu.profile_dir("default").exists());
}

#[test]
fn no_arguments_show_the_latest_snapshot() {
    let qdu = Qdu::new();
    qdu.write_file("data/file", 1024);
    qdu.snapshot(&[]).ok();
    let plain = qdu.run(&[]).ok();
    assert!(plain.stdout.starts_with("Disk usage\n"), "{}", plain.stdout);
    let version = qdu.run(&["--version"]).ok();
    assert_eq!(
        version.stdout,
        format!("qdu {}\n", env!("CARGO_PKG_VERSION"))
    );
}

#[test]
fn no_color_turns_automatic_color_off() {
    let qdu = Qdu::new();
    let doctor = |no_color: Option<&str>| {
        let mut command = qdu.command(&["doctor", "--style", "plain"]);
        if let Some(value) = no_color {
            command.env("NO_COLOR", value);
        }
        run_on_terminal(command, Terminal::Stdout).map(|(_, written, _)| written)
    };
    let Some(colored) = doctor(None) else {
        return;
    };
    assert!(colored.contains("\x1b[32m"), "{colored:?}");
    let plain = doctor(Some("1")).unwrap();
    assert!(!plain.contains("\x1b["), "{plain:?}");
    // An empty NO_COLOR does not count.
    assert!(doctor(Some("")).unwrap().contains("\x1b[32m"));
}
