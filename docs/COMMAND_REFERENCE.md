# qdu command reference

English | [日本語](COMMAND_REFERENCE_ja.md)

The installed program is authoritative: use `qdu --help` and `qdu <command> --help`. This reference summarizes all commands and option families.

## Global and common options

| Option | Default | Meaning |
|---|---|---|
| `--version` | — | Print version and exit |
| `--profile NAME` | `$QDU_PROFILE` or `default` | Select a profile |
| `--format table\|tsv\|json` | `table` | Select output format |
| `--style auto\|rich\|plain` | `auto` | Select table style |
| `--color auto\|always\|never` | `auto` | Control terminal color |
| `-h`, `--help` | — | Print contextual help |

Metrics may also be given by their output names `allocated_bytes`, `apparent_bytes`, and `inode_count`. Sizes accept decimal suffixes such as `GB` and binary suffixes such as `GiB`; a prefix alone, as in `du`, is binary, so `2T` equals `2TiB`. Durations accept units such as `h` and `d`. Profile-backed options use the saved value unless explicitly overridden.

## Commands

### `qdu snapshot`

Capture a snapshot. Key options: `-p/--path`, repeatable `--exclude`, `--exclude-from`, `--keep-snapshots`, `--compact-after`, `--with-users`/`--without-users`, `--user-max-depth`, `--file-top`, `--record-max-depth`, `--cross-filesystems`/`--one-file-system`, and `--quiet`, which also hides the progress line shown on a terminal.

### `qdu show`

Rank directories. Key options: `--snapshot` (`latest`), `-L/--max-depth` (`1`), `-n/--top` (`20`), `--under` (`.`), `--min-size`, `--match`, `--metric allocated|apparent`, `--warn-age`, `--stale`, `--stale-only`, `--stale-thresholds`, `--users`, `--user-top`, and `--user-dir-top`.

Capacity options shared by `show`, `check`, and `profile add` are `--capacity-limit`, `--no-capacity-limit`, `--capacity-user`, and `--capacity-profile`.

### `qdu diff`

Compare snapshots. Use `--from` and `--to` (default: the latest complete snapshot and the one before it); scope with `--under`, `--max-depth`, `--top`, `--min-size`, and `--match`; filter with `--growth-only` or `--shrink-only`; order by `growth`, `shrink`, `absolute`, or `current`; and compare `--metric allocated|apparent|inodes`. JSON entries carry `previous`, `current`, `change`, and `change_percent`.

### `qdu list`

List snapshot history with the disk space each snapshot file uses and the total for the profile. `--top` defaults to `20`; table, TSV, and JSON are supported.

### `qdu users`

Rank owners from a snapshot collected with user statistics. Options: `--snapshot`, `--top` (`10`), `--dirs` (`5`), and `--metric allocated|inodes`.

### `qdu errors`

Show recoverable scan errors. `--snapshot` defaults to `latest-any`; `--top` defaults to `100`.

### `qdu files`

Show stored large-file candidates or perform a live file scan with `--live`. Options include `--path` for live mode, `--snapshot`, `--top` (`30`), `--under`, `--min-size`, `--user`, `--stale`, `--stale-only`, and `--stale-thresholds`.

### `qdu inodes`

Rank directory inodes or use `--users` for owner ranking. Directory options include `--snapshot`, `--max-depth`, `--top`, `--under`, and `--match`; owner detail uses `--dirs`.

### `qdu explain PATH`

Show the immediate children below one recorded path. `--diff` switches to change analysis. It accepts the corresponding `show` or `diff` filtering, staleness, owner, and output options.

### `qdu browse`

Select a recorded directory with optional `fzf`, then render it like `show`. Directories are listed largest first with their allocated size; the query matches paths only. It accepts snapshot, ranking, staleness, owner, and output options.

### `qdu check`

Evaluate `--growth-over`, `--disk-usage-over`, `--inode-usage-over`, `--snapshot-age-over`, and/or the configured/explicit capacity policy. `--under` scopes growth and must be recorded in at least one of the two snapshots; a directory found in only one of them counts as created or removed. At least one policy must be active. An exceeded policy exits with `10`.

### `qdu schedule`

Print, install (`--install`), or remove (`--remove`) the crontab entry that takes this profile's snapshots. Options: `--every daily|weekly|monthly` (`weekly`), `--at HH:MM` (`03:15`), `--weekday` (`mon`), and `--qdu PATH` for the executable cron runs.

### `qdu compact`

Gzip old databases. Options: `--older-than` (`7d`), `--keep-latest` (`2`), and `--dry-run`. Snapshots are also compressed automatically after each `qdu snapshot`; see `--compact-after`.

### `qdu verify`

Verify `--snapshot` (`latest`) or use `--all` for every indexed snapshot.

### `qdu doctor`

Inspect the Python/SQLite environment, configuration, root, state storage, lock, index and stored size, the newest snapshot, whether the profile root still matches its snapshots, the crontab entry from `qdu schedule`, and free space. It exits with 1 when a check fails.

### `qdu repair`

Rebuild the profile index from valid snapshot files. Use `--dry-run` to preview.

### `qdu profile`

- `profile list` lists configured profiles with their snapshot count, newest snapshot, capacity limit, and root.
- `profile show NAME` renders one profile's settings under their configuration keys. TSV has one key and value per row, with one row per exclusion pattern; JSON is also available.
- `profile add NAME --path PATH` creates or updates safe settings. It accepts retention and compaction, exclusions (`--exclude` replaces the stored list), owner collection (`--with-users`/`--without-users`), stored-depth, large-file, capacity, and filesystem-boundary options. Options that are not given keep their stored value; in a new profile they follow the `[defaults]` section.
- `profile remove NAME [--delete-data]` removes configuration and optionally private profile state.

Profile names are restricted to alphanumerics plus `.`, `_`, and `-`; path syntax is rejected.

### `qdu config`

- `config path` prints the resolved file path.
- `config init` creates a starter file without overwriting an existing one.
- `config show [--profile NAME]` renders a profile's effective settings, like `profile show`.

## Snapshot selectors

`latest` selects the newest complete snapshot; `latest-any` includes incomplete snapshots; `previous` selects the eligible predecessor. Exact IDs, indexed filenames, and unambiguous ID prefixes are accepted. Unsafe filenames in an index are rejected as malformed stored data.

## Exit codes

See [Operations and maintenance](OPERATIONS.md#exit-codes) for the stable exit-code contract.
