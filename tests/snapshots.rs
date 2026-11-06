//! Snapshot creation, ranking, comparison, and listing.

mod support;

use std::ffi::OsStr;
use std::fs;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::PermissionsExt;

use support::{Qdu, paths, unreadable_directory};

#[test]
fn rejects_snapshot_filenames_outside_the_snapshot_directory() {
    let qdu = Qdu::new();
    qdu.write_file("data", 1024);
    qdu.snapshot(&[]).ok();
    let mut index = qdu.index();
    index["snapshots"][0]["filename"] = "../outside.sqlite3".into();
    qdu.write_index(&index);
    let run = qdu.run(&["list"]).status(4);
    assert!(
        run.stderr.contains("invalid profile index"),
        "{}",
        run.stderr
    );
}

#[test]
fn snapshot_shows_no_progress_without_a_terminal() {
    let qdu = Qdu::new();
    qdu.write_file("data", 1024);
    let root = qdu.root.to_str().unwrap();
    let run = qdu.run(&["snapshot", "--path", root]).ok();
    assert!(run.stdout.contains("Snapshot saved"), "{}", run.stdout);
    assert!(!run.stderr.contains('\r'));
    assert!(!run.stderr.contains("Scanning"));
}

#[test]
fn snapshot_show_and_list_produce_json() {
    let qdu = Qdu::new();
    qdu.write_file("a/large.bin", 8192);
    qdu.write_file("b/small.bin", 1024);
    qdu.snapshot(&["--file-top", "10"]).ok();
    let payload = qdu.json(&["show", "-L", "2"]);
    assert_eq!(payload["scope"], ".");
    assert_eq!(payload["rows"][0]["path"], "a");
    let listing = qdu.json(&["list"]);
    assert_eq!(listing["rows"].as_array().unwrap().len(), 1);
}

#[test]
fn list_reports_the_disk_space_of_each_snapshot() {
    let qdu = Qdu::new();
    qdu.write_file("data", 1024);
    qdu.snapshot(&[]).ok();
    qdu.snapshot(&[]).ok();
    let mut expected = std::collections::BTreeMap::new();
    for record in qdu.records() {
        let file = qdu
            .snapshots_dir()
            .join(record["filename"].as_str().unwrap());
        expected.insert(
            record["snapshot_id"].as_str().unwrap().to_string(),
            fs::metadata(file).unwrap().len(),
        );
    }
    let payload = qdu.json(&["list"]);
    let reported: std::collections::BTreeMap<String, u64> = payload["rows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| {
            (
                item["snapshot_id"].as_str().unwrap().to_string(),
                item["stored_bytes"].as_u64().unwrap(),
            )
        })
        .collect();
    assert_eq!(reported, expected);
    assert_eq!(
        payload["stored_bytes_total"].as_u64().unwrap(),
        expected.values().sum::<u64>()
    );
    let tsv = qdu.run(&["list", "--format", "tsv"]).ok();
    let header: Vec<&str> = tsv.stdout.lines().next().unwrap().split('\t').collect();
    assert!(header.contains(&"stored_bytes"), "{header:?}");
    assert_eq!(header.last(), Some(&"snapshot_id"));
    let table = qdu.run(&["list", "--style", "plain"]).ok();
    assert!(table.stdout.contains("Stored"));
    assert!(table.stdout.contains("2 snapshot(s) use"));
}

#[test]
fn scope_share_uses_the_under_directory() {
    let qdu = Qdu::new();
    qdu.write_file("scope/a.bin", 4096);
    qdu.write_file("scope/child/b.bin", 4096);
    qdu.write_file("outside/c.bin", 65536);
    qdu.snapshot(&[]).ok();
    let payload = qdu.json(&["show", "scope", "-L", "1"]);
    let child = payload["rows"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["path"] == "child")
        .unwrap()
        .clone();
    let share = child["share_percent"].as_f64().unwrap();
    assert!(share > 0.0 && share <= 100.0, "{share}");
}

#[test]
fn path_scope_is_case_sensitive_and_literal() {
    let qdu = Qdu::new();
    qdu.write_file("data/lower/file", 8192);
    qdu.write_file("data_x/under/file", 16384);
    // macOS volumes are usually case-insensitive; there "Data" is "data".
    let case_sensitive = !qdu.root.join("DATA").exists();
    if case_sensitive {
        qdu.write_file("Data/upper/file", 4096);
    }
    qdu.snapshot(&["--file-top", "10"]).ok();
    qdu.write_file("data/lower/file", 32768);
    if case_sensitive {
        qdu.write_file("Data/upper/file", 16384);
    }
    qdu.snapshot(&["--file-top", "10"]).ok();
    let entries = |arguments: &[&str]| paths(&qdu.json(arguments)["rows"]);
    assert_eq!(entries(&["show", "data", "-L", "2"]), ["lower"]);
    assert_eq!(entries(&["diff", "data", "-L", "2"]), ["lower"]);
    assert_eq!(
        paths(&qdu.json(&["files", "data"])["rows"]),
        ["data/lower/file"]
    );
    // An absolute path inside the root names the same directory.
    let absolute = qdu.root.join("data");
    let absolute = absolute.to_str().unwrap();
    assert_eq!(entries(&["show", absolute, "-L", "2"]), ["lower"]);
    let outside = qdu.run(&["show", "/"]).status(2);
    assert!(
        outside.stderr.contains("outside the snapshot root"),
        "{}",
        outside.stderr
    );
    if case_sensitive {
        assert_eq!(entries(&["show", "Data", "-L", "2"]), ["upper"]);
        assert_eq!(entries(&["diff", "Data", "-L", "2"]), ["upper"]);
    }
}

