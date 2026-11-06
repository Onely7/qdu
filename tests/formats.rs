//! The output layout that scripts rely on: TSV headers, JSON keys and their
//! order, and table titles. A change here is a change of FORMAT_VERSION or
//! of documented keys.

mod support;

use serde_json::Value;
use support::Qdu;

/// A profile with two snapshots of a small tree, with owner statistics.
fn sample() -> Qdu {
    let qdu = Qdu::new();
    qdu.write_file("a/b/data.bin", 8192);
    qdu.write_file("c/small", 1024);
    qdu.snapshot(&["--with-users"]).ok();
    qdu.write_file("c/more", 4096);
    qdu.snapshot(&["--with-users"]).ok();
    qdu
}

fn tsv_header(qdu: &Qdu, arguments: &[&str]) -> Vec<String> {
    let mut arguments = arguments.to_vec();
    arguments.extend(["--format", "tsv"]);
    let run = qdu.run(&arguments);
    assert!(matches!(run.status, 0 | 10), "{}", run.stderr);
    run.stdout
        .lines()
        .next()
        .unwrap()
        .split('\t')
        .map(str::to_string)
        .collect()
}

fn keys(value: &Value) -> Vec<String> {
    value.as_object().unwrap().keys().cloned().collect()
}

fn json(qdu: &Qdu, arguments: &[&str]) -> Value {
    let mut arguments = arguments.to_vec();
    arguments.extend(["--format", "json"]);
    let run = qdu.run(&arguments);
    assert!(matches!(run.status, 0 | 10), "{}", run.stderr);
    run.json()
}

/// The JSON keys after the common envelope, and the keys of the first row,
/// which must be the TSV header.
#[track_caller]
fn assert_layout(qdu: &Qdu, arguments: &[&str], context: &[&str], row: &[&str]) {
    let payload = json(qdu, arguments);
    assert_eq!(payload["format_version"], 1);
    let command = if arguments[0] == "profile" {
        arguments[..2].join(" ")
    } else {
        arguments[0].to_string()
    };
    assert_eq!(payload["command"], command.as_str());
    let mut expected = vec!["format_version", "command", "profile"];
    expected.extend(context);
    expected.push("rows");
    assert_eq!(keys(&payload), expected, "{arguments:?}");
    assert_eq!(keys(&payload["rows"][0]), row, "{arguments:?}");
    assert_eq!(tsv_header(qdu, arguments), row, "{arguments:?}");
}

fn table_titles(qdu: &Qdu, arguments: &[&str], first_title: &str) -> Vec<String> {
    let mut arguments = arguments.to_vec();
    arguments.extend(["--style", "plain"]);
    let run = qdu.run(&arguments);
    assert!(matches!(run.status, 0 | 10), "{}", run.stderr);
    let header = run
        .stdout
        .lines()
        .find(|line| line.trim_start().starts_with(first_title))
        .unwrap_or_else(|| panic!("no table in\n{}", run.stdout));
    header.split_whitespace().map(str::to_string).collect()
}

#[test]
fn show_layout() {
    let qdu = sample();
    assert_layout(
        &qdu,
        &["show"],
        &[
            "snapshot",
            "scope",
            "metric",
            "scope_value",
            "staleness",
            "capacity",
            "filesystem",
        ],
        &[
            "rank",
            "allocated_bytes",
            "apparent_bytes",
            "inode_count",
            "share_percent",
            "file_count",
            "directory_count",
            "path",
        ],
    );
    assert_eq!(
        tsv_header(&qdu, &["show", "--stale"]),
        [
            "rank",
            "allocated_bytes",
            "apparent_bytes",
            "inode_count",
            "share_percent",
            "file_count",
            "directory_count",
            "latest_modified_epoch",
            "latest_modified_at",
            "stale_age_days",
            "stale_level",
            "path",
        ]
    );
    assert_eq!(
        tsv_header(&qdu, &["show", "--owners"])[7..],
        ["owner_uid", "owner", "owner_share_percent", "path"]
    );
    assert_eq!(
        table_titles(&qdu, &["show"], "Rank"),
        ["Rank", "Size", "Share", "Distribution", "Files", "Path"]
    );
    assert_eq!(
        table_titles(&qdu, &["show", "--owners"], "Rank"),
        [
            "Rank",
            "Size",
            "Share",
            "Distribution",
            "Files",
            "Owner",
            "Path"
        ]
    );
    assert_eq!(
        table_titles(&qdu, &["show", "--metric", "inodes", "--stale"], "Rank"),
        [
            "Rank",
            "Inodes",
            "Share",
            "Distribution",
            "Files",
            "Stale",
            "Path"
        ]
    );
}

