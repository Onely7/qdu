# qdu analysis guide

English | [日本語](ANALYSIS_GUIDE_ja.md)

## Directory rankings

`show` reads a stored snapshot; it does not walk the filesystem again. Without a command, `qdu` runs `show` with the options given, so `qdu -L 2` ranks two levels of directories.

```bash
qdu show --max-depth 3 --top 50
qdu show Library/Caches
qdu show /home/alice/Library/Caches     # an absolute path inside the root
qdu show --min-size 1GiB
qdu show --match cache
qdu show --match 'models/*'
```

`PATH` is the directory to rank, relative to the snapshot root or an absolute path inside it. The default metric is allocated bytes. Compare logical file lengths, or count inodes to find where many small files are:

```bash
qdu show --metric apparent
qdu show --metric inodes --max-depth 3 --top 30
```

With `--metric inodes`, `--min-size` is an inode count. Filters are applied inside `PATH`. Paths in table, TSV, and JSON output are deterministic, and an empty result is rendered explicitly rather than falling back to another directory.

## Capacity policy

Compare the profile root with an operational limit:

```bash
qdu show --capacity-limit 2TiB
qdu check --capacity-limit 2TiB
```

Persist a policy on a profile:

```bash
qdu profile set shared-home --path /home --capacity-limit 2TiB
```

For an owner-specific policy, collect owner statistics first:

```bash
qdu snapshot --profile shared-home --with-users
qdu show --profile shared-home --capacity-limit 2TiB --capacity-user "$USER"
```

If the snapshot has no owner statistics, `qdu show` warns and omits the capacity result, while `qdu check` fails with exit code 2. `--no-capacity-limit` suppresses a saved policy for one invocation. `--capacity-profile` ignores a saved user selector and assesses the whole root.

## Stale data

```bash
qdu show --stale
qdu show --stale-only 90d --stale-thresholds 30,90,180,365
qdu files --stale-only 26w
```

`--stale-only` takes a number of days, such as `90`, or a duration of whole days, such as `90d` or `26w`. The Stale column shows the age of every entry in days, and its color shows how stale the entry is. Age is measured from the snapshot creation time to each stored last-modified timestamp, so an old snapshot remains internally consistent. A live file query uses the current time.

## Diffs

`diff` normally compares the latest complete snapshot with its previous complete snapshot:

```bash
qdu diff
qdu diff data --growth-only --top 50
qdu diff --shrink-only
qdu diff --order absolute
qdu diff --from 30d                     # with the snapshot of a month ago
qdu diff --from @before-cleanup --to latest
```

`--from` must name a snapshot older than `--to`. `--metric` compares `allocated` bytes (the default), `apparent` bytes, or `inodes`. Inodes reveal where many small files appeared even when they take little space:

```bash
qdu diff --metric inodes --growth-only
```

Each row reports the previous and current values, the change, and the change relative to the previous value (`change_percent` in JSON). For a path that did not exist before, the table shows `new` and JSON has `null`. `--min-size` keeps rows whose larger value reaches the threshold; with `--metric inodes` it is an inode count.

Orders are `growth`, `shrink`, `absolute`, and `current`. Without `--order`, `--shrink-only` ranks by shrinkage and other diffs by growth; equal changes are listed by path. The [Command reference](COMMAND_REFERENCE.md#snapshot-selectors) lists every way to name a snapshot.

qdu warns when the two snapshots differ in root, exclusion patterns, filesystem traversal, or the depth to which they store directories, since each of these changes the comparison.

## History and forecasts

`history` follows one directory, or the root, through the newest complete snapshots:

```bash
qdu history
qdu history data/models --top 52
qdu history --metric inodes
```

Each row shows the size in one snapshot and the change from the one before; a directory absent from a snapshot is shown as `absent`. The summary fits the growth per week to all rows. For allocated bytes, it says when the filesystem would fill at that rate, from its free space now, or for snapshots taken on another host, from the free space that the newest one recorded; for the root of a profile with a capacity limit, it also says when the root would reach the limit. The root's history comes from the snapshot index; a directory's history reads each snapshot, which takes longer for compressed ones.

## Drill down

`show PATH` lists what is inside a directory, and `diff PATH` how it changed:

```bash
qdu show Library/Caches
qdu diff Library/Caches
```

If `fzf` is installed, choose from recorded directories interactively:

```bash
qdu browse
```

Directories are listed largest first with their allocated size, and typing filters on the path only. qdu resolves the executable to an absolute path and invokes it without a shell. If fzf itself fails, browse reports its exit status; a cancelled choice ends quietly.

## Owners

Owner analysis requires a snapshot created with `--with-users`:

```bash
qdu users --top 20 --dirs 5
qdu users --metric inodes
qdu users projects/shared               # who owns the files in one directory
qdu show projects --owners              # the largest owner of each directory
```

The table ranks owners and then lists each owner's largest directories. With `PATH`, the ranking covers the files below that directory, so a shared directory shows who fills it, whoever owns the directory itself; `PATH` must lie within the `user_max_depth` of the snapshot. `show --owners` adds the largest owner of each ranked directory and that owner's share, leaving it empty below `user_max_depth`. In TSV and JSON, each owner and each of its directories is one row, told apart by `type` (`user` or `directory`).

Numeric UIDs work even when an account name no longer resolves. Owner directory aggregation is limited by the configured `user_max_depth`.

## Large files

```bash
qdu files --top 30 --min-size 1GiB
qdu files projects --user alice
qdu files --live --path /srv/data
```

Snapshot mode queries only the retained `file_top` candidates. Use `--live` when you require the current filesystem view or a candidate outside the stored limit; on a terminal, a live scan shows its progress on standard error. A live scan crosses into other filesystems only when the profile sets `cross_filesystems`, as a snapshot does. `--path` and `--threads` apply to live scans only.

## Output formats

```bash
qdu show --format table --style plain --color never
qdu show --format tsv
qdu show --format json
```

Use JSON or TSV for automation; the [Command reference](COMMAND_REFERENCE.md#output-formats) describes their layout. Tables shorten long paths in the middle only on a terminal, and never shorten snapshot IDs. Control characters and bidirectional-text controls in names are escaped in tables, warnings, and error messages, so stored paths cannot inject terminal escape sequences; JSON keeps the original string value.

See the [Command reference](COMMAND_REFERENCE.md) for option defaults.