#[test]
fn apparent_metric_is_consistent() {
    let qdu = Qdu::new();
    qdu.write_file("a/data.bin", 12345);
    qdu.snapshot(&[]).ok();
    let payload = qdu.json(&["show", "--metric", "apparent_bytes"]);
    assert_eq!(payload["metric"], "apparent_bytes");
    assert_eq!(payload["rows"][0]["path"], "a");
    // The directory's own size counts toward its apparent size.
    assert!(payload["rows"][0]["apparent_bytes"].as_i64().unwrap() >= 12345);
    // The scope's own value includes the directories themselves.
    assert!(payload["scope_value"].as_i64().unwrap() >= 12345);
}

#[test]
fn diff_explains_that_it_needs_two_snapshots() {
    let qdu = Qdu::new();
    qdu.write_file("file", 1024);
    qdu.snapshot(&[]).ok();
    let run = qdu.run(&["diff"]).status(2);
    assert!(
        run.stderr.contains("needs two complete snapshots"),
        "{}",
        run.stderr
    );
    qdu.snapshot(&[]).ok();
    let first = qdu.snapshot_ids()[0].clone();
    let run = qdu.run(&["diff", "--to", &first]).status(2);
    assert!(
        run.stderr
            .contains(&format!("no complete snapshot exists before {first}")),
        "{}",
        run.stderr
    );
}

#[test]
fn diff_orders_growth_shrink_absolute_and_current() {
    let qdu = Qdu::new();
    qdu.write_file("grow/file", 4096);
    qdu.write_file("shrink/file", 16384);
    qdu.snapshot(&[]).ok();
    qdu.write_file("grow/file", 32768);
    qdu.write_file("shrink/file", 1);
    qdu.snapshot(&[]).ok();
    for order in ["growth", "shrink", "absolute", "current"] {
        assert_eq!(qdu.json(&["diff", "--order", order])["order"], order);
    }
    let grown = qdu.json(&["diff", "--growth-only"]);
    assert!(
        grown["rows"]
            .as_array()
            .unwrap()
            .iter()
            .all(|item| item["change"].as_i64().unwrap() > 0)
    );
    let shrunk = qdu.json(&["diff", "--shrink-only"]);
    assert!(
        shrunk["rows"]
            .as_array()
            .unwrap()
            .iter()
            .all(|item| item["change"].as_i64().unwrap() < 0)
    );
    // Without --order, --shrink-only puts the largest shrinkage first.
    assert_eq!(shrunk["order"], "shrink");
}

#[test]
fn diff_compares_an_older_snapshot_with_a_newer_one() {
    let qdu = Qdu::new();
    qdu.write_file("data", 1024);
    qdu.snapshot(&[]).ok();
    qdu.snapshot(&[]).ok();
    let reversed = qdu
        .run(&["diff", "--from", "latest", "--to", "previous"])
        .status(2);
    assert!(reversed.stderr.contains("swap them"), "{}", reversed.stderr);
    let same = qdu.run(&["diff", "--from", "latest"]).status(2);
    assert!(same.stderr.contains("both name"), "{}", same.stderr);
}

#[test]
fn equal_changes_are_ranked_by_path() {
    let qdu = Qdu::new();
    for name in ["c", "a", "b"] {
        qdu.write_file(&format!("{name}/file"), 4096);
    }
    qdu.snapshot(&[]).ok();
    for name in ["c", "a", "b"] {
        qdu.write_file(&format!("{name}/more"), 8192);
    }
    qdu.snapshot(&[]).ok();
    assert_eq!(paths(&qdu.json(&["diff"])["rows"]), ["a", "b", "c"]);
    assert_eq!(paths(&qdu.json(&["diff", "-n", "2"])["rows"]), ["a", "b"]);
}

#[test]
fn diff_compares_the_selected_metric() {
    let qdu = Qdu::new();
    qdu.write_file("many/seed", 1024);
    qdu.write_file("big/data", 4096);
    qdu.snapshot(&[]).ok();
    for index in 0..5 {
        qdu.write_file(&format!("many/new-{index}"), 0);
    }
    qdu.write_file("big/data", 1 << 20);
    qdu.snapshot(&[]).ok();
    let inodes = qdu.json(&["diff", "--metric", "inodes", "--order", "growth"]);
    assert_eq!(inodes["metric"], "inode_count");
    assert_eq!(inodes["rows"][0]["path"], "many");
    assert_eq!(inodes["rows"][0]["change"], 5);
    assert_eq!(inodes["rows"][0]["change_percent"], 250.0);
    let apparent = qdu.json(&["diff", "--metric", "apparent_bytes"]);
    assert_eq!(apparent["metric"], "apparent_bytes");
    let big = apparent["rows"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["path"] == "big")
        .unwrap()
        .clone();
    assert_eq!(big["change"], (1 << 20) - 4096);
    let allocated = qdu.json(&["diff"]);
    assert_eq!(allocated["metric"], "allocated_bytes");
    assert_eq!(allocated["rows"][0]["path"], "big");
    let table = qdu
        .run(&["diff", "--metric", "inodes", "--style", "plain"])
        .ok();
    assert!(table.stdout.contains("+5"));
    assert!(table.stdout.contains("+250.0%"));
    qdu.run(&["diff", "--metric", "files"]).status(2);
}

#[test]
fn snapshot_summary_counts_one_file_in_the_singular() {
    let qdu = Qdu::new();
    qdu.write_file("only", 1024);
    let root = qdu.root.to_str().unwrap();
    let run = qdu
        .run(&["snapshot", "--path", root, "--style", "plain"])
        .ok();
    assert!(
        run.stdout.contains(" in 1 file and 1 directory\n"),
        "{}",
        run.stdout
    );
}

