# qdu command reference

English | [日本語](COMMAND_REFERENCE_ja.md)

The installed program is authoritative: use `qdu --help` and `qdu <command> --help`. This reference summarizes all commands and option families. Without a command, `qdu` runs `qdu show` with the options given, so `qdu -L 2` is `qdu show -L 2`.

## Global and common options

| Option | Default | Meaning |
|---|---|---|
| `--version` | — | Print version and exit |
| `--profile NAME` | `$QDU_PROFILE` or `default` | Select a profile; an empty name means `default` |
| `--format table\|tsv\|json` | `table` | Select output format |
| `--style auto\|rich\|plain` | `auto` | Select table style |
| `--color auto\|always\|never` | `auto` | Control color; `auto` colors standard output when it is a terminal and warnings when standard error is one, unless the `NO_COLOR` environment variable is set |
| `-h`, `--help` | — | Print contextual help |

`PATH` arguments name a directory relative to the snapshot root, or an absolute path inside it. Metrics may also be given by their output names `allocated_bytes`, `apparent_bytes`, and `inode_count`. Sizes accept decimal suffixes such as `GB` and binary suffixes such as `GiB`; a prefix alone, as in `du`, is binary, so `2T` equals `2TiB`. Durations take a number and one of `s`, `m`, `h`, `d`, or `w`, such as `90d`. Profile-backed options use the saved value unless explicitly overridden.

## Commands

### `qdu snapshot`

Capture a snapshot. Key options: `-p/--path`, repeatable `--exclude`, `--exclude-from`, `--save`, repeatable `--label NAME`, `--keep-snapshots`, `--compact-after`, `--with-users`/`--without-users`, `--user-max-depth`, `--file-top`, `--record-max-depth`, `--cross-filesystems`/`--one-file-system`, `--threads`, which sets the number of scanning threads, and `--quiet`, which also hides the progress line shown on a terminal. `--format json` prints the new snapshot's record, its scan errors, retention, and the settings it saved.

Without `-p/--path` or a configured `path`, a profile scans the directory of its existing snapshots, or your home directory when it has none. The first snapshot of a new profile, one with neither a configuration section nor snapshots, saves the directory, exclusions, and settings it was given; later, `--save` does so.

### `qdu show [PATH]`

Rank the directories below `PATH`. Key options: `--snapshot` (`latest`), `-L/--max-depth` (`1`), `-n/--top` (`20`), `--min-size`, `--match`, `--metric allocated|apparent|inodes`, `--owners`, `--stale`, `--stale-only`, and `--stale-thresholds`. `--owners` names each directory's largest owner and that owner's share, from a snapshot taken with `--with-users`, for directories down to its `user_max_depth`. With `--metric inodes`, `--min-size` is an inode count. `--stale-only` takes days, such as `90`, or a duration of whole days, such as `90d`.

Capacity options shared by `show`, `check`, and `profile set` are `--capacity-limit`, `--no-capacity-limit`, `--capacity-user`, and `--capacity-profile`.

### `qdu diff [PATH]`

Compare snapshots. Use `--from` and `--to` (default: the latest complete snapshot and the one before it), where `--from` must be older; scope with `PATH`, `--max-depth`, `--top`, `--min-size`, and `--match`; filter with `--growth-only` or `--shrink-only`; order with `--order growth|shrink|absolute|current` (default: `shrink` with `--shrink-only`, otherwise `growth`); and compare `--metric allocated|apparent|inodes`.

### `qdu history [PATH]`

Show the value of `PATH`, or of the root, in each of the newest complete snapshots, with the change from the one before. Options: `-n/--top` (`20` snapshots) and `--metric allocated|apparent|inodes`. The summary gives the growth per week and, at that rate, when the filesystem would fill and, for the root, when it would reach the profile's capacity limit.

### `qdu list`

List snapshot history with the labels and the disk space of each snapshot file, and the total for the profile. `--top` defaults to `20`.

### `qdu users [PATH]`

Rank owners from a snapshot collected with user statistics. With `PATH`, rank the owners of the files below that directory, which must lie within the snapshot's `user_max_depth`, and list each owner's directories below it. Options: `--snapshot`, `--top` (`10`), `--dirs` (`5`), and `--metric allocated|inodes`.

### `qdu errors`

Show the paths that a scan could not read, with the kind `denied` or `error`. `--snapshot` defaults to `latest-any`; `--top` defaults to `100`.

### `qdu files [PATH]`

Show stored large-file candidates below `PATH`, or perform a live file scan with `--live`. Options include `--path DIR` and `--threads` for live mode only, `--snapshot`, `--top` (`30`), `--min-size`, `--user`, `--stale`, `--stale-only`, and `--stale-thresholds`. A live scan whose directory cannot be read is a usage error.

### `qdu browse`

Select a recorded directory with optional `fzf`, then render it like `show`. Directories are listed largest first with their allocated size; the query matches paths only. It accepts snapshot, ranking, staleness, and output options.

