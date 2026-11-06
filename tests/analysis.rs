//! Capacity limits, owners, files, inodes, staleness, and browse.

mod support;

use std::collections::HashMap;
use std::fs;
use std::time::{Duration, SystemTime};

use serde_json::Value;
use support::{Qdu, paths};

fn uid() -> String {
    // SAFETY: getuid has no preconditions.
    unsafe { libc::getuid() }.to_string()
}

fn set_mtime(path: &std::path::Path, days_ago: u64) {
    let time = SystemTime::now() - Duration::from_secs(days_ago * 86400);
    fs::File::options()
        .write(true)
        .open(path)
        .or_else(|_| fs::File::open(path))
        .unwrap()
        .set_modified(time)
        .unwrap();
}

fn by_path(items: &Value) -> HashMap<String, Value> {
    items
        .as_array()
        .unwrap()
        .iter()
        .map(|item| (item["path"].as_str().unwrap().to_string(), item.clone()))
        .collect()
}

/// Warnings are colored when standard error is a terminal, whatever
/// standard output is.
#[test]
fn warnings_are_colored_by_their_own_stream() {
    let qdu = Qdu::new();
    qdu.write_file("data.bin", 8192);
    qdu.snapshot(&[]).ok();
    let arguments = ["show", "--capacity-limit", "1B"];
    let Some((_, warning, _)) =
        support::run_on_terminal(qdu.command(&arguments), support::Terminal::Stderr)
    else {
        return;
    };
    assert!(warning.contains("\u{1b}[33mqdu: warning:"), "{warning:?}");
    if let Some((_, _, piped)) =
        support::run_on_terminal(qdu.command(&arguments), support::Terminal::Stdout)
    {
        assert!(piped.contains("qdu: warning:"), "{piped:?}");
        assert!(!piped.contains('\u{1b}'), "{piped:?}");
    }
}

#[test]
fn capacity_limit_is_reported_and_warns_when_exceeded() {
    let qdu = Qdu::new();
    qdu.write_file("data.bin", 8192);
    qdu.snapshot(&[]).ok();
    let run = qdu
        .run(&["show", "--capacity-limit", "1B", "--format", "json"])
        .ok();
    let payload = run.json();
    assert_eq!(payload["capacity"]["exceeded"], true);
    assert!(payload["capacity"]["usage_percent"].as_f64().unwrap() > 100.0);
    assert!(
        run.stderr.contains("capacity limit exceeded"),
        "{}",
        run.stderr
    );
    let tsv = qdu
        .run(&["show", "--capacity-limit", "1B", "--format", "tsv"])
        .ok();
    // Rows in TSV describe directories; the limit is in the JSON context.
    let header = tsv.stdout.lines().next().unwrap();
    assert!(header.starts_with("rank\t"), "{header}");
    assert!(!header.contains("capacity"), "{header}");
}

#[test]
fn profile_capacity_user_requires_user_collection() {
    let qdu = Qdu::new();
    let root = qdu.root.to_str().unwrap();
    let run = qdu
        .run(&[
            "profile",
            "set",
            "invalid-user-limit",
            "--path",
            root,
            "--capacity-limit",
            "1GiB",
            "--capacity-user",
            &uid(),
        ])
        .status(2);
    assert!(run.stderr.contains("collect_users"), "{}", run.stderr);
}

#[test]
fn profile_capacity_limit_is_persistent_and_checkable() {
    let qdu = Qdu::new();
    qdu.write_file("data.bin", 8192);
    let root = qdu.root.to_str().unwrap();
    qdu.run(&[
        "profile",
        "set",
        "limited",
        "--path",
        root,
        "--capacity-limit",
        "1B",
    ])
    .ok();
    qdu.run(&["snapshot", "--profile", "limited", "--quiet"])
        .ok();
    assert_eq!(
        qdu.json(&["show", "--profile", "limited"])["capacity"]["exceeded"],
        true
    );
    let check = qdu
        .run(&["check", "--profile", "limited", "--format", "json"])
        .status(10)
        .json();
    assert_eq!(check["passed"], false);
    assert_eq!(check["rows"][0]["name"], "operational_capacity");
}

