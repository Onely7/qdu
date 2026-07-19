# Operations and maintenance

English | [日本語](OPERATIONS.md)

## Verify and diagnose

```bash
qdu verify
qdu verify --snapshot latest
qdu doctor
qdu doctor --profile data
```

`verify` checks the indexed file, checksum, gzip materialization, SQLite integrity, schema version, and stored invariants. `doctor` reports configuration, state-directory, index, snapshot, and lock health.

## Repair

Preview recovery before replacing an index:

```bash
qdu repair --dry-run
qdu repair
```

Repair scans only valid snapshot basenames under the profile's snapshot directory, validates candidate databases, reconstructs records, and atomically saves a new index. Invalid or unrelated files are not imported.

## Compact old snapshots

```bash
qdu compact --older-than 30d --keep-latest 5
```

Compaction preserves protected recent and selector snapshots, creates `.sqlite3.gz` atomically, updates the checksum and index, and then removes the uncompressed source.

## Locks

Mutating operations use an owner-scoped profile lock. If a process is still active, qdu refuses concurrent mutation. Inspect with `qdu doctor`; remove metadata only after confirming it is stale:

```bash
qdu unlock --profile data
```

## Automation checks

`qdu check` evaluates one or more policies and returns a distinct alert status when a threshold is exceeded:

```bash
qdu check --growth-over 10GiB --path data
qdu check --disk-usage-over 85
qdu check --inode-usage-over 80
qdu check --snapshot-age-over 8d
qdu check --capacity-limit 2TiB
```

Owner-specific capacity checks require a snapshot collected with `--with-users`.

## Scheduling

Use a user-level scheduler and retain logs. A typical job takes a snapshot and then verifies it:

```bash
qdu snapshot --profile data --quiet && qdu verify --profile data
```

Do not overlap jobs for the same profile. qdu's lock will prevent corruption, but repeated overlaps indicate a scheduling problem. See [Weekly snapshots with tmux](QDU_TMUX_WEEKLY_SNAPSHOT_GUIDE_en.md) when cron or a service manager is unavailable.

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
