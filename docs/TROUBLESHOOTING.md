# Troubleshooting

English | [日本語](TROUBLESHOOTING_ja.md)

## `qdu: command not found`

```bash
ls -l "$HOME/.local/bin/qdu"
export PATH="$HOME/.local/bin:$PATH"
qdu --version
```

Persist the PATH change in `~/.bashrc` or `~/.zshrc`.

## `no snapshots are available`

Take a snapshot with the same profile used by the query:

```bash
qdu snapshot --profile data
qdu show --profile data
```

## `no complete snapshot is available`

Every snapshot so far met a failure other than a denied permission, such as an I/O error. Inspect the newest incomplete result and its captured errors:

```bash
qdu show --snapshot latest-any
qdu errors --snapshot latest-any
```

Errors with the kind `error` caused it; fix their cause, exclude those paths, or narrow the mount scope, then take another snapshot. Paths with the kind `denied` do not make a snapshot incomplete.

## A note says the newest snapshot is incomplete

When a command reads `latest`, the newest complete snapshot, and a newer snapshot is incomplete, qdu notes it on standard error. Read the newer one with `--snapshot latest-any`, and see why it is incomplete with `qdu errors`.

## `unknown profile`

The profile is neither configured nor has snapshots; the message suggests the closest known name. Create it with `qdu profile set NAME --path DIR`, or take its first snapshot with `qdu snapshot --profile NAME --path DIR`.

## Invalid profile name

Names are identifiers, not paths. Use up to 64 alphanumerics, `.`, `_`, and `-`, starting with an alphanumeric. qdu intentionally rejects absolute paths, `..`, and separators before reading or deleting profile files.

```bash
qdu profile set project-data --path /srv/project-data
```

If an old hand-edited config has an invalid section name, rename it to a valid profile identifier and run `qdu doctor`.

## Profile holds snapshots of another root

qdu does not silently repurpose stored history: all snapshots of a profile are of one directory. The error appears when `--path`, or the profile's configured `path`, names another directory than the existing snapshots; `qdu doctor` reports a configured mismatch as `Snapshot root`. Pass the original root with `--path`, correct the profile with `qdu profile set NAME --path ORIGINAL`, or create another profile:

```bash
qdu profile set another-data --path /another/data
```

## `qdu browse` is unavailable

```bash
command -v fzf
```

Install `fzf`, or use `qdu show PATH` and `qdu diff PATH`. qdu executes only the resolved `fzf` binary without a shell.

## `/home` completes immediately with `0B`

Autofs or NFS children may have different device IDs and are skipped by the one-filesystem default.

```bash
findmnt -T /home
findmnt -R /home | head -n 100
stat -c 'path=%n device=%d' /home /home/"$USER"
```

If those paths are intended scan targets:

```bash
qdu profile set shared-home --path /home --cross-filesystems --with-users
qdu snapshot --profile shared-home
```

## A warning says that the root is an automount point

An automounter map lists only the entries mounted when the scan reads it, so the snapshot leaves out the rest, such as the home directories nobody used recently. Open the entries first, as [Filesystem boundaries](SNAPSHOTS_AND_SCANNING.md#filesystem-boundaries) shows, and take the snapshot again.

## Values differ from `du` or a file manager

Check allocated versus apparent size, exclusions, unreadable paths, mount boundaries, hard-link deduplication, and changes since snapshot creation. Directories the user may not read are left out of the totals and listed by `qdu errors`.

```bash
qdu show --metric apparent
qdu errors --snapshot latest-any
qdu profile show
```

## Deep directories are absent

Review query filters (`PATH`, `--max-depth`, `--match`, `--min-size`) and the collection-time `record_max_depth`. Data below the recorded depth requires a new snapshot.

## `cannot read snapshot` or exit status 4

A snapshot file is damaged, is not an SQLite database, or has a schema this qdu does not read. Run `qdu verify --all` to find it and `qdu repair --dry-run` to see how the index would be rebuilt.

## Verification or index failure

Do not delete files first. Preserve the state directory, then run:

```bash
qdu doctor
qdu verify --all
qdu repair --dry-run
```

An index that references a parent path, absolute path, arbitrary extension, or nested filename is rejected before any referenced file operation. If valid databases remain under `snapshots/`, `qdu repair` can reconstruct the index.

## Another qdu operation is using this profile

Another command that changes the profile, often a scheduled `qdu snapshot`, is still running; the message and `qdu doctor` show its process. Wait for it to finish. The lock is released when that process exits, even after a crash, so there is nothing to remove by hand.

## Unexpected output in scripts

Use `--format json` or `--format tsv` and check the exit code. Do not parse colored tables; their paths are shortened in the middle on a terminal. For human terminal output, qdu escapes control characters and bidirectional-text controls in tables, warnings, and error messages, so stored path data cannot emit terminal control sequences.
