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

Inspect the newest incomplete result and its captured errors:

```bash
qdu show --snapshot latest-any
qdu errors --snapshot latest-any
```

Correct permissions, exclusions, or mount scope, then take another snapshot.

## Invalid profile name

Names are identifiers, not paths. Use alphanumerics, `.`, `_`, and `-`. qdu intentionally rejects absolute paths, `..`, and separators before reading or deleting profile files.

```bash
qdu profile add project-data --path /srv/project-data
```

If an old hand-edited config has an invalid section name, rename it to a valid profile identifier and run `qdu doctor`.

## Profile already bound to another root

qdu does not silently repurpose stored history. Create another profile:

```bash
qdu profile add another-data --path /another/data
```

## `qdu browse` is unavailable

```bash
command -v fzf
```

Install `fzf`, or use `qdu show` and `qdu explain PATH`. qdu executes only the resolved `fzf` binary without a shell.

## `/home` completes immediately with `0B`

Autofs or NFS children may have different device IDs and are skipped by the one-filesystem default.

```bash
findmnt -T /home
findmnt -R /home | head -n 100
stat -c 'path=%n device=%d' /home /home/"$USER"
```

If those paths are intended scan targets:

```bash
qdu profile add shared-home --path /home --cross-filesystems --with-users
qdu snapshot --profile shared-home
```

## Values differ from `du` or a file manager

Check allocated versus apparent size, exclusions, unreadable paths, mount boundaries, hard-link deduplication, and changes since snapshot creation.

```bash
qdu show --metric apparent_bytes
qdu errors --snapshot latest-any
qdu config show
```

## Deep directories are absent

Review query filters (`--max-depth`, `--under`, `--match`, `--min-size`) and the collection-time `record_max_depth`. Data below the recorded depth requires a new snapshot.

## Verification or index failure

Do not delete files first. Preserve the state directory, then run:

```bash
qdu doctor
qdu verify --all
qdu repair --dry-run
```

An index that references a parent path, absolute path, arbitrary extension, or nested filename is rejected before any referenced file operation. If valid databases remain under `snapshots/`, `qdu repair` can reconstruct the index.

## Stale lock

Use `qdu doctor` to distinguish an active process from stale metadata. Only then run:

```bash
qdu unlock --profile data
```

## Unexpected output in scripts

Use `--format json` or `--format tsv` and check the exit code. Do not parse colored tables. For human terminal output, qdu escapes control characters so stored path data cannot emit terminal control sequences.