### `qdu check [PATH]`

Evaluate `--growth-over`, `--disk-usage-over`, `--inode-usage-over`, `--snapshot-age-over`, and/or the configured or explicit capacity policy. `PATH` scopes growth and must be recorded in at least one of the two snapshots; a directory found in only one of them counts as created or removed. At least one policy must be active. An exceeded policy exits with `10`.

### `qdu schedule`

Print, install (`--install`), or remove (`--remove`) the crontab entry that takes this profile's snapshots. Options: `--every daily|weekly|monthly` (`weekly`), `--at HH:MM` (`03:15`), `--weekday` (`mon`; weekly only), and `--qdu PATH` for the executable cron runs.

### `qdu compact`

Gzip old databases. Options: `--older-than` (`7d`), `--keep-latest` (`2`), and `--dry-run`, which only reads. Snapshots are also compressed automatically after each `qdu snapshot`; see `--compact-after`.

### `qdu verify`

Verify `--snapshot` (`latest`) or use `--all` for every indexed snapshot.

### `qdu doctor`

Inspect the qdu and SQLite versions, configuration, root, state storage, lock, index and stored size, the newest snapshot, whether the profile root still matches its snapshots, the crontab entry from `qdu schedule`, and free space, without changing anything. It exits with 1 when a check fails.

### `qdu repair`

Rebuild the profile index from valid snapshot files, keeping labels from a readable index, and remove partial files left by an interrupted command. Use `--dry-run` to preview.

### `qdu label NAME`

Label the snapshot named by `--snapshot` (`latest`), moving the label from the snapshot that had it, or remove it with `--remove`. Labeled snapshots are kept by retention and selected by `@NAME`. `qdu snapshot --label NAME` labels a new snapshot.

### `qdu forget SELECTOR...`

Remove the named snapshots and their files. `--dry-run` lists them first.

### `qdu profile`

- `profile list` lists configured profiles and profiles with snapshots, with their snapshot count, newest snapshot, capacity limit, and root. It accepts `--format`.
- `profile show [NAME]` renders a profile's settings under their configuration keys; without a name, the profile that `QDU_PROFILE` names, or `default`. TSV has one setting and value per row, with one row per exclusion pattern; JSON holds the settings under `settings`.
- `profile set NAME [--path PATH]` creates a profile or changes the given settings of one; `profile add` is another name for it. A new profile needs `--path`; a profile with snapshots keeps their directory. It accepts retention and compaction, exclusions (`--exclude` replaces the stored list), owner collection (`--with-users`/`--without-users`), stored-depth, large-file, capacity, and filesystem-boundary options. Options that are not given keep their stored value; in a new profile they follow the `[defaults]` section.
- `profile remove NAME [--delete-data]` removes the configuration, the crontab entry, and, with `--delete-data`, the private profile state.

Profile names have at most 64 characters: alphanumerics plus `.`, `_`, and `-`, starting with an alphanumeric; path syntax is rejected.

### `qdu config`

- `config path` prints the resolved file path.
- `config init` creates a starter file with a `[defaults]` section, or adds that section to an existing file that has none; other sections are kept.

### `qdu completions SHELL`

Print a completion script for `bash`, `zsh`, `fish`, `elvish`, or `powershell`, for example `qdu completions bash > ~/.local/share/bash-completion/completions/qdu`.

## Snapshot selectors

`--snapshot`, `--from`, `--to`, and `qdu forget` name a snapshot in these ways:

| Selector | Selects |
|---|---|
| `latest` | The newest complete snapshot |
| `latest-any` | The newest snapshot, complete or not |
| `previous`, `latest~1` | The complete snapshot before `latest`; `latest~N` goes N back |
| `30d`, `2w`, `12h` | The newest complete snapshot at least that old |
| `2026-01-31` | The newest complete snapshot taken by the end of that local day |
| `@NAME` | The snapshot labeled `NAME` |
| An ID, a file name, or a unique ID prefix | That snapshot, complete or not |

`qdu errors`, `qdu verify`, `qdu label`, and `qdu forget` count every snapshot for `latest~N`. Unsafe filenames in an index are rejected as malformed stored data.

## Output formats

Tables are for people: they shorten paths in the middle when a terminal is too narrow, and never shorten snapshot IDs. TSV and JSON are for scripts, and their layout changes only with `format_version`:

- JSON is one object with `format_version` (`1`), `command`, and `profile`, then context such as `snapshot`, `scope`, and `metric`, and the records under `rows`.
- TSV prints the same rows, with the JSON keys as its header. Text is escaped as `\t`, `\n`, `\r`, and `\\`; an absent value is empty.
- Ranked rows have a `rank`. Sizes are bytes, ages are days, and times are Unix epochs, with ISO 8601 text where a column says `_at`.

## Exit codes

See [Operations and maintenance](OPERATIONS.md#exit-codes) for the stable exit-code contract.