#[test]
fn capacity_user_uses_owner_statistics() {
    let qdu = Qdu::new();
    qdu.write_file("owned/data.bin", 8192);
    qdu.snapshot(&["--with-users"]).ok();
    let payload = qdu.json(&["show", "--capacity-limit", "1B", "--capacity-user", &uid()]);
    let target = payload["capacity"]["target"].as_str().unwrap();
    assert!(target.contains(&format!("uid {}", uid())), "{target}");
    assert_eq!(payload["capacity"]["exceeded"], true);
}

#[test]
fn no_capacity_limit_suppresses_a_saved_user_limit() {
    let qdu = Qdu::new();
    qdu.write_file("data.bin", 8192);
    let root = qdu.root.to_str().unwrap();
    qdu.run(&[
        "profile",
        "set",
        "limited-user",
        "--path",
        root,
        "--with-users",
        "--capacity-limit",
        "1B",
        "--capacity-user",
        &uid(),
    ])
    .ok();
    qdu.run(&["snapshot", "--profile", "limited-user", "--quiet"])
        .ok();
    let payload = qdu.json(&["show", "--profile", "limited-user", "--no-capacity-limit"]);
    assert!(payload["capacity"].is_null());
}

#[test]
fn capacity_user_requires_owner_statistics() {
    let qdu = Qdu::new();
    qdu.write_file("data.bin", 1024);
    qdu.snapshot(&[]).ok();
    let user = uid();
    let options = ["--capacity-limit", "1GiB", "--capacity-user", user.as_str()];
    // 'show' still ranks directories; 'check' needs a definite answer.
    let run = qdu
        .run(&[&["show"][..], &options, &["--format", "json"]].concat())
        .ok();
    assert!(run.json()["capacity"].is_null());
    assert!(
        run.stderr.contains("capacity limit not evaluated"),
        "{}",
        run.stderr
    );
    assert!(run.stderr.contains("--with-users"));
    let check = qdu.run(&[&["check"][..], &options].concat()).status(2);
    assert!(check.stderr.contains("--with-users"), "{}", check.stderr);
}

#[test]
fn users_inodes_and_owner_directories() {
    let qdu = Qdu::new();
    qdu.write_file("alice/data.bin", 8192);
    qdu.snapshot(&["--with-users"]).ok();
    let payload = qdu.json(&["users"]);
    let users = support::rows_of_type(&payload, "user");
    assert!(!users.is_empty());
    let directories = support::rows_of_type(&payload, "directory");
    assert!(directories.iter().any(|row| row["path"] == "alice"));
    assert_eq!(
        qdu.json(&["users", "--metric", "inodes"])["metric"],
        "inode_count"
    );
}

#[test]
fn users_requires_owner_statistics_in_every_format() {
    let qdu = Qdu::new();
    qdu.write_file("alice/data.bin", 8192);
    qdu.snapshot(&[]).ok();
    for format in ["table", "tsv", "json"] {
        let run = qdu.run(&["users", "--format", format]).status(2);
        assert!(run.stderr.contains("--with-users"), "{}", run.stderr);
    }
    qdu.snapshot(&["--with-users"]).ok();
    let payload = qdu.json(&["users"]);
    let users = support::rows_of_type(&payload, "user");
    assert_eq!(users[0]["uid"].to_string(), uid());
    assert_eq!(users[0]["rank"], 1);
}

#[test]
fn large_files_from_the_snapshot_and_a_live_scan() {
    let qdu = Qdu::new();
    qdu.write_file("one.bin", 4096);
    qdu.write_file("two.bin", 8192);
    qdu.snapshot(&["--file-top", "10"]).ok();
    assert_eq!(qdu.json(&["files"])["rows"][0]["path"], "two.bin");
    assert_eq!(qdu.json(&["files", "--live"])["rows"][0]["path"], "two.bin");
}

