# qdu

English | [日本語](README_ja.md)

`qdu` (quick `du`) records directory usage as snapshots, so you can inspect rankings and changes without rescanning a large tree for every query.

```bash
qdu snapshot   # record the current state
qdu show       # display the largest directories
qdu diff       # display changes since the previous snapshot
```

It does not require `sudo`. It scans what the current user can read and records recoverable access failures in an incomplete snapshot.

## Features

- Persistent directory-usage snapshots and ranked diffs
- Allocated-size, apparent-size, file-count, and inode analysis
- Optional owner statistics and large-file candidates
- Staleness annotations and operational capacity limits
- Named profiles for multiple scan roots
- Explicit one-filesystem or cross-filesystem scanning
- Human-readable tables plus stable TSV and JSON output
- Verification, repair, retention, and gzip compaction

## Requirements

- Linux or macOS
- Python 3.10 through 3.14
- No third-party runtime Python packages

Only `qdu browse` requires the optional [`fzf`](https://github.com/junegunn/fzf) executable.

## Install

Build the tracked source into a zipapp and install it in a user-writable directory:

```bash
make build
mkdir -p "$HOME/.local/bin"
install -m 755 ./dist/qdu "$HOME/.local/bin/qdu"
export PATH="$HOME/.local/bin:$PATH"
qdu --version
```

You can use `./install.sh` for the same local installation workflow.

## First use

With no profile or path option, qdu uses the `default` profile and scans your home directory.

```bash
qdu snapshot
qdu show --max-depth 2 --top 30

# Take another snapshot later, then compare complete snapshots.
qdu snapshot
qdu diff --growth-only
```

Snapshots contain measurements and paths, not file contents.

## Profiles

Use profiles to keep scan roots, retention, and analysis options separate:

```bash
qdu profile add project-data --path /data/my-project
qdu snapshot --profile project-data
qdu show --profile project-data
```

Profile names may contain ASCII letters, digits, `.`, `_`, and `-`. Absolute paths, `..`, and path separators are rejected before configuration or state files are accessed.

For autofs, NFS, or another layout in which child directories are separate filesystems, opt in explicitly:

```bash
qdu profile add shared-home \
  --path /home \
  --cross-filesystems \
  --with-users
```

## Common analysis

```bash
# Stale areas and files
qdu show --stale-only 90 --max-depth 4 --top 100
qdu files --stale-only 90 --top 30

# Capacity policy
qdu show --capacity-limit 2TiB
qdu check --capacity-limit 2TiB

# Files and inodes
qdu files --top 30
qdu inodes --max-depth 3 --top 30
```

## Storage

Configuration:

```text
${XDG_CONFIG_HOME:-$HOME/.config}/qdu/config.ini
```

Snapshot state:

```text
${XDG_STATE_HOME:-$HOME/.local/state}/qdu/profiles/<profile>/
```

qdu validates profile path components and indexed snapshot basenames before filesystem operations. SQLite values are bound parameters, and query metrics are selected from fixed allow lists.

## Documentation

| Document | Purpose |
|---|---|
| [Documentation index](docs/README.md) | Find the appropriate guide |
| [Getting started](docs/GETTING_STARTED.md) | Installation and first workflow |
| [Snapshots and scanning](docs/SNAPSHOTS_AND_SCANNING.md) | Permissions, exclusions, mounts, NFS, and autofs |
| [Analysis guide](docs/ANALYSIS_GUIDE.md) | Rankings, diffs, users, files, inodes, and staleness |
| [Configuration and storage](docs/CONFIGURATION_AND_STORAGE.md) | Profiles, XDG files, and snapshot state |
| [Operations](docs/OPERATIONS.md) | Verification, repair, compaction, checks, and exit codes |
| [Troubleshooting](docs/TROUBLESHOOTING.md) | Common failures and diagnostics |
| [Command reference](docs/COMMAND_REFERENCE.md) | Commands and option families |
| [Development](docs/DEVELOPMENT.md) | Tests, quality checks, CI, security, and layout |
| [Weekly snapshots with tmux](docs/QDU_TMUX_WEEKLY_SNAPSHOT_GUIDE.md) | Scheduling without cron |

Run `qdu --help` or `qdu <command> --help` for the authoritative options accepted by the installed version.
