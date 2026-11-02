# Operations and maintenance

English | [日本語](OPERATIONS_ja.md)

## Verify and diagnose

```bash
qdu verify
qdu verify --snapshot latest
qdu doctor
qdu doctor --profile data
```

`verify` checks the indexed file, checksum, gzip materialization, SQLite integrity, schema version, and stored invariants. `doctor` reports configuration, state-directory, index, snapshot, and lock health, when the newest snapshot was taken, whether the profile root still matches its snapshots, and whether `qdu schedule` has installed a crontab entry.

## Repair

Preview recovery before replacing an index:

```bash
qdu repair --dry-run
qdu repair
```

Repair scans only valid snapshot basenames under the profile's snapshot directory, validates candidate databases, reconstructs records, and atomically saves a new index. Invalid or unrelated files are not imported.

## Retention and compaction

After each snapshot, qdu removes the oldest snapshots beyond `keep_snapshots` (default 52, about a year of weekly snapshots) and gzips snapshots older than `compact_after` (default `7d`). The two newest snapshots stay uncompressed so that `show` and `diff` read them directly. An archived snapshot takes about a seventh of the space and remains usable by every command. Set either per profile, or for one run:

```bash
qdu profile add data --path /srv/data --keep-snapshots 104 --compact-after 30d
qdu snapshot --compact-after off
```

Compact manually with other limits:

```bash
qdu compact --older-than 30d --keep-latest 5
```

`qdu list` shows the disk space each snapshot file uses, which helps choose retention and compaction settings.

Compaction preserves protected recent and selector snapshots, creates `.sqlite3.gz` atomically, updates the checksum and index, and then removes the uncompressed source.

## Locks

Commands that change a profile's snapshots take an advisory lock on the profile, and a second such command exits with status 5 while the first runs. The error and `qdu doctor` show the process that holds the lock. The operating system releases the lock when that process exits, even after a crash, so a lock never needs to be removed by hand.

## Automation checks

`qdu check` evaluates one or more policies and returns a distinct alert status when a threshold is exceeded:

```bash
qdu check --growth-over 10GiB --under data
qdu check --disk-usage-over 85
qdu check --inode-usage-over 80
qdu check --snapshot-age-over 8d
qdu check --capacity-limit 2TiB
```

`--snapshot-age-over` alerts when the checked snapshot, the latest complete one by default, is older than the given duration, which reveals scheduled snapshots that stopped running or keep failing. Owner-specific capacity checks require a snapshot collected with `--with-users`.

## Scheduling

Where cron is available, `qdu schedule` writes the crontab entry for you. It prints the entry by default and installs it with `--install`:

```bash
qdu schedule --profile data                       # print a weekly entry, Monday 03:15
qdu schedule --profile data --every daily --at 02:30 --install
qdu schedule --profile data --remove
```

The entry runs the installed qdu by its absolute path with `--quiet`, appends warnings and errors to `snapshot.log` in the profile's state directory, and carries over `XDG_CONFIG_HOME` and `XDG_STATE_HOME` when they are set. Each profile has one entry, marked `# qdu:<profile>`; installing again replaces it, and other crontab lines are left alone. Pair it with `qdu check --snapshot-age-over 8d` to notice a schedule that stopped working.

Do not overlap jobs for the same profile. qdu's lock will prevent corruption, but repeated overlaps indicate a scheduling problem. See [Weekly snapshots with tmux](QDU_TMUX_WEEKLY_SNAPSHOT_GUIDE.md) when cron or a service manager is unavailable.

## Exit codes

| Code | Meaning |
|---:|---|
| `0` | Successful command; checks passed |
| `1` | Operational or unexpected failure |
| `2` | Invalid CLI usage, configuration, selector, or input boundary |
| `3` | A snapshot was saved but is incomplete |
| `4` | Malformed or incompatible stored snapshot/index data |
| `5` | Busy profile or another process owns the lock |
| `6` | Integrity verification failed |
| `10` | A requested `check` threshold was exceeded |

Scripts should test the exact documented code instead of parsing human-readable messages. Use JSON or TSV for data interchange.
