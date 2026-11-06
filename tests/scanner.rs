//! Traversal rules: depth, owners, filesystem boundaries, and progress.

mod support;

use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::Path;

use support::{Qdu, Terminal, run_on_terminal};

#[test]
fn deep_trees_are_scanned_completely() {
    let qdu = Qdu::new();
    let depth = 300;
    let leaf = (0..depth).fold(qdu.root.clone(), |path, _| path.join("d"));
    fs::create_dir_all(&leaf).unwrap();
    fs::write(leaf.join("file"), vec![b'x'; 4096]).unwrap();
    fs::write(qdu.root.join("d/sibling"), vec![b'x'; 4096]).unwrap();
    qdu.snapshot(&[]).ok();
    let record = qdu.records().pop().unwrap();
    assert_eq!(record["complete"], true);
    assert_eq!(record["file_count"], 2);
    assert_eq!(record["directory_count"], depth + 1);
}

/// Directories are opened relative to their parent, so a path longer than
/// the system accepts in one call is read, down to a directory large enough
/// to be examined on several threads. With few file descriptors no parent
/// stays open, and such paths are opened one component at a time.
#[test]
fn paths_longer_than_the_system_limit_are_scanned() {
    use std::os::unix::process::CommandExt;

    let qdu = Qdu::new();
    let name = "d".repeat(200);
    let levels = 25;
    let mut directory = fs::File::open(&qdu.root).unwrap();
    for _ in 0..levels {
        directory = support::make_directory_at(&directory, &name);
        support::write_file_at(&directory, "file", 100);
    }
    let big = support::make_directory_at(&directory, "big");
    for index in 0..2100 {
        support::write_file_at(&big, &format!("f{index:04}"), 0);
    }
    support::write_file_at(&big, "largest", 64 * 1024);
    let root = qdu.root.to_str().unwrap();
    let expect_everything = |run: support::Run| {
        let run = run.ok();
        assert!(run.stderr.is_empty(), "{}", run.stderr);
        let record = qdu.records().pop().unwrap();
        assert_eq!(record["complete"], true);
        assert_eq!(record["file_count"], levels + 2101);
        assert_eq!(record["directory_count"], levels + 2);
    };
    expect_everything(qdu.snapshot(&[]));

    // Four threads keep the descriptors in use well below the limit.
    let mut command = qdu.command(&["snapshot", "--path", root, "--quiet", "--threads", "4"]);
    // SAFETY: setrlimit is async-signal-safe and only changes the child.
    unsafe {
        command.pre_exec(|| {
            let limit = libc::rlimit {
                rlim_cur: 40,
                rlim_max: 40,
            };
            if libc::setrlimit(libc::RLIMIT_NOFILE, &limit) == 0 {
                Ok(())
            } else {
                Err(std::io::Error::last_os_error())
            }
        });
    }
    let output = command.output().unwrap();
    expect_everything(support::Run {
        status: output.status.code().unwrap_or(-1),
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    });

    let files = qdu.json(&["files", "--live", "--path", root, "-n", "1"]);
    let paths = support::paths(&files["rows"]);
    assert_eq!(paths.len(), 1);
    assert!(paths[0].ends_with("/big/largest"), "{}", paths[0]);
}