#[test]
fn reports_are_compact_and_diffs_list_changes_only() {
    let qdu = Qdu::new();
    qdu.write_file("grow/file", 1024);
    qdu.write_file("same/file", 1024);
    let root = qdu.root.to_str().unwrap();
    let run = qdu
        .run(&["snapshot", "--path", root, "--style", "plain"])
        .ok();
    assert!(
        run.stdout.contains("2 files and 3 directories"),
        "{}",
        run.stdout
    );
    assert!(!run.stdout.contains("Skipped mounts"));
    qdu.write_file("grow/file", 65536);
    qdu.snapshot(&[]).ok();
    let show = qdu.run(&["show", "--style", "plain"]).ok();
    let header: Vec<&str> = show
        .stdout
        .split("\n\n")
        .next()
        .unwrap()
        .lines()
        .skip(1)
        .map(|line| line.split_whitespace().next().unwrap())
        .collect();
    assert_eq!(header, ["Root", "Snapshot", "Scope", "Disk"]);
    assert_eq!(paths(&qdu.json(&["diff"])["rows"]), ["grow"]);
    let same = qdu.run(&["diff", "same", "--style", "plain"]).ok();
    assert!(
        same.stdout.contains("No changes in this scope."),
        "{}",
        same.stdout
    );
}