#[test]
fn diff_layout() {
    let qdu = sample();
    assert_layout(
        &qdu,
        &["diff"],
        &["previous", "current", "scope", "metric", "order"],
        &[
            "rank",
            "change",
            "change_percent",
            "current",
            "previous",
            "path",
        ],
    );
    assert_eq!(
        table_titles(&qdu, &["diff"], "Rank"),
        [
            "Rank", "Change", "Change", "%", "Current", "Previous", "Path"
        ]
    );
}

#[test]
fn files_layout() {
    let qdu = sample();
    let row = [
        "rank",
        "allocated_bytes",
        "apparent_bytes",
        "uid",
        "user",
        "modified_epoch",
        "path",
    ];
    let context = [
        "source",
        "snapshot",
        "root",
        "scope",
        "stored_candidate_limit",
        "staleness",
    ];
    assert_layout(&qdu, &["files"], &context, &row);
    assert_layout(&qdu, &["files", "--live"], &context, &row);
    assert_eq!(
        table_titles(&qdu, &["files"], "Rank"),
        ["Rank", "Allocated", "Apparent", "Owner", "Modified", "Path"]
    );
}

#[test]
fn users_layout() {
    let qdu = sample();
    assert_layout(
        &qdu,
        &["users"],
        &["snapshot", "scope", "metric"],
        &[
            "type",
            "rank",
            "uid",
            "user",
            "path",
            "depth",
            "allocated_bytes",
            "apparent_bytes",
            "inode_count",
            "file_count",
            "directory_count",
            "share_percent",
        ],
    );
    assert_eq!(
        table_titles(&qdu, &["users"], "Rank"),
        ["Rank", "User", "UID", "Size", "Share", "Inodes"]
    );
}

#[test]
fn list_and_errors_layout() {
    let qdu = sample();
    assert_layout(
        &qdu,
        &["list"],
        &["latest_complete", "latest_any", "stored_bytes_total"],
        &[
            "created_at",
            "created_epoch",
            "status",
            "error_count",
            "denied_count",
            "allocated_bytes",
            "apparent_bytes",
            "file_count",
            "directory_count",
            "inode_count",
            "archived",
            "stored_bytes",
            "labels",
            "collect_users",
            "large_file_limit",
            "exclude_hash",
            "root",
            "host",
            "filename",
            "sha256",
            "snapshot_id",
        ],
    );
    assert_eq!(
        table_titles(&qdu, &["list"], "Created"),
        [
            "Created", "Status", "Size", "Files", "Archive", "Stored", "Labels", "Snapshot"
        ]
    );
    assert_eq!(
        tsv_header(&qdu, &["errors"]),
        ["path", "operation", "kind", "message"]
    );
    let errors = json(&qdu, &["errors"]);
    assert_eq!(
        keys(&errors),
        [
            "format_version",
            "command",
            "profile",
            "snapshot",
            "error_count",
            "denied_count",
            "rows"
        ]
    );
}

#[test]
fn check_verify_and_doctor_layout() {
    let qdu = sample();
    assert_layout(
        &qdu,
        &["check", "--disk-usage-over", "100"],
        &["snapshot", "passed"],
        &[
            "name",
            "passed",
            "observed",
            "threshold",
            "unit",
            "observed_text",
            "threshold_text",
            "target",
        ],
    );
    assert_layout(
        &qdu,
        &["verify"],
        &["verified"],
        &["snapshot_id", "status", "detail"],
    );
    assert_layout(&qdu, &["doctor"], &[], &["name", "status", "detail"]);
}

#[test]
fn profile_layout() {
    let qdu = sample();
    assert_layout(
        &qdu,
        &["profile", "list"],
        &[],
        &[
            "profile",
            "snapshots",
            "latest_snapshot_id",
            "latest_created_at",
            "latest_complete",
            "capacity_limit_bytes",
            "capacity_user",
            "root",
        ],
    );
    let shown = json(&qdu, &["profile", "show"]);
    assert_eq!(
        keys(&shown),
        ["format_version", "command", "profile", "settings"]
    );
    assert_eq!(shown["command"], "profile show");
    assert_eq!(tsv_header(&qdu, &["profile", "show"]), ["setting", "value"]);
}

#[test]
fn snapshot_layout() {
    let qdu = Qdu::new();
    qdu.write_file("data", 1024);
    let root = qdu.root.to_str().unwrap();
    let payload = json(&qdu, &["snapshot", "--path", root]);
    assert_eq!(
        keys(&payload),
        [
            "format_version",
            "command",
            "profile",
            "snapshot",
            "scan",
            "stored_bytes",
            "retention",
            "saved_settings"
        ]
    );
}
