# Snapshots and scanning scope

English | [日本語](SNAPSHOTS_AND_SCANNING_ja.md)

## Take a snapshot

```bash
qdu snapshot
qdu snapshot --path /srv/data
qdu snapshot --profile data
```

A profile is bound to its first persisted root. qdu refuses to silently change that root; create another profile for another tree.

Useful collection options are:

```bash
qdu snapshot --with-users
qdu snapshot --record-max-depth 6
qdu snapshot --file-top 2000
qdu snapshot --exclude .cache --exclude '*.tmp'
qdu snapshot --exclude-from ~/.config/qdu/excludes
```

`--with-users` records aggregate owner information. `--record-max-depth` limits stored directory detail, not filesystem traversal. `--file-top` controls how many large-file candidates are retained.

When standard error is a terminal, `qdu snapshot` keeps one progress line up to date: entries and directories scanned so far, the allocation counted, the elapsed time, and the directory being read. `--quiet` hides it, and nothing is written there when standard error is redirected to a file or pipe.

## Complete and incomplete snapshots

Unreadable paths are recorded as scan errors. A snapshot with such errors remains available through `latest-any`, while `latest` continues to select the most recent complete snapshot.

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
qdu profile add shared-home --path /home --cross-filesystems --with-users
```

Crossing mounts can substantially increase scan time and network traffic. Verify the mount layout first with tools such as `findmnt`, `mount`, and `stat`.

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
- `*` and `?` match within one name, and `[abc]` and `[!abc]` are character classes. `**/` matches any number of directories, `/**` everything inside a directory, and `a/**/b` zero or more directories in between.
- A backslash makes the next character literal, and matching is case-sensitive.
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

The scanner uses the same exclusion contract for directory, owner, and large-file statistics. Hard-linked regular files are counted once per scan to avoid double-counting allocated blocks.

## File names that are not UTF-8

A file name that is not valid UTF-8, for example one extracted from an archive created with another encoding, is stored with each undecodable byte spelled as `\xNN`, such as `report-\x93\xfa.txt`. In an exclusion pattern, a backslash escapes the next character, so write `report-\\x93*` for that name, or use a wildcard such as `report-*`.

## Permissions and changes during scanning

Run qdu as the user whose accessible view you want to measure. Do not schedule it with `sudo` merely to suppress permission errors. Files can change while walking; recoverable failures make the result incomplete and remain inspectable with `qdu errors`.

Continue with the [Analysis guide](ANALYSIS_GUIDE.md).