/// Owner rows follow the documented rule: every inode counts toward its
/// directory's ancestors down to `--user-max-depth`.
#[test]
fn owner_rows_sum_each_uid_down_to_the_user_max_depth() {
    let qdu = Qdu::new();
    for (path, size) in [
        ("a/mine", 1000),
        ("a/other", 3000),
        ("a/b/c/d/deep", 5000),
        ("e/mine", 7000),
    ] {
        qdu.write_file(path, size);
    }
    let maximum = 2;
    let mut expected: BTreeMap<(u32, String), [i64; 5]> = BTreeMap::new();
    let mut add = |uid: u32, owner_path: &str, metadata: &fs::Metadata, is_directory: bool| {
        let parts: Vec<&str> = if owner_path == "." {
            Vec::new()
        } else {
            owner_path.split('/').collect()
        };
        for depth in 0..=parts.len().min(maximum) {
            let key = if depth == 0 {
                ".".to_string()
            } else {
                parts[..depth].join("/")
            };
            let usage = expected.entry((uid, key)).or_default();
            usage[0] += metadata.blocks() as i64 * 512;
            usage[1] += metadata.size() as i64;
            usage[2] += i64::from(!is_directory);
            usage[3] += i64::from(is_directory);
            usage[4] += 1;
        }
    };
    fn walk(root: &Path, relative: &str, visit: &mut dyn FnMut(&str, &fs::Metadata, bool, &str)) {
        let path = if relative == "." {
            root.to_path_buf()
        } else {
            root.join(relative)
        };
        let metadata = fs::symlink_metadata(&path).unwrap();
        visit(relative, &metadata, true, relative);
        for entry in fs::read_dir(&path).unwrap() {
            let entry = entry.unwrap();
            let name = entry.file_name().into_string().unwrap();
            let child = if relative == "." {
                name
            } else {
                format!("{relative}/{name}")
            };
            let metadata = entry.metadata().unwrap();
            if metadata.is_dir() {
                walk(root, &child, visit);
            } else {
                visit(&child, &metadata, false, relative);
            }
        }
    }
    walk(
        &qdu.root,
        ".",
        &mut |_path, metadata, is_directory, container| {
            add(metadata.uid(), container, metadata, is_directory);
        },
    );
    let depth = maximum.to_string();
    qdu.snapshot(&["--with-users", "--user-max-depth", &depth])
        .ok();
    let connection = rusqlite::Connection::open(qdu.latest_database()).unwrap();
    let mut statement = connection
        .prepare(
            "SELECT uid, path, allocated_bytes, apparent_bytes, file_count, directory_count, \
             inode_count FROM owner_directories",
        )
        .unwrap();
    let stored: BTreeMap<(u32, String), [i64; 5]> = statement
        .query_map([], |row| {
            Ok((
                (row.get::<_, u32>(0)?, row.get::<_, String>(1)?),
                [
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                    row.get(6)?,
                ],
            ))
        })
        .unwrap()
        .map(Result::unwrap)
        .collect();
    assert_eq!(stored, expected);
    let uid = fs::metadata(&qdu.root).unwrap().uid();
    assert!(stored.contains_key(&(uid, "a/b".to_string())));
    assert!(!stored.contains_key(&(uid, "a/b/c".to_string())));
    let totals: i64 = connection
        .query_row(
            "SELECT inode_count FROM owners WHERE uid = ?1",
            [uid],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(totals, expected[&(uid, ".".to_string())][4]);
}

/// `/dev` on Linux usually holds other filesystems, such as `/dev/shm`.
#[cfg(target_os = "linux")]
#[test]
fn one_file_system_skips_mounted_directories() {
    let device = |path: &str| fs::metadata(path).map(|metadata| metadata.dev()).ok();
    let mounted = ["shm", "pts", "mqueue"].into_iter().find(|name| {
        device(&format!("/dev/{name}")).is_some_and(|dev| Some(dev) != device("/dev"))
    });
    let Some(mounted) = mounted else {
        return;
    };
    let qdu = Qdu::new();
    let run = qdu.run(&["snapshot", "--path", "/dev", "--quiet"]);
    assert!(matches!(run.status, 0 | 3), "{}", run.stderr);
    let skipped = qdu.json(&["show", "--snapshot", "latest-any"])["filesystem"].clone();
    assert_eq!(skipped["mode"], "one-filesystem");
    let names: Vec<String> = skipped["skipped_filesystems"]
        .as_array()
        .unwrap()
        .iter()
        .map(|name| name.as_str().unwrap().to_string())
        .collect();
    assert!(names.contains(&mounted.to_string()), "{names:?}");
    let crossing = qdu.run(&[
        "snapshot",
        "--path",
        "/dev",
        "--quiet",
        "--cross-filesystems",
    ]);
    assert!(matches!(crossing.status, 0 | 3), "{}", crossing.stderr);
    let filesystem = qdu.json(&["show", "--snapshot", "latest-any"])["filesystem"].clone();
    assert_eq!(filesystem["mode"], "cross-filesystems");
    assert_eq!(filesystem["skipped_filesystem_count"], 0);
    // Neither /dev nor its mounts are automounter maps.
    let connection = rusqlite::Connection::open(qdu.latest_database()).unwrap();
    let automounts: String = connection
        .query_row(
            "SELECT value FROM metadata WHERE key = 'automount_paths'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(automounts, "");
}

#[test]
fn progress_is_drawn_on_a_terminal_and_erased() {
    let qdu = Qdu::new();
    for index in 0..5 {
        qdu.write_file(&format!("dir-{index}/file"), 4096);
    }
    let root = qdu.root.to_str().unwrap().to_string();
    let command = qdu.command(&["snapshot", "--path", &root]);
    let Some((status, written, summary)) = run_on_terminal(command, Terminal::Stderr) else {
        return;
    };
    assert!(status.success());
    assert!(written.starts_with("\r"), "{written:?}");
    assert!(
        written.contains("Writing snapshot (10 entries scanned in "),
        "{written:?}"
    );
    assert!(written.ends_with("\r\x1b[K"), "{written:?}");
    assert!(summary.contains("Snapshot saved"), "{summary}");
}
