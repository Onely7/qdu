# Operations and maintenance

English | [日本語](OPERATIONS_ja.md)

## Verify and diagnose

```bash
qdu verify
qdu verify --snapshot latest
qdu doctor
qdu doctor --profile data
```

`verify` checks the indexed file, checksum, gzip materialization, SQLite integrity, schema version, and stored invariants. `doctor` reports configuration, state-directory, index, snapshot, and lock health, when the newest snapshot was taken, whether the profile root still matches its snapshots, and whether `qdu schedule` has installed a crontab entry. A configuration it cannot read is a `FAILED` row, after which it checks what it still can. It changes nothing: a missing state directory is checked for whether it could be created, and the lock file is not created to inspect it.

## Repair

Preview recovery before replacing an index:

```bash
qdu repair --dry-run
qdu repair
```

Repair scans only valid snapshot basenames under the profile's snapshot directory, validates candidate databases, reconstructs records, and atomically saves a new index, keeping the labels of the old index when it can still be read. Invalid or unrelated files are not imported. It also removes partial files that an interrupted command left behind, such as an unfinished `.gz.tmp` archive, and leaves anything in the profile's `tmp` directory that is not a regular file.

## Retention and compaction

After each snapshot, qdu removes the oldest unlabeled snapshots beyond `keep_snapshots` (default 52, about a year of weekly snapshots) and gzips snapshots older than `compact_after` (default `7d`). Labeled snapshots are kept and do not count toward `keep_snapshots`. The two newest snapshots stay uncompressed so that `show` and `diff` read them directly. An archived snapshot takes about a seventh of the space and remains usable by every command. Set either per profile, or for one run:

```bash
qdu profile set data --path /srv/data --keep-snapshots 104 --compact-after 30d
qdu snapshot --compact-after off
```

Compact manually with other limits; `--dry-run` lists what it would compress without taking the lock:

```bash
qdu compact --older-than 30d --keep-latest 5 --dry-run
qdu compact --older-than 30d --keep-latest 5
```

`qdu list` shows the disk space each snapshot file uses, which helps choose retention and compaction settings.

Compaction preserves protected recent and selector snapshots, creates `.sqlite3.gz` atomically, updates the checksum and index, and then removes the uncompressed source.

## Labels and forgetting snapshots

A label names one snapshot of a profile, so that `@NAME` selects it, and keeps it from retention:

```bash
qdu snapshot --label before-cleanup        # label a new snapshot
qdu label baseline --snapshot 2026-01-31   # label an existing one
qdu diff --from @baseline
qdu label baseline --remove
```

Giving a label to another snapshot moves it there. `qdu list` shows the labels.

`qdu forget` removes chosen snapshots and their files; any [snapshot selector](COMMAND_REFERENCE.md#snapshot-selectors) names them:

```bash
qdu forget latest~5 latest~6 --dry-run
qdu forget latest~5 latest~6
```

## Locks

Commands that change a profile's snapshots or labels, and `profile remove --delete-data`, take an advisory lock on the profile. A second such command waits up to two seconds, which covers the moment `qdu doctor` holds the lock to test it, and then exits with status 5 while the first runs. The error and `qdu doctor` show the process that holds the lock. The operating system releases the lock when that process exits, even after a crash, so a lock never needs to be removed by hand. A process that waited for a lock file that was removed meanwhile, for example by `qdu profile remove --delete-data`, locks the file now at the path instead. `profile remove --delete-data` takes the lock before it changes anything, so a busy profile stays as it was.

## Automation checks

`qdu check` evaluates one or more policies and returns a distinct alert status when a threshold is exceeded:

```bash
qdu check data --growth-over 10GiB
qdu check --disk-usage-over 85
qdu check --inode-usage-over 80
qdu check --snapshot-age-over 8d
qdu check --capacity-limit 2TiB
```

`--snapshot-age-over` alerts when the checked snapshot, the latest complete one by default, is older than the given duration, which reveals scheduled snapshots that stopped running or keep failing. Owner-specific capacity checks require a snapshot collected with `--with-users`. `--disk-usage-over` and `--inode-usage-over` measure the filesystem now, with disk usage computed as `df` computes it: used space over used plus available space, so the blocks reserved for root count as neither and a full disk reads 100%; for a snapshot taken on another host, such as a copy that a dashboard gathers from several servers, they judge the usage that the snapshot recorded. In TSV and JSON, each check reports `observed` and `threshold` as numbers in its `unit` (`bytes`, `percent`, or `seconds`), besides the text that the table shows.

## Scheduling

Where cron is available, `qdu schedule` writes the crontab entry for you. It prints the entry by default and installs it with `--install`:

```bash
qdu schedule --profile data                       # print a weekly entry, Monday 03:15
qdu schedule --profile data --every daily --at 02:30 --install
qdu schedule --profile data --remove
```

`--weekday` sets the day of weekly snapshots and is refused with `--every daily` or `monthly`, which run every day and on the 1st.

The entry runs the installed qdu by its absolute path with `--quiet`; when qdu was started through a link on `PATH`, such as the `~/.local/bin/qdu` that `uv tool install` and `pipx` create, the entry keeps that link, so it still works after an upgrade. Run `qdu schedule` from an installed qdu: the copy that `uvx qdu` runs lives in uv's cache, which uv may delete, and qdu warns when you do. The entry appends warnings and errors to `snapshot.log` in the profile's state directory, and carries over `XDG_CONFIG_HOME` and `XDG_STATE_HOME` when they are set. Each profile has one entry, marked `# qdu:<profile>`; installing again replaces it, and other crontab lines are kept byte for byte. `qdu profile remove` removes the profile's entry as well. Pair it with `qdu check --snapshot-age-over 8d` to notice a schedule that stopped working.

Do not overlap jobs for the same profile. qdu's lock will prevent corruption, but repeated overlaps indicate a scheduling problem. See [Weekly snapshots with tmux](QDU_TMUX_WEEKLY_SNAPSHOT_GUIDE.md) when cron or a service manager is unavailable.

## Exit codes

| Code | Meaning |
|---:|---|
| `0` | Successful command; checks passed |
| `1` | Operational or unexpected failure; `doctor` found a failed check |
| `2` | Invalid CLI usage, configuration, selector, profile, or input boundary; `profile list` met a profile it could not read |
| `3` | A snapshot was saved but is incomplete; denied permissions alone do not cause this |
| `4` | Malformed or incompatible stored snapshot/index data |
| `5` | Busy profile or another process owns the lock |
| `6` | Integrity verification failed |
| `10` | A requested `check` threshold was exceeded |
| `130` | Interrupted by Ctrl-C (SIGINT); SIGTERM gives `143` and SIGHUP `129` |

Scripts should test the exact documented code instead of parsing human-readable messages. Use JSON or TSV for data interchange.