#[test]
fn live_and_snapshot_files_count_sparse_allocation_alike() {
    let qdu = Qdu::new();
    qdu.write_file("dense.bin", 8192);
    fs::File::create(qdu.root.join("sparse.bin"))
        .unwrap()
        .set_len(1 << 20)
        .unwrap();
    qdu.snapshot(&["--file-top", "10"]).ok();
    let sizes = |arguments: &[&str]| -> HashMap<String, (i64, i64)> {
        by_path(&qdu.json(arguments)["rows"])
            .into_iter()
            .map(|(path, item)| {
                (
                    path,
                    (
                        item["allocated_bytes"].as_i64().unwrap(),
                        item["apparent_bytes"].as_i64().unwrap(),
                    ),
                )
            })
            .collect()
    };
    let stored = sizes(&["files"]);
    assert_eq!(stored["sparse.bin"].1, 1 << 20);
    assert_eq!(sizes(&["files", "--live"]), stored);
}

#[test]
fn ties_are_ordered_by_path_in_snapshots_and_live_scans() {
    let qdu = Qdu::new();
    for name in ["c.bin", "a.bin", "b.bin", "d.bin"] {
        qdu.write_file(name, 8192);
    }
    qdu.snapshot(&["--file-top", "3"]).ok();
    let expected = ["a.bin", "b.bin", "c.bin"];
    assert_eq!(paths(&qdu.json(&["files"])["rows"]), expected);
    assert_eq!(
        paths(&qdu.json(&["files", "--live", "-n", "3"])["rows"]),
        expected
    );
}

#[test]
fn files_and_inodes_describe_their_snapshot() {
    let qdu = Qdu::new();
    qdu.write_file("a/data", 1024);
    qdu.snapshot(&["--file-top", "5"]).ok();
    let files = qdu.run(&["files", "--style", "plain"]).ok().stdout;
    let snapshot_line = regex::Regex::new(r"\nSnapshot +\d{4}-\d\d-\d\d \d\d:\d\d \(").unwrap();
    assert!(snapshot_line.is_match(&files), "{files}");
    assert!(
        files.contains("Candidates  the 5 largest files at snapshot time"),
        "{files}"
    );
    let live = qdu
        .run(&["files", "--live", "--style", "plain"])
        .ok()
        .stdout;
    assert!(live.contains("Source  live scan at "), "{live}");
    let inodes = qdu
        .run(&["show", "--metric", "inodes", "--style", "plain"])
        .ok()
        .stdout;
    assert!(
        inodes.contains(&format!("Root      {}\n", qdu.root.display())),
        "{inodes}"
    );
    assert!(
        regex::Regex::new(r"\nScope +\. +3 inodes\n")
            .unwrap()
            .is_match(&inodes),
        "{inodes}"
    );
    assert!(
        regex::Regex::new(r"\nInodes +[0-9.]+% used, [0-9,]+ available of ")
            .unwrap()
            .is_match(&inodes),
        "{inodes}"
    );
}

#[test]
fn doctor_scopes_and_inodes_produce_json() {
    let qdu = Qdu::new();
    qdu.write_file("a/b/data", 1024);
    qdu.snapshot(&[]).ok();
    let doctor = qdu.run(&["doctor", "--format", "json"]);
    assert!(matches!(doctor.status, 0 | 1));
    assert!(doctor.json().get("rows").is_some());
    assert_eq!(qdu.json(&["show", "a"])["scope"], "a");
    let inodes = qdu.json(&["show", "--metric", "inodes"]);
    assert_eq!(inodes["metric"], "inode_count");
    assert_eq!(inodes["rows"][0]["path"], "a");
    assert_eq!(inodes["rows"][0]["inode_count"], 3);
}

#[test]
fn diff_rejects_opposite_change_filters() {
    let qdu = Qdu::new();
    qdu.run(&["diff", "a", "--growth-only", "--shrink-only"])
        .status(2);
}

