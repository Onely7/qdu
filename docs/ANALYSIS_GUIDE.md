# qdu analysis guide

English | [日本語](ANALYSIS_GUIDE_ja.md)

## Directory rankings

`show` reads a stored snapshot; it does not walk the filesystem again.

```bash
qdu show --max-depth 3 --top 50
qdu show --under Library/Caches
qdu show --min-size 1GiB
qdu show --match cache
qdu show --match 'models/*'
```

The default metric is allocated bytes. Compare logical file lengths when needed:

```bash
qdu show --metric apparent
```

Filters are applied inside the selected `--under` scope. Paths in table/TSV/JSON output remain deterministic, and an empty match is rendered explicitly rather than falling back to another scope.

## Capacity policy

Compare the profile root with an operational limit:

```bash
qdu show --capacity-limit 2TiB
qdu check --capacity-limit 2TiB
```

Persist a policy on a profile:

```bash
qdu profile add shared-home --path /home --capacity-limit 2TiB
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
qdu show --stale-only 90 --stale-thresholds 30,90,180,365
qdu files --stale-only 180
```

Age is measured from the snapshot creation time to each stored last-modified timestamp, so an old snapshot remains internally consistent. A live file query uses the current time.

## Diffs

`diff` normally compares the latest complete snapshot with its previous complete snapshot:

```bash
qdu diff
qdu diff --growth-only --top 50
qdu diff --shrink-only
qdu diff --order absolute
qdu diff --from 20260701 --to latest
```

`--metric` compares `allocated` bytes (the default), `apparent` bytes, or `inodes`. Inodes reveal where many small files appeared even when they take little space:

```bash
qdu diff --metric inodes --growth-only
```

Each entry reports the previous and current values, the change, and the change relative to the previous value (`change_percent` in JSON). For a path that did not exist before, the table shows `new` and JSON has `null`. `--min-size` keeps entries whose larger value reaches the threshold; with `--metric inodes` it is an inode count.

Orders are `growth`, `shrink`, `absolute`, and `current`. Selectors include `latest`, `latest-any`, `previous`, an exact snapshot ID or filename, and an unambiguous ID prefix.

## Drill down

```bash
qdu explain Library/Caches
qdu explain Library/Caches --diff
```

If `fzf` is installed, choose from recorded directories interactively:

```bash
qdu browse
```

Directories are listed largest first with their allocated size, and typing filters on the path only.

qdu resolves the executable to an absolute path and invokes it without a shell.

## Owners

Owner analysis requires a snapshot created with `--with-users`:

```bash
qdu users --top 20 --dirs 5
qdu users --metric inodes
qdu show --users
qdu inodes --users
```

`show --users` adds a `users` array to JSON output. TSV output holds a single table, so use `qdu users --format tsv` for owner rows. Without owner statistics, `--users` stops with the same usage error as `qdu users`.

Numeric UIDs work even when an account name no longer resolves. Owner directory aggregation is limited by the configured `user_max_depth`.

## Large files

```bash
qdu files --top 30 --min-size 1GiB
qdu files --under projects --user alice
qdu files --live --path /srv/data
```

Snapshot mode queries only the retained `file_top` candidates. Use `--live` when you require the current filesystem view or a candidate outside the stored limit. A live scan crosses into other filesystems only when the profile sets `cross_filesystems`, as a snapshot does.

## Inodes

```bash
qdu inodes --max-depth 3 --top 30
qdu inodes --under cache --match wheels
qdu inodes --users --dirs 5
```

Snapshot and current filesystem free-inode values are shown when the platform supplies them.

## Output formats

```bash
qdu show --format table --style plain --color never
qdu show --format tsv
qdu show --format json
```

Use JSON or TSV for automation. Control characters in terminal cells are escaped so stored paths cannot inject terminal escape sequences. JSON preserves the original string value.

See the [Command reference](COMMAND_REFERENCE.md) for option defaults.
