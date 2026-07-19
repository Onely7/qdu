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

Patterns are matched against a relative path and its basename. Plain names such as `.git` match a component; glob syntax supports `*`, `?`, and character classes.

```bash
qdu snapshot \
  --exclude .git \
  --exclude node_modules \
  --exclude 'cache/**'
```

An exclusion file contains one pattern per line. Blank lines and lines beginning with `#` are ignored.

```text
# build outputs
node_modules
.venv
*.tmp
```

The scanner uses the same exclusion contract for directory, owner, and large-file statistics. Hard-linked regular files are counted once per scan to avoid double-counting allocated blocks.

## Permissions and changes during scanning

Run qdu as the user whose accessible view you want to measure. Do not schedule it with `sudo` merely to suppress permission errors. Files can change while walking; recoverable failures make the result incomplete and remain inspectable with `qdu errors`.

Continue with the [Analysis guide](ANALYSIS_GUIDE.md).