#[test]
fn hard_links_are_counted_once() {
    let qdu = Qdu::new();
    let original = qdu.write_file("a/original", 4096);
    fs::hard_link(&original, qdu.root.join("a/link")).unwrap();
    qdu.snapshot(&["--with-users"]).ok();
    let connection = rusqlite::Connection::open(qdu.latest_database()).unwrap();
    let files: i64 = connection
        .query_row(
            "SELECT file_count FROM directories WHERE path='.'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(files, 1);
}

#[test]
fn hard_links_are_charged_to_the_first_path_in_sorted_order() {
    let qdu = Qdu::new();
    let original = qdu.write_file("z/original", 65536);
    fs::hard_link(&original, qdu.root.join("a-link")).unwrap();
    qdu.snapshot(&[]).ok();
    let payload = qdu.json(&["show"]);
    let entries = payload["rows"].as_array().unwrap();
    let z = entries.iter().find(|item| item["path"] == "z").unwrap();
    assert_eq!(z["file_count"], 0);
    assert_eq!(payload["snapshot"]["file_count"], 1);
}

#[test]
fn exclusions_apply_to_directories_and_owners() {
    let qdu = Qdu::new();
    qdu.write_file("keep/data", 1024);
    qdu.write_file("nested/cache/data", 16384);
    qdu.snapshot(&["--exclude", "cache", "--with-users"]).ok();
    let shown = paths(&qdu.json(&["show", "-L", "5"])["rows"]);
    assert!(
        !shown.iter().any(|path| path.contains("cache")),
        "{shown:?}"
    );
    let users = qdu.json(&["users"]);
    for row in support::rows_of_type(&users, "directory") {
        assert!(!row["path"].as_str().unwrap().contains("cache"), "{row}");
    }
}

#[test]
fn exclusion_patterns_anchor_and_select_directories() {
    let qdu = Qdu::new();
    for path in [
        "build/out.o",
        "src/build/keep.o",
        "logs/app.log",
        "other/logs",
        "run/models/weights.bin",
        "run/models/notes.txt",
        "cache/blob",
    ] {
        qdu.write_file(path, 4096);
    }
    qdu.snapshot(&[
        "--file-top",
        "20",
        "--exclude",
        "/build",
        "--exclude",
        "logs/",
        "--exclude",
        "**/models/*.bin",
        "--exclude",
        "cache/**",
    ])
    .ok();
    let mut directories = paths(&qdu.json(&["show", "-L", "5"])["rows"]);
    directories.sort();
    assert_eq!(
        directories,
        ["cache", "other", "run", "run/models", "src", "src/build"]
    );
    let mut files = paths(&qdu.json(&["files"])["rows"]);
    files.sort();
    assert_eq!(
        files,
        ["other/logs", "run/models/notes.txt", "src/build/keep.o"]
    );
}

#[test]
fn tabs_newlines_and_unicode_are_stored_safely() {
    let qdu = Qdu::new();
    qdu.write_file("日本語/line\nbreak/tab\tfile", 1024);
    qdu.snapshot(&[]).ok();
    let shown = paths(&qdu.json(&["show", "-L", "5"])["rows"]);
    assert!(
        shown
            .iter()
            .any(|path| path.contains('\n') || path.contains('\t'))
    );
    let table = qdu.run(&["show", "-L", "5", "--style", "plain"]).ok();
    assert!(!table.stdout.contains("line\nbreak"));
    assert!(table.stdout.contains("line\\nbreak"));
}

#[test]
fn undecodable_file_names_are_stored_with_escapes() {
    let qdu = Qdu::new();
    let mut directory = qdu.root.as_os_str().as_bytes().to_vec();
    directory.extend_from_slice(b"/latin1-\xe9t\xe9");
    let directory = std::path::PathBuf::from(OsStr::from_bytes(&directory));
    if fs::create_dir(&directory).is_err() {
        // APFS, for example, rejects names that are not UTF-8.
        return;
    }
    fs::write(
        directory.join(OsStr::from_bytes(b"f\xff.bin")),
        vec![b'x'; 8192],
    )
    .unwrap();
    qdu.snapshot(&["--file-top", "10"]).ok();
    assert_eq!(paths(&qdu.json(&["show"])["rows"]), ["latin1-\\xe9t\\xe9"]);
    for arguments in [&["files"][..], &["files", "--live"][..]] {
        assert_eq!(
            paths(&qdu.json(arguments)["rows"]),
            ["latin1-\\xe9t\\xe9/f\\xff.bin"]
        );
    }
    let table = qdu.run(&["show", "--style", "plain"]).ok();
    assert!(table.stdout.contains("latin1-\\xe9t\\xe9"));
}

#[test]
fn names_stored_alike_are_merged() {
    let qdu = Qdu::new();
    // "a\xff" escapes an invalid byte and is also a valid name itself.
    let raw = qdu.root.join(OsStr::from_bytes(b"a\xff"));
    if fs::create_dir(&raw).is_err() {
        // APFS, for example, rejects names that are not UTF-8.
        return;
    }
    fs::create_dir(qdu.root.join("a\\xff")).unwrap();
    fs::write(raw.join("one"), vec![b'x'; 4096]).unwrap();
    fs::write(qdu.root.join("a\\xff/two"), vec![b'x'; 8192]).unwrap();
    fs::write(qdu.root.join(OsStr::from_bytes(b"f\xff")), vec![b'x'; 4096]).unwrap();
    fs::write(qdu.root.join("f\\xff"), vec![b'x'; 8192]).unwrap();
    qdu.snapshot(&["--file-top", "10", "--with-users"]).ok();
    let payload = qdu.json(&["show"]);
    let merged = payload["rows"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["path"] == "a\\xff")
        .unwrap()
        .clone();
    assert_eq!(merged["directory_count"], 2);
    assert_eq!(merged["file_count"], 2);
    let files = paths(&qdu.json(&["files"])["rows"]);
    assert_eq!(files.iter().filter(|path| *path == "f\\xff").count(), 1);
    qdu.run(&["verify"]).ok();
}

#[test]
fn many_exclusion_patterns_compile() {
    let qdu = Qdu::new();
    qdu.write_file("proj7/x/build-1ab/main.o", 4096);
    qdu.write_file("keep/main.c", 4096);
    let patterns: String = (0..3000)
        .map(|index| format!("**/proj{index}/*/build-?[a-z]*/*.o\n"))
        .collect();
    let excludes = qdu.base.join("excludes");
    fs::write(&excludes, patterns).unwrap();
    qdu.snapshot(&[
        "--exclude-from",
        excludes.to_str().unwrap(),
        "--file-top",
        "10",
    ])
    .ok();
    assert_eq!(paths(&qdu.json(&["files"])["rows"]), ["keep/main.c"]);
}

#[test]
fn scopes_cannot_climb_out_of_the_root() {
    let qdu = Qdu::new();
    qdu.write_file("a/file", 1024);
    std::os::unix::fs::symlink("/", qdu.root.join("link")).unwrap();
    qdu.snapshot(&[]).ok();
    for scope in ["..", "a/..", "link/..", "a/../.."] {
        let run = qdu.run(&["files", "--live", scope]).status(2);
        assert!(
            run.stderr.contains("inside the snapshot root"),
            "{scope}: {}",
            run.stderr
        );
        qdu.run(&["show", scope]).status(2);
    }
    let run = qdu
        .run(&["files", "--live", "link", "--format", "json"])
        .status(2);
    assert!(run.stderr.contains("outside the root"), "{}", run.stderr);
    let missing = qdu.run(&["files", "--live", "no/such/dir"]).status(2);
    assert!(missing.stderr.contains("no/such/dir"), "{}", missing.stderr);
}

/// Tables are never shortened off a terminal; on one, a long path loses
/// its middle and a snapshot ID stays whole.
#[test]
fn tables_shorten_paths_only_on_a_terminal() {
    let mut qdu = Qdu::new();
    let long = format!("{}/{}/end-of-path", "a".repeat(60), "b".repeat(60));
    qdu.write_file(&format!("{long}/file"), 1024);
    qdu.snapshot(&[]).ok();
    qdu.env("COLUMNS", "80");
    let piped = qdu.run(&["show", "-L", "3", "--style", "plain"]).ok();
    assert!(piped.stdout.contains(&long), "{}", piped.stdout);
    let command = qdu.command(&["show", "-L", "3", "--style", "plain"]);
    let Some((status, shown, _)) = support::run_on_terminal(command, support::Terminal::Stdout)
    else {
        return;
    };
    assert!(status.success());
    assert!(!shown.contains(&long), "{shown}");
    assert!(shown.contains("…"), "{shown}");
    assert!(shown.contains("end-of-path"), "{shown}");
    let id = qdu.snapshot_ids().pop().unwrap();
    let command = qdu.command(&["list", "--style", "plain"]);
    if let Some((_, listed, _)) = support::run_on_terminal(command, support::Terminal::Stdout) {
        assert!(listed.contains(&id), "{listed}");
    }
}

#[test]
fn diff_warns_when_depths_differ() {
    let qdu = Qdu::new();
    qdu.write_file("a/b/c/file", 4096);
    qdu.snapshot(&["--record-max-depth", "1"]).ok();
    qdu.snapshot(&["--record-max-depth", "3"]).ok();
    let run = qdu.run(&["diff"]).ok();
    assert!(
        run.stderr.contains("different depths (1 and 3)"),
        "{}",
        run.stderr
    );
    qdu.snapshot(&["--record-max-depth", "3"]).ok();
    assert!(qdu.run(&["diff"]).ok().stderr.is_empty());
}

#[test]
fn record_max_depth_limits_stored_rows_but_not_totals() {
    let qdu = Qdu::new();
    qdu.write_file("a/b/c/deep", 4096);
    qdu.write_file("x/file", 4096);
    qdu.snapshot(&["--record-max-depth", "1"]).ok();
    let payload = qdu.json(&["show", "-L", "5"]);
    let mut stored = paths(&payload["rows"]);
    stored.sort();
    assert_eq!(stored, ["a", "x"]);
    // The deep file still counts toward its stored ancestor.
    assert_eq!(payload["rows"][0]["path"], "a");
    assert_eq!(payload["rows"][0]["file_count"], 1);
    qdu.run(&["show", "a/b"]).status(2);
}

#[test]
fn match_keeps_paths_by_substring_or_glob() {
    let qdu = Qdu::new();
    qdu.write_file("models/llama/w.bin", 50000);
    qdu.write_file("cache/pip/x.whl", 9000);
    qdu.write_file("data/file", 4096);
    qdu.snapshot(&[]).ok();
    let matched =
        |pattern: &str| paths(&qdu.json(&["show", "-L", "3", "--match", pattern])["rows"]);
    // Text matches anywhere, ignoring case; a glob must match the whole path.
    assert_eq!(matched("LLAMA"), ["models/llama"]);
    assert_eq!(matched("*/pip"), ["cache/pip"]);
    assert!(matched("pip/*").is_empty());
}

/// The first snapshot of an unconfigured profile keeps the settings it was
/// given; later ones apply theirs once unless they use --save.
#[test]
fn first_snapshot_keeps_its_settings() {
    let qdu = Qdu::new();
    qdu.write_file("data/file", 1024);
    qdu.write_file("cache/file", 1024);
    let root = qdu.root.to_str().unwrap();
    let run = qdu
        .run(&[
            "snapshot",
            "--path",
            root,
            "--with-users",
            "--file-top",
            "5",
            "--exclude",
            "cache",
        ])
        .ok();
    assert!(run.stdout.contains("Saved"), "{}", run.stdout);
    let first = qdu.records().pop().unwrap();
    qdu.run(&["snapshot", "--quiet"]).ok();
    let second = qdu.records().pop().unwrap();
    assert_eq!(second["collect_users"], true);
    assert_eq!(second["large_file_limit"], 5);
    assert_eq!(second["exclude_hash"], first["exclude_hash"]);
    let settings = qdu.json(&["profile", "show"])["settings"].clone();
    assert_eq!(settings["path"], root);
    assert_eq!(settings["exclude"], serde_json::json!(["cache"]));
    // A configured profile applies other settings to one snapshot only...
    qdu.run(&["snapshot", "--quiet", "--file-top", "9"]).ok();
    assert_eq!(qdu.records().pop().unwrap()["large_file_limit"], 9);
    let settings = qdu.json(&["profile", "show"])["settings"].clone();
    assert_eq!(settings["large_file_limit"], 5);
    // ...unless they are saved.
    let saved = qdu
        .run(&["snapshot", "--save", "--file-top", "7", "--format", "json"])
        .ok()
        .json();
    assert_eq!(
        saved["saved_settings"],
        serde_json::json!(["large_file_limit"])
    );
    let settings = qdu.json(&["profile", "show"])["settings"].clone();
    assert_eq!(settings["large_file_limit"], 7);
}

/// A profile that has snapshots but no section, as one from the Python qdu
/// 0.2.0 does, is not new: its options apply to one snapshot unless they
/// are saved.
#[test]
fn only_a_new_profile_keeps_its_settings_unasked() {
    let qdu = Qdu::new();
    qdu.write_file("data/file", 1024);
    let root = qdu.root.to_str().unwrap();
    qdu.run(&["snapshot", "--path", root, "--quiet"]).ok();
    fs::remove_file(qdu.config_file()).unwrap();
    let run = qdu
        .run(&["snapshot", "--with-users", "--format", "json"])
        .ok()
        .json();
    assert_eq!(run["saved_settings"], serde_json::json!([]));
    assert!(!qdu.config_file().exists());
    assert_eq!(qdu.records().pop().unwrap()["collect_users"], true);
    // Its snapshots give the directory, so settings need no --path...
    qdu.run(&["profile", "set", "default", "--keep-snapshots", "9"])
        .ok();
    let settings = qdu.json(&["profile", "show"])["settings"].clone();
    assert_eq!(settings["keep_snapshots"], 9);
    // ...and another directory would make every later snapshot fail.
    let other = qdu.root.join("data");
    let run = qdu
        .run(&[
            "profile",
            "set",
            "default",
            "--path",
            other.to_str().unwrap(),
        ])
        .status(2);
    assert!(run.stderr.contains("holds snapshots of"), "{}", run.stderr);
    qdu.run(&["profile", "set", "default", "--path", root]).ok();
    qdu.snapshot(&[]).ok();
}

#[test]
fn selectors_name_snapshots_by_position_age_date_and_label() {
    let qdu = Qdu::new();
    qdu.write_file("data", 1024);
    for _ in 0..3 {
        qdu.snapshot(&[]).ok();
    }
    let ids = qdu.snapshot_ids();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    let mut index = qdu.index();
    for (position, days) in [(0, 40), (1, 10), (2, 0)] {
        index["snapshots"][position]["created_epoch"] = (now - days * 86_400).into();
    }
    qdu.write_index(&index);
    let selected = |selector: &str| {
        qdu.json(&["show", "--snapshot", selector])["snapshot"]["snapshot_id"]
            .as_str()
            .unwrap()
            .to_string()
    };
    assert_eq!(selected("latest~2"), ids[0]);
    assert_eq!(selected("previous"), ids[1]);
    assert_eq!(selected("30d"), ids[0]);
    assert_eq!(selected("5d"), ids[1]);
    assert_eq!(selected("2999-12-31"), ids[2]);
    for (selector, message) in [
        ("latest~3", "only 3 complete snapshots"),
        ("60d", "the oldest is from"),
        ("2000-01-01", "on or before 2000-01-01"),
        ("@missing", "no snapshot is labeled missing"),
    ] {
        let run = qdu.run(&["show", "--snapshot", selector]).status(2);
        assert!(run.stderr.contains(message), "{selector}: {}", run.stderr);
    }
    qdu.run(&["show", "--snapshot", "2026-02-30"]).status(2);

    qdu.run(&["label", "baseline", "--snapshot", "latest~2"])
        .ok();
    assert_eq!(selected("@baseline"), ids[0]);
    let moved = qdu.run(&["label", "baseline"]).ok();
    assert!(moved.stdout.contains("moved from"), "{}", moved.stdout);
    assert_eq!(selected("@baseline"), ids[2]);
    qdu.run(&["label", "baseline", "--remove"]).ok();
    qdu.run(&["show", "--snapshot", "@baseline"]).status(2);
    qdu.run(&["label", "-bad"]).status(2);

    // A labeled snapshot outlives retention.
    qdu.run(&["label", "keep", "--snapshot", "latest~2"]).ok();
    qdu.snapshot(&["--keep-snapshots", "2"]).ok();
    let listed = qdu.json(&["list"]);
    let kept: Vec<(String, serde_json::Value)> = listed["rows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| {
            (
                row["snapshot_id"].as_str().unwrap().to_string(),
                row["labels"].clone(),
            )
        })
        .collect();
    assert_eq!(kept.len(), 3, "{kept:?}");
    assert!(kept.contains(&(ids[0].clone(), serde_json::json!(["keep"]))));
    assert!(!kept.iter().any(|(id, _)| *id == ids[1]), "{kept:?}");
    qdu.snapshot(&["--label", "newest"]).ok();
    let newest = qdu.json(&["list", "-n", "1"])["rows"][0]["labels"].clone();
    assert_eq!(newest, serde_json::json!(["newest"]));
}

