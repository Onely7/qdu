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

Sizes accept decimal and binary suffixes such as `GB`, `GiB`, and `TiB`. Durations accept units such as `h` and `d`. Profile-backed options use the saved value unless explicitly overridden.

## Commands

### `qdu snapshot`

Capture a snapshot. Key options: `-p/--path`, repeatable `--exclude`, `--exclude-from`, `--keep-snapshots`, `--with-users`/`--without-users`, `--user-max-depth`, `--file-top`, `--record-max-depth`, `--cross-filesystems`/`--one-file-system`, and `--quiet`.

### `qdu show`

Rank directories. Key options: `--snapshot` (`latest`), `-L/--max-depth` (`1`), `-n/--top` (`20`), `--under` (`.`), `--min-size`, `--match`, `--metric allocated_bytes|apparent_bytes`, `--warn-age`, `--stale`, `--stale-only`, `--stale-thresholds`, `--users`, `--user-top`, and `--user-dir-top`.

Capacity options shared by `show`, `check`, and `profile add` are `--capacity-limit`, `--no-capacity-limit`, `--capacity-user`, and `--capacity-profile`.

### `qdu diff`

Compare snapshots. Use `--from`, `--to`, or `--previous`; scope with `--under`, `--max-depth`, `--top`, `--min-size`, and `--match`; filter with `--growth-only` or `--shrink-only`; order by `growth`, `shrink`, `absolute`, or `current`.

### `qdu list`

List snapshot history. `--top` defaults to `20`; table, TSV, and JSON are supported.

### `qdu users`

Rank owners from a snapshot collected with user statistics. Options: `--snapshot`, `--top` (`10`), `--dirs` (`5`), and `--metric allocated_bytes|inode_count`.

### `qdu errors`

Show recoverable scan errors. `--snapshot` defaults to `latest-any`; `--top` defaults to `100`.

### `qdu files`

Show stored large-file candidates or perform a live file scan with `--live`. Options include `--path` for live mode, `--snapshot`, `--top` (`30`), `--under`, `--min-size`, `--user`, `--stale`, `--stale-only`, and `--stale-thresholds`.

### `qdu inodes`

Rank directory inodes or use `--users` for owner ranking. Directory options include `--snapshot`, `--max-depth`, `--top`, `--under`, and `--match`; owner detail uses `--dirs`.

### `qdu explain PATH`

Show the immediate children below one recorded path. `--diff` switches to change analysis. It accepts the corresponding `show` or `diff` filtering, staleness, owner, and output options.

### `qdu browse`

Select a recorded directory with optional `fzf`, then render it like `show`. It accepts snapshot, ranking, staleness, owner, and output options.

### `qdu check`

Evaluate `--growth-over`, `--disk-usage-over`, `--inode-usage-over`, and/or the configured/explicit capacity policy. `--path` scopes growth. At least one policy must be active. An exceeded policy exits with `10`.

### `qdu compact`

Gzip old databases. Options: `--older-than` (`30d`), `--keep-latest` (`2`), and `--dry-run`.

### `qdu verify`

Verify `--snapshot` (`latest`) or use `--all` for every indexed snapshot.

### `qdu doctor`

Inspect the Python/SQLite environment, configuration, root, state storage, index, lock, and filesystem capacity.

### `qdu repair`

Rebuild the profile index from valid snapshot files. Use `--dry-run` to preview.

### `qdu unlock`

Remove stale lock metadata only when no active process owns the lock.

### `qdu profile`

- `profile list` lists configured profiles.
- `profile show NAME` renders one profile; JSON is available.
- `profile add NAME --path PATH` creates or updates safe settings. It accepts retention, exclusions, owner collection, stored-depth, large-file, capacity, and filesystem-boundary options.
- `profile remove NAME [--delete-data]` removes configuration and optionally private profile state.

Profile names are restricted to alphanumerics plus `.`, `_`, and `-`; path syntax is rejected.

### `qdu config`

- `config path` prints the resolved file path.
- `config init` creates a starter file without overwriting an existing one.
- `config show [--profile NAME]` renders effective settings.

## Snapshot selectors

`latest` selects the newest complete snapshot; `latest-any` includes incomplete snapshots; `previous` selects the eligible predecessor. Exact IDs, indexed filenames, and unambiguous ID prefixes are accepted. Unsafe filenames in an index are rejected as malformed stored data.

## Exit codes

See [Operations and maintenance](OPERATIONS.md#exit-codes) for the stable exit-code contract.