#[test]
fn stale_directory_annotations_colors_and_filtering() {
    let qdu = Qdu::new();
    for (name, days) in [
        ("fresh", 10),
        ("old30", 31),
        ("old90", 91),
        ("old180", 181),
        ("old365", 366),
    ] {
        let file = qdu.write_file(&format!("{name}/data.bin"), 1024);
        set_mtime(&file, days);
        set_mtime(file.parent().unwrap(), days);
    }
    let recent = qdu.write_file("parent-with-recent-child/nested/recent.bin", 1024);
    set_mtime(&recent, 5);
    set_mtime(&qdu.root.join("parent-with-recent-child/nested"), 400);
    set_mtime(&qdu.root.join("parent-with-recent-child"), 400);
    qdu.snapshot(&["--file-top", "20"]).ok();
    let entries = by_path(&qdu.json(&["show", "--stale", "-L", "2", "-n", "100"])["rows"]);
    for (path, level) in [
        ("fresh", "recent"),
        ("old30", "notice"),
        ("old90", "warning"),
        ("old180", "elevated"),
        ("old365", "critical"),
        ("parent-with-recent-child", "recent"),
    ] {
        assert_eq!(entries[path]["stale_level"], level, "{path}");
    }
    let filtered =
        paths(&qdu.json(&["show", "--stale-only", "180", "-L", "2", "-n", "100"])["rows"]);
    assert!(filtered.contains(&"old180".to_string()));
    assert!(filtered.contains(&"old365".to_string()));
    assert!(!filtered.contains(&"old90".to_string()));
    assert!(!filtered.contains(&"parent-with-recent-child".to_string()));
    let colored = qdu
        .run(&[
            "show", "--stale", "--color", "always", "--style", "plain", "-L", "1", "-n", "100",
        ])
        .ok()
        .stdout;
    for code in ["\x1b[34m", "\x1b[33m", "\x1b[38;5;208m", "\x1b[31m"] {
        assert!(colored.contains(code), "{code:?} missing");
    }
}

#[test]
fn stale_file_annotations_and_filtering() {
    let qdu = Qdu::new();
    let recent = qdu.write_file("recent.bin", 2048);
    let old = qdu.write_file("old.bin", 4096);
    set_mtime(&recent, 10);
    set_mtime(&old, 120);
    qdu.snapshot(&["--file-top", "10"]).ok();
    let files = by_path(&qdu.json(&["files", "--stale"])["rows"]);
    assert_eq!(files["recent.bin"]["stale_level"], "recent");
    assert_eq!(files["old.bin"]["stale_level"], "warning");
    assert_eq!(
        paths(&qdu.json(&["files", "--stale-only", "90"])["rows"]),
        ["old.bin"]
    );
}

#[test]
fn owner_reports_respect_their_limits() {
    let qdu = Qdu::new();
    qdu.write_file("a/data", 4096);
    qdu.write_file("b/data", 8192);
    qdu.snapshot(&["--with-users"]).ok();
    let none = qdu.json(&["users", "--dirs", "0"]);
    assert!(support::rows_of_type(&none, "directory").is_empty());
    let shown = qdu.json(&["users", "-n", "1", "--dirs", "1"]);
    assert_eq!(support::rows_of_type(&shown, "user").len(), 1);
    let directories = support::rows_of_type(&shown, "directory");
    assert_eq!(directories.len(), 1);
    assert_eq!(directories[0]["path"], "b");
}