/// Debug builds fail to read the directories named in QDU_DEBUG_IO_ERROR.
#[cfg(debug_assertions)]
#[test]
fn latest_notes_a_newer_incomplete_snapshot() {
    let mut qdu = Qdu::new();
    qdu.write_file("ok/file", 1024);
    qdu.write_file("broken/file", 1024);
    qdu.snapshot(&[]).ok();
    qdu.env("QDU_DEBUG_IO_ERROR", "broken");
    qdu.snapshot(&[]).status(3);
    let run = qdu.run(&["show"]).ok();
    assert!(
        run.stderr.contains("note: the newest snapshot"),
        "{}",
        run.stderr
    );
    let any = qdu.run(&["show", "--snapshot", "latest-any"]).ok();
    assert!(!any.stderr.contains("note:"), "{}", any.stderr);
}

#[test]
fn history_follows_a_directory_and_forecasts_its_limit() {
    let qdu = Qdu::new();
    qdu.write_file("keep/file", 4096);
    qdu.snapshot(&[]).ok();
    qdu.write_file("data/one", 1 << 20);
    qdu.snapshot(&[]).ok();
    qdu.write_file("data/two", 1 << 20);
    qdu.snapshot(&[]).ok();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    let mut index = qdu.index();
    for (position, weeks) in [(0, 2), (1, 1), (2, 0)] {
        index["snapshots"][position]["created_epoch"] = (now - weeks * 7 * 86_400).into();
    }
    qdu.write_index(&index);
    let root = qdu.root.to_str().unwrap();
    qdu.run(&[
        "profile",
        "set",
        "default",
        "--path",
        root,
        "--capacity-limit",
        "1TiB",
    ])
    .ok();

    let whole = qdu.json(&["history"]);
    assert_eq!(whole["scope"], ".");
    assert_eq!(whole["rows"].as_array().unwrap().len(), 3);
    assert!(whole["growth_per_week"].as_f64().unwrap() > 0.0);
    assert!(
        whole["forecast"]["capacity_limit_reached_epoch"]
            .as_i64()
            .unwrap()
            > now
    );

    let data = qdu.json(&["history", "data"]);
    let values: Vec<serde_json::Value> = data["rows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| row["allocated_bytes"].clone())
        .collect();
    assert_eq!(values[0], serde_json::Value::Null);
    assert!(values[2].as_i64().unwrap() > values[1].as_i64().unwrap());
    assert_eq!(data["rows"][0]["change"], serde_json::Value::Null);
    assert!(data["rows"][2]["change"].as_i64().unwrap() > 0);
    // A directory's history ignores the limit, which applies to the root.
    assert_eq!(
        data["forecast"]["capacity_limit_bytes"],
        serde_json::Value::Null
    );

    let table = qdu.run(&["history", "-n", "2", "--style", "plain"]).ok();
    assert!(table.stdout.contains("Growth"), "{}", table.stdout);
    assert!(table.stdout.contains("per week"), "{}", table.stdout);
    // Snapshot IDs contain "Z-" after their UTC time.
    let rows = table
        .stdout
        .lines()
        .filter(|line| line.contains("Z-"))
        .count();
    assert_eq!(rows, 2, "{}", table.stdout);
}

#[test]
fn snapshot_reports_in_json() {
    let qdu = Qdu::new();
    qdu.write_file("data", 1024);
    let root = qdu.root.to_str().unwrap();
    let payload = qdu
        .run(&["snapshot", "--path", root, "--format", "json"])
        .ok()
        .json();
    assert_eq!(payload["format_version"], 1);
    assert_eq!(payload["command"], "snapshot");
    assert_eq!(payload["profile"], "default");
    let id = qdu.snapshot_ids().pop().unwrap();
    assert_eq!(payload["snapshot"]["snapshot_id"], id.as_str());
    assert_eq!(payload["scan"]["error_count"], 0);
    assert_eq!(payload["retention"]["removed"], serde_json::json!([]));
}

#[test]
fn list_and_errors_honor_top() {
    let qdu = Qdu::new();
    qdu.write_file("data", 1024);
    for _ in 0..3 {
        qdu.snapshot(&[]).ok();
    }
    let newest = qdu.snapshot_ids().pop().unwrap();
    let listed = qdu.json(&["list", "-n", "1"])["rows"].clone();
    assert_eq!(listed.as_array().unwrap().len(), 1);
    assert_eq!(listed[0]["snapshot_id"], newest.as_str());
    if unreadable_directory(&qdu, "first").is_none()
        || unreadable_directory(&qdu, "second").is_none()
    {
        return;
    }
    qdu.snapshot(&[]).ok();
    let errors = qdu.json(&["errors", "-n", "1"]);
    assert_eq!(errors["denied_count"], 2);
    assert_eq!(errors["rows"].as_array().unwrap().len(), 1);
}

#[test]
fn diff_from_selects_the_older_snapshot() {
    let qdu = Qdu::new();
    for size in [1024, 8192, 65536] {
        qdu.write_file("data/file", size);
        qdu.snapshot(&[]).ok();
    }
    let ids = qdu.snapshot_ids();
    let payload = qdu.json(&["diff", "--from", &ids[0]]);
    assert_eq!(payload["previous"]["snapshot_id"], ids[0].as_str());
    assert_eq!(payload["current"]["snapshot_id"], ids[2].as_str());
    let default = qdu.json(&["diff"]);
    assert_eq!(default["previous"]["snapshot_id"], ids[1].as_str());
}

/// Debug builds fail to read the directories named in QDU_DEBUG_IO_ERROR.
#[cfg(debug_assertions)]
#[test]
fn incomplete_snapshot_does_not_replace_the_latest_complete_one() {
    let mut qdu = Qdu::new();
    qdu.write_file("ok/file", 1024);
    qdu.write_file("broken/file", 1024);
    qdu.snapshot(&[]).ok();
    let first = qdu.index()["latest_complete"].clone();
    qdu.env("QDU_DEBUG_IO_ERROR", "broken");
    qdu.snapshot(&[]).status(3);
    let index = qdu.index();
    assert_eq!(index["latest_complete"], first);
    assert_ne!(index["latest_any"], first);
    assert_eq!(qdu.json(&["show"])["snapshot"]["snapshot_id"], first);
}

#[cfg(debug_assertions)]
#[test]
fn quiet_incomplete_snapshot_still_reports_errors() {
    let mut qdu = Qdu::new();
    qdu.write_file("ok/file", 1024);
    qdu.write_file("broken/file", 1024);
    qdu.env("QDU_DEBUG_IO_ERROR", "broken");
    let run = qdu.snapshot(&[]).status(3);
    assert_eq!(run.stdout, "");
    assert!(run.stderr.contains("could not be read"), "{}", run.stderr);
    let payload = qdu.json(&["errors"]);
    assert_eq!(payload["error_count"], 1);
    assert_eq!(payload["denied_count"], 0);
    assert_eq!(payload["rows"][0]["path"], "broken");
    assert_eq!(payload["rows"][0]["operation"], "scandir");
    assert_eq!(payload["rows"][0]["kind"], "error");
    assert!(
        paths(&qdu.json(&["show", "--snapshot", "latest-any"])["rows"]).contains(&"ok".to_string())
    );
}

/// A directory the user may not read is recorded, but the snapshot is
/// complete: scanning again as the same user would read no more.
#[test]
fn denied_directories_leave_the_snapshot_complete() {
    let qdu = Qdu::new();
    qdu.write_file("ok/file", 1024);
    qdu.snapshot(&[]).ok();
    if unreadable_directory(&qdu, "denied").is_none() {
        return;
    }
    let run = qdu.snapshot(&[]).ok();
    assert_eq!(run.stdout, "");
    assert!(run.stderr.contains("lack of permission"), "{}", run.stderr);
    let index = qdu.index();
    assert_eq!(index["latest_complete"], index["latest_any"]);
    let record = qdu.records().pop().unwrap();
    assert_eq!(record["complete"], true);
    assert_eq!(record["error_count"], 0);
    assert_eq!(record["denied_count"], 1);
    let payload = qdu.json(&["errors"]);
    assert_eq!(payload["rows"][0]["path"], "denied");
    assert_eq!(payload["rows"][0]["operation"], "scandir");
    assert_eq!(payload["rows"][0]["kind"], "denied");
    let shown = qdu.run(&["show"]).ok();
    assert!(
        shown.stdout.contains("1 path denied permission"),
        "{}",
        shown.stdout
    );
    let root = qdu.root.to_str().unwrap();
    let live = qdu
        .run(&["files", "--live", "--path", root, "--format", "json"])
        .ok();
    assert!(live.stderr.contains("denied"), "{}", live.stderr);
}

/// Names reach warnings and error messages with their control characters
/// escaped, so a file name cannot send escape sequences to the terminal.
#[test]
fn warnings_escape_control_characters_in_names() {
    let qdu = Qdu::new();
    let name = "evil\u{1b}]0;title\u{7}\u{9b}31m";
    if unreadable_directory(&qdu, name).is_none() {
        return;
    }
    let run = qdu.snapshot(&[]).ok();
    assert!(!run.stderr.contains('\u{1b}'), "{:?}", run.stderr);
    assert!(!run.stderr.contains('\u{9b}'), "{:?}", run.stderr);
    assert!(
        run.stderr.contains("evil\\u001b]0;title\\u0007\\u009b31m"),
        "{:?}",
        run.stderr
    );
    let missing = qdu.run(&["show", "no\u{1b}[2Jsuch"]).status(2);
    assert!(
        missing.stderr.contains("no\\u001b[2Jsuch"),
        "{:?}",
        missing.stderr
    );
}

/// A directory that may be listed but not searched denies the status of
/// every entry; that is one record, not one per entry.
#[test]
fn entries_of_an_unsearchable_directory_are_one_record() {
    let qdu = Qdu::new();
    for index in 0..5 {
        qdu.write_file(&format!("listed/file{index}"), 1024);
    }
    if !support::permissions_enforced() {
        return;
    }
    let listed = qdu.root.join("listed");
    fs::set_permissions(&listed, fs::Permissions::from_mode(0o600)).unwrap();
    qdu.snapshot(&[]).ok();
    let payload = qdu.json(&["errors"]);
    assert_eq!(payload["denied_count"], 1);
    let errors = payload["rows"].as_array().unwrap();
    assert_eq!(errors.len(), 1);
    assert_eq!(errors[0]["path"], "listed");
    assert_eq!(errors[0]["operation"], "stat");
    assert!(
        errors[0]["message"]
            .as_str()
            .unwrap()
            .contains("(5 entries)"),
        "{}",
        errors[0]["message"]
    );
}

#[test]
fn profile_root_is_not_silently_changed() {
    let qdu = Qdu::new();
    qdu.write_file("data", 1024);
    qdu.snapshot(&[]).ok();
    let other = qdu.base.join("other");
    fs::create_dir(&other).unwrap();
    let run = qdu
        .run(&["snapshot", "--path", other.to_str().unwrap(), "--quiet"])
        .status(2);
    assert!(
        run.stderr.contains(&format!(
            "holds snapshots of {}, not {}",
            qdu.root.display(),
            other.display()
        )),
        "{}",
        run.stderr
    );
    let status = |doctor: &serde_json::Value, name: &str| {
        doctor["rows"]
            .as_array()
            .unwrap()
            .iter()
            .find(|item| item["name"] == name)
            .unwrap()["status"]
            .clone()
    };
    // Without a configured path, the profile scans its snapshots' root.
    let doctor = qdu.json(&["doctor"]);
    assert_eq!(status(&doctor, "Snapshot root"), "OK");
    assert_eq!(status(&doctor, "Latest snapshot"), "OK");
    // profile set refuses a directory that differs from the snapshots'...
    let home = qdu.home.to_str().unwrap();
    qdu.run(&["profile", "set", "default", "--path", home])
        .status(2);
    // ...and with one edited into the file, 'qdu snapshot' refuses to run.
    fs::create_dir_all(qdu.config_file().parent().unwrap()).unwrap();
    fs::write(
        qdu.config_file(),
        format!("[profile:default]\npath = {home}\n"),
    )
    .unwrap();
    let doctor = qdu.run(&["doctor", "--format", "json"]).status(1).json();
    assert_eq!(status(&doctor, "Snapshot root"), "FAILED");
    qdu.run(&["snapshot", "--quiet"]).status(2);
}

#[test]
fn later_snapshots_reuse_the_first_path() {
    let qdu = Qdu::new();
    qdu.write_file("data", 1024);
    qdu.snapshot(&[]).ok();
    qdu.write_file("more", 4096);
    qdu.run(&["snapshot", "--quiet"]).ok();
    let records = qdu.records();
    assert_eq!(records.len(), 2);
    assert_eq!(records[1]["root"], qdu.root.to_str().unwrap());
    let shown = qdu.json(&["profile", "show", "default"]);
    assert_eq!(shown["settings"]["path"], qdu.root.to_str().unwrap());
}

#[test]
fn pruning_keeps_the_requested_count() {
    let qdu = Qdu::new();
    for size in [1024, 2048, 4096] {
        qdu.write_file("data", size);
        qdu.snapshot(&["--keep-snapshots", "2"]).ok();
    }
    assert_eq!(qdu.records().len(), 2);
}

#[test]
fn fatal_snapshot_error_leaves_no_temporary_database() {
    let qdu = Qdu::new();
    let missing = qdu.base.join("missing");
    qdu.run(&["snapshot", "--path", missing.to_str().unwrap(), "--quiet"])
        .status(2);
    let temporary = qdu.profile_dir("default").join("tmp");
    if temporary.exists() {
        assert_eq!(fs::read_dir(temporary).unwrap().count(), 0);
    }
}

#[test]
fn interrupted_snapshot_leaves_no_temporary_database() {
    let qdu = Qdu::new();
    for index in 0..20000 {
        qdu.write_file(&format!("d{}/f{index}", index % 200), 0);
    }
    let root = qdu.root.to_str().unwrap().to_string();
    let mut child = qdu
        .command(&["snapshot", "--path", &root, "--quiet", "--threads", "1"])
        .spawn()
        .unwrap();
    std::thread::sleep(std::time::Duration::from_millis(20));
    // SAFETY: kill only sends a signal to the child we started.
    unsafe {
        libc::kill(child.id() as i32, libc::SIGINT);
    }
    let status = child.wait().unwrap();
    assert!(matches!(status.code(), Some(0 | 130)), "{status:?}");
    let temporary = qdu.profile_dir("default").join("tmp");
    if temporary.exists() {
        assert_eq!(fs::read_dir(temporary).unwrap().count(), 0);
    }
}

#[test]
fn archived_snapshots_verify_and_take_part_in_diff() {
    let qdu = Qdu::new();
    qdu.write_file("data/file", 1024);
    qdu.snapshot(&[]).ok();
    qdu.write_file("data/file", 16384);
    qdu.snapshot(&[]).ok();
    qdu.run(&["compact", "--older-than", "0s", "--keep-latest", "1"])
        .ok();
    qdu.run(&["verify", "--all"]).ok();
    let payload = qdu.json(&["diff"]);
    assert!(payload["rows"][0]["change"].as_i64().unwrap() > 0);
}

#[test]
fn thread_counts_do_not_change_the_snapshot() {
    let qdu = Qdu::new();
    for index in 0..300 {
        qdu.write_file(
            &format!("d{}/e{}/f{index}", index % 7, index % 13),
            index * 37,
        );
    }
    let original = qdu.write_file("d1/original", 9000);
    fs::hard_link(&original, qdu.root.join("d0/link")).unwrap();
    let mut tables = Vec::new();
    for threads in ["1", "8"] {
        qdu.snapshot(&["--threads", threads, "--with-users", "--file-top", "50"])
            .ok();
        let connection = rusqlite::Connection::open(qdu.latest_database()).unwrap();
        let mut rows = Vec::new();
        for query in [
            "SELECT path, allocated_bytes, apparent_bytes, file_count, directory_count, \
             inode_count FROM directories ORDER BY path",
            "SELECT uid || ' ' || path, allocated_bytes, apparent_bytes, file_count, \
             directory_count, inode_count FROM owner_directories ORDER BY uid, path",
            "SELECT path, uid, allocated_bytes, apparent_bytes, 0, 0 FROM large_files ORDER BY path",
        ] {
            let mut statement = connection.prepare(query).unwrap();
            let found: Vec<(String, i64, i64, i64, i64, i64)> = statement
                .query_map([], |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                    ))
                })
                .unwrap()
                .map(Result::unwrap)
                .collect();
            rows.push(found);
        }
        tables.push(rows);
    }
    assert_eq!(tables[0], tables[1]);
    qdu.run(&["snapshot", "--threads", "0"]).status(2);
}
