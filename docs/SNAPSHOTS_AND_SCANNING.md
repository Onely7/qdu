# Snapshots and scanning scope

English | [日本語](SNAPSHOTS_AND_SCANNING_ja.md)

## Take a snapshot

```bash
qdu snapshot
qdu snapshot --path /srv/data
qdu snapshot --profile data
```

A profile is bound to its first persisted root. qdu refuses to silently change that root; create another profile for another tree.

The first snapshot of a new profile, one with neither a configuration section nor snapshots, saves the directory, the exclusions, and the profile settings it was given, and says which; `qdu profile show` then lists them. Later snapshots apply their options to that run only, unless `--save` keeps them too. `--label NAME` labels the new snapshot; see [Operations](OPERATIONS.md#labels-and-forgetting-snapshots).

Useful collection options are:

```bash
qdu snapshot --with-users
qdu snapshot --record-max-depth 6
qdu snapshot --file-top 2000
qdu snapshot --exclude .cache --exclude '*.tmp'
qdu snapshot --exclude-from ~/.config/qdu/excludes
```

`--with-users` records aggregate owner information. `--record-max-depth` limits stored directory detail, not filesystem traversal; `qdu diff` warns when it compares two snapshots stored to different depths. `--file-top` controls how many large-file candidates are retained. `--format json` prints the new snapshot's record, its scan errors, and what retention removed.

When standard error is a terminal, `qdu snapshot` keeps one progress line up to date: entries and directories scanned so far, the allocation counted, the elapsed time, and the directory being read. `--quiet` hides it, and nothing is written there when standard error is redirected to a file or pipe.

qdu reads several directories at once, which shortens scans most on NFS and other network storage, where each directory read waits for the server. It uses as many threads as the machine has CPUs, up to 16. `--threads N` or the `QDU_THREADS` environment variable sets another count; lower it on a shared login node, or raise it for a file server that answers many requests in parallel. The thread count does not change the result.

If you interrupt a snapshot with Ctrl-C, qdu removes the partial snapshot database and exits with status 130; the earlier snapshots stay as they were.

## Complete and incomplete snapshots

A scan sorts the failures it meets into three kinds:

| Kind | Examples | Effect |
|---|---|---|
| Vanished | A file or directory removed after its directory was listed, or a directory replaced by another one or by a symbolic link | Left out without a record |
| Denied | A directory or entry the user may not read (`EACCES`, `EPERM`) | Recorded with the kind `denied`; the snapshot stays complete, and `qdu snapshot` warns and exits with 0 |
| Error | Anything else, such as `EIO` or a stale NFS handle | Recorded with the kind `error`; the snapshot is incomplete, and `qdu snapshot` exits with 3 |

A directory that may be listed but not searched denies the status of each of its entries; that is one record with the number of entries. A denied directory and its contents are not counted, and scanning again as the same user would read no more.

An incomplete snapshot stays available through `latest-any`, while `latest` continues to select the most recent complete snapshot. When a command reads `latest` and a newer snapshot is incomplete, qdu notes it on standard error.

```bash
qdu list
qdu errors --snapshot latest-any
qdu show --snapshot latest-any
```

Fatal failures clean up the temporary database and do not publish a snapshot.

## Filesystem boundaries

The default is one filesystem. Mounted child trees are skipped and reported:

```bash
qdu snapshot --one-file-system
```

Use cross-filesystem mode when the intended dataset spans mounts, such as autofs-managed NFS home directories:

```bash
qdu snapshot --cross-filesystems
```

Persist the choice in a profile:

```bash
qdu profile set shared-home --path /home --cross-filesystems --with-users
```

Crossing mounts can substantially increase scan time and network traffic. Verify the mount layout first with tools such as `findmnt`, `mount`, and `stat`.

qdu examines directory entries without triggering automounts, so an automount point that the scan skips stays unmounted. A directory that qdu enters is opened, which mounts an automount point; with one filesystem, qdu then sees the new filesystem and skips it as well.

An automounter map that mounts entries on demand, such as an autofs `/home` with one entry per user, lists only the entries mounted when it is read, so the others are left out of a snapshot. qdu warns when it reads such a map and records its path; open the entries to include first, for example each home directory of the accounts on the machine:

```bash
getent passwd | cut -d: -f6 | grep '^/home/' | while read -r home; do
  ls "$home" > /dev/null 2>&1
done
qdu snapshot --profile shared-home
```

qdu opens each directory relative to its parent, so paths longer than the system accepts in one call (4,096 bytes on Linux, 1,024 on macOS) are read too. A directory reached twice, through a bind mount or a mount loop, is read once.

## Exclusion patterns

```bash
qdu snapshot \
  --exclude .git \
  --exclude node_modules \
  --exclude 'cache/**'
```

Patterns follow the gitignore syntax, except that negation (`!`) is not supported:

- A pattern without a slash, such as `.git` or `*.tmp`, matches a file or directory name at any depth.
- A pattern with a slash at the beginning or in the middle, such as `/build` or `data/raw`, matches the path from the scan root. Start it with `**/` to match at any depth.
- A trailing slash, as in `logs/`, matches directories only.
- `*` and `?` match within one name, and `[abc]` and `[!abc]` are character classes, which may hold POSIX classes such as `[[:digit:]]`. `**/` matches any number of directories, `/**` everything inside a directory, and `a/**/b` zero or more directories in between.
- A backslash makes the next character literal, and matching is case-sensitive.
- A pattern may not start with `#` or `;`, which the configuration file reads as a comment, or have spaces around it; write `\#name`, and `[ ]` for a space in a name.
- qdu does not enter an excluded directory, so nothing below it is counted.

| Pattern | Excludes | Keeps |
|---|---|---|
| `node_modules` | `node_modules` and `a/node_modules`, with their contents | `node_modules2` |
| `*.tmp` | `a.tmp`, `work/a.tmp` | `a.tmp.gz` |
| `/build` | `build` at the scan root | `src/build` |
| `data/raw` | `data/raw` at the scan root | `p/data/raw` |
| `**/models/*.bin` | `models/a.bin`, `run/models/a.bin` | `models/sub/a.bin` |
| `logs/` | Directories named `logs` | A file named `logs` |
| `cache/**` | Everything inside the root's `cache` | `cache` itself |

An exclusion file contains one pattern per line. Blank lines and lines beginning with `#` are ignored.

```text
# build outputs
node_modules
.venv
*.tmp
```

The scanner uses the same exclusion contract for directory, owner, and large-file statistics. A file with several hard links is counted once per scan, in the directory of the link whose path sorts first, so its allocated blocks are not counted twice and repeated scans charge it to the same directory.

## File names that are not UTF-8

A file name that is not valid UTF-8, for example one extracted from an archive created with another encoding, is stored with each undecodable byte spelled as `\xNN`, such as `report-\x93\xfa.txt`. In an exclusion pattern, a backslash escapes the next character, so write `report-\\x93*` for that name, or use a wildcard such as `report-*`. If another entry in the same directory is literally named `report-\x93\xfa.txt`, both are stored as one path and their sizes are added together.

## Permissions and changes during scanning

Run qdu as the user whose accessible view you want to measure. Do not schedule it with `sudo` merely to suppress permission errors: they do not make a snapshot incomplete. Files can change while qdu walks the tree; entries that vanish are left out, and other recoverable failures make the result incomplete and remain inspectable with `qdu errors`.

Continue with the [Analysis guide](ANALYSIS_GUIDE.md).