/// `users PATH` ranks the owners of one directory from the statistics that
/// reach its depth.
#[test]
fn users_rank_the_owners_of_a_directory() {
    let qdu = Qdu::new();
    qdu.write_file("a/x/data", 4096);
    qdu.write_file("a/y/data", 8192);
    qdu.write_file("b/data", 65536);
    qdu.snapshot(&["--with-users", "--user-max-depth", "2"])
        .ok();
    let payload = qdu.json(&["users", "a"]);
    assert_eq!(payload["scope"], "a");
    let users = support::rows_of_type(&payload, "user");
    assert_eq!(users.len(), 1);
    let whole = qdu.json(&["show", "a"])["scope_value"].clone();
    assert_eq!(users[0]["allocated_bytes"], whole);
    assert_eq!(users[0]["share_percent"], 100.0);
    let directories: Vec<&str> = support::rows_of_type(&payload, "directory")
        .iter()
        .map(|row| row["path"].as_str().unwrap())
        .collect();
    assert_eq!(directories, ["a/y", "a/x"]);
    // An absolute path inside the root names the same directory.
    let absolute = qdu.root.join("a");
    let same = qdu.json(&["users", absolute.to_str().unwrap()]);
    assert_eq!(same["rows"], payload["rows"]);
    for (path, message) in [
        ("a/x/deeper", "reach depth 2"),
        ("missing", "not present in snapshot"),
    ] {
        let run = qdu.run(&["users", path]).status(2);
        assert!(run.stderr.contains(message), "{path}: {}", run.stderr);
    }
}

/// `show --owners` names the largest owner of each directory as deep as the
/// owner statistics go.
#[test]
fn show_names_the_largest_owner_of_each_directory() {
    let qdu = Qdu::new();
    qdu.write_file("a/x/deep/data", 4096);
    qdu.write_file("b/data", 8192);
    qdu.snapshot(&[]).ok();
    let run = qdu.run(&["show", "--owners"]).status(2);
    assert!(run.stderr.contains("--with-users"), "{}", run.stderr);
    qdu.snapshot(&["--with-users", "--user-max-depth", "1"])
        .ok();
    let rows = qdu.json(&["show", "--owners", "-L", "2"])["rows"].clone();
    let rows = rows.as_array().unwrap();
    let by_path = |path: &str| rows.iter().find(|row| row["path"] == path).unwrap();
    for path in ["a", "b"] {
        let row = by_path(path);
        assert_eq!(row["owner_uid"].to_string(), uid(), "{row}");
        assert_eq!(row["owner_share_percent"], 100.0);
        assert!(row["owner"].is_string());
    }
    // a/x lies below user_max_depth 1, so its owner is not known.
    assert!(by_path("a/x")["owner_uid"].is_null());
}

#[test]
fn live_scan_options_require_live() {
    let qdu = Qdu::new();
    for option in [&["--path", "/tmp"][..], &["--threads", "2"][..]] {
        let run = qdu.run(&[&["files"][..], option].concat()).status(2);
        assert!(run.stderr.contains("--live"), "{}", run.stderr);
    }
}

#[test]
fn files_filter_by_owner() {
    let qdu = Qdu::new();
    qdu.write_file("one.bin", 4096);
    qdu.snapshot(&["--file-top", "10"]).ok();
    for arguments in [&["files"][..], &["files", "--live"][..]] {
        let user = uid();
        let mine = qdu.json(&[arguments, &["--user", user.as_str()]].concat());
        assert_eq!(paths(&mine["rows"]), ["one.bin"]);
    }
    let run = qdu.run(&["files", "--user", "no-such-user-qdu"]).status(2);
    assert!(run.stderr.contains("unknown user"), "{}", run.stderr);
}

#[test]
fn stale_thresholds_set_the_levels() {
    let qdu = Qdu::new();
    let file = qdu.write_file("old/data.bin", 1024);
    set_mtime(&file, 15);
    set_mtime(file.parent().unwrap(), 15);
    qdu.snapshot(&[]).ok();
    let level = |thresholds: &[&str]| {
        let arguments = [&["show", "--stale"][..], thresholds].concat();
        by_path(&qdu.json(&arguments)["rows"])["old"]["stale_level"].clone()
    };
    assert_eq!(level(&[]), "recent");
    assert_eq!(level(&["--stale-thresholds", "10,20,30,40"]), "notice");
    assert_eq!(level(&["--stale-thresholds", "1,2,3,14"]), "critical");
    qdu.run(&["show", "--stale-thresholds", "30,20,10,5"])
        .status(2);
}

#[test]
fn ages_are_days_or_durations() {
    let qdu = Qdu::new();
    qdu.write_file("data", 1024);
    qdu.snapshot(&[]).ok();
    let days = qdu.json(&["show", "--stale-only", "84"]);
    let weeks = qdu.json(&["show", "--stale-only", "12w"]);
    assert_eq!(days["rows"], weeks["rows"]);
    // A huge age keeps nothing rather than wrapping around.
    let huge = qdu.json(&["show", "--stale-only", "18446744073709551615"]);
    assert_eq!(huge["rows"], serde_json::json!([]));
    let run = qdu.run(&["show", "--stale-only", "36h"]).status(2);
    assert!(run.stderr.contains("whole days"), "{}", run.stderr);
    let run = qdu.run(&["check", "--snapshot-age-over", "soon"]).status(2);
    assert!(run.stderr.contains("such as 90d"), "{}", run.stderr);
}

#[test]
fn snapshot_age_check_reports_old_snapshots() {
    let qdu = Qdu::new();
    qdu.write_file("data", 1024);
    qdu.snapshot(&[]).ok();
    let mut index = qdu.index();
    let created = index["snapshots"][0]["created_epoch"].as_i64().unwrap();
    index["snapshots"][0]["created_epoch"] = serde_json::json!(created - 10 * 86400);
    qdu.write_index(&index);
    qdu.run(&["check", "--snapshot-age-over", "8d"]).status(10);
    qdu.run(&["check", "--snapshot-age-over", "30d"]).ok();
}

#[test]
fn capacity_profile_ignores_a_configured_user() {
    let qdu = Qdu::new();
    qdu.write_file("data.bin", 8192);
    let root = qdu.root.to_str().unwrap();
    qdu.run(&[
        "profile",
        "set",
        "limited",
        "--path",
        root,
        "--with-users",
        "--capacity-limit",
        "1GiB",
        "--capacity-user",
        &uid(),
    ])
    .ok();
    qdu.run(&["snapshot", "--profile", "limited", "--quiet"])
        .ok();
    let user = qdu.json(&["show", "--profile", "limited"])["capacity"]["target"].clone();
    assert!(user.as_str().unwrap().starts_with("user "), "{user}");
    let whole = qdu.json(&["show", "--profile", "limited", "--capacity-profile"]);
    assert_eq!(whole["capacity"]["target"], "profile root");
}

#[test]
fn an_empty_result_is_explicit_and_json_is_empty() {
    let qdu = Qdu::new();
    qdu.write_file("data", 1024);
    qdu.snapshot(&[]).ok();
    let table = qdu
        .run(&["show", "--min-size", "1TiB", "--style", "plain"])
        .ok();
    assert!(table.stdout.contains("No matching entries."));
    assert_eq!(
        qdu.json(&["show", "--min-size", "1TiB"])["rows"],
        serde_json::json!([])
    );
}

#[test]
fn browse_streams_paths_to_fzf() {
    let mut qdu = Qdu::new();
    qdu.write_file("a/child/file", 1024);
    qdu.write_file(" spaced\nname/file", 4096);
    qdu.snapshot(&[]).ok();
    let received = qdu.base.join("fzf-input");
    qdu.script("fzf", "cat >\"$FZF_INPUT\"\nprintf '%s\\0' \"$FZF_PICK\"\n");
    let path = qdu.path_with_scripts();
    qdu.env("PATH", path);
    qdu.env("FZF_INPUT", received.to_str().unwrap());
    for pick in ["a", " spaced\nname", "a\twith\ttabs"] {
        if pick.contains('\t') {
            qdu.write_file(&format!("{pick}/file"), 16384);
            qdu.snapshot(&[]).ok();
        }
        let mut run = qdu.command(&["browse", "--format", "json"]);
        run.env("FZF_PICK", format!("  4.0KiB\t{pick}"));
        let output = run.output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let payload: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(payload["scope"], pick);
    }
    let text = fs::read_to_string(&received).unwrap();
    let records: Vec<&str> = text
        .split('\0')
        .filter(|record| !record.is_empty())
        .collect();
    let (size, root) = records[0].split_once('\t').unwrap();
    assert_eq!(root, ".");
    assert_eq!(size.len(), 8);
    assert!(size.ends_with('B'));
    let browsed: Vec<&str> = records
        .iter()
        .map(|record| record.split_once('\t').unwrap().1)
        .collect();
    assert_eq!(browsed[1], "a\twith\ttabs");
    assert!(browsed.contains(&" spaced\nname"));
}

#[test]
fn browse_reports_a_failing_fzf_but_not_a_cancelled_one() {
    let mut qdu = Qdu::new();
    qdu.write_file("data", 1024);
    qdu.snapshot(&[]).ok();
    qdu.script("fzf", "cat >/dev/null\nexit \"$FZF_STATUS\"\n");
    let path = qdu.path_with_scripts();
    qdu.env("PATH", path);
    for (status, expected) in [("2", 2), ("130", 0), ("1", 0)] {
        let output = qdu
            .command(&["browse"])
            .env("FZF_STATUS", status)
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(expected), "fzf exit {status}");
    }
}

#[test]
fn browse_explains_that_fzf_is_missing() {
    let mut qdu = Qdu::new();
    qdu.write_file("data", 1024);
    qdu.snapshot(&[]).ok();
    let empty = qdu.base.join("empty-path");
    fs::create_dir(&empty).unwrap();
    qdu.env("PATH", empty.to_str().unwrap());
    let run = qdu.run(&["browse"]).status(2);
    assert!(
        run.stderr.contains("fzf is not installed"),
        "{}",
        run.stderr
    );
}

/// A snapshot read on another host, as a dashboard that gathers several
/// servers reads it, reports the capacity it recorded instead of that of
/// whatever this host has at the same path.
#[test]
fn snapshots_from_another_host_keep_their_recorded_capacity() {
    let qdu = Qdu::new();
    qdu.write_file("a", 4096);
    qdu.snapshot(&[]).ok();
    qdu.write_file("b", 65536);
    qdu.snapshot(&[]).ok();
    let week = 7 * 86_400;
    let mut index = qdu.index();
    let newest = index["snapshots"][1]["created_epoch"].as_i64().unwrap();
    index["snapshots"][0]["created_epoch"] = (newest - week).into();
    for position in 0..2 {
        index["snapshots"][position]["host"] = "elsewhere.example".into();
    }
    qdu.write_index(&index);
    let size = |position: usize| {
        index["snapshots"][position]["allocated_bytes"]
            .as_i64()
            .unwrap()
    };
    let growth = size(1) - size(0);
    assert!(growth > 0);
    let connection = rusqlite::Connection::open(qdu.latest_database()).unwrap();
    for (key, value) in [
        ("host", "elsewhere.example".to_string()),
        ("filesystem_total_bytes", (growth * 8).to_string()),
        ("filesystem_available_bytes", (growth * 2).to_string()),
        ("filesystem_free_bytes", (growth * 2).to_string()),
    ] {
        connection
            .execute(
                "UPDATE metadata SET value = ?2 WHERE key = ?1",
                rusqlite::params![key, value],
            )
            .unwrap();
    }
    drop(connection);
    let shown = qdu.json(&["show"]);
    assert!(shown["filesystem"]["current"].is_null(), "{shown}");
    assert_eq!(
        shown["filesystem"]["snapshot"]["available_bytes"],
        growth * 2
    );
    // Two weeks of growth fill the free space that the snapshot recorded.
    let history = qdu.json(&["history"]);
    let full = history["forecast"]["filesystem_full_epoch"]
        .as_i64()
        .unwrap();
    assert!((full - (newest + 2 * week)).abs() <= 1, "{history}");
    // 6 of 8 parts used: 75%.
    let run = qdu
        .run(&["check", "--disk-usage-over", "70", "--format", "json"])
        .status(10);
    assert_eq!(run.json()["rows"][0]["observed"], 75.0);
    qdu.run(&["check", "--disk-usage-over", "80"]).ok();
}
