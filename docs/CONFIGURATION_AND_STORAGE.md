# Configuration and storage

English | [日本語](CONFIGURATION_AND_STORAGE_ja.md)

## Configuration file

Create the starter file, or add its `[defaults]` section to an existing file that has none, such as one that `qdu profile set` wrote:

```bash
qdu config init
qdu config path
qdu profile show            # the settings of the profile that QDU_PROFILE names, or default
qdu profile show data
```

The default path follows XDG:

```text
${XDG_CONFIG_HOME:-$HOME/.config}/qdu/config.ini
```

An illustrative configuration is:

```ini
[defaults]
keep_snapshots = 52
compact_after = 7d
collect_users = false
user_max_depth = 3
large_file_limit = 1000
record_max_depth =
cross_filesystems = false
capacity_limit =
capacity_user =
exclude =
    .git
    node_modules

[profile:data]
path = /srv/data
keep_snapshots = 60
collect_users = true
cross_filesystems = true
capacity_limit = 2TiB
exclude =
    .cache
    tmp
```

`compact_after` accepts a duration such as `7d` or `off`; see [Operations](OPERATIONS.md#retention-and-compaction). `collect_users` and `cross_filesystems` accept `true`, `yes`, `on`, or `1`, and `false`, `no`, `off`, or `0`; any other value is an error. `path` defaults to your home directory. A profile whose `path` is not set scans the directory of its existing snapshots instead; `qdu profile show` displays the directory it will scan. Command-line overrides take precedence over configuration.

The configuration changes in two ways besides editing the file:

- `qdu profile set NAME` writes only the options it is given, so the other settings of an existing profile are kept and a new profile follows `[defaults]`. A new profile needs `--path`, and a profile with snapshots keeps their directory, because later snapshots must scan the same one. `qdu profile add` is another name for it.
- The first snapshot of a new profile, one with neither a `[profile:NAME]` section nor snapshots, writes the directory, exclusions, and settings it was given into a new section; after that, `qdu snapshot --save` does so.

Without `qdu config init`, no implicit `.git` or `node_modules` exclusions are applied. An exclusion that starts with `#` or `;` would read as a comment, so qdu refuses to store one; see [Exclusion patterns](SNAPSHOTS_AND_SCANNING.md#exclusion-patterns).

## Profile-name boundary

Profile names have at most 64 characters, start with an ASCII alphanumeric character, and then contain only alphanumerics, `.`, `_`, and `-`. Absolute paths, `..`, `/`, and `\` are rejected consistently by configuration operations and state-path construction. An empty name, as from `QDU_PROFILE=`, means the default profile.

Commands that read a profile refuse one that is neither configured nor has snapshots, and suggest the closest known name. A snapshot with `--path` starts a new profile, and `qdu profile list` also lists profiles that have snapshots but no configuration.

This is a security boundary: a profile is a single path component below qdu's private state directory, never a user-selected filesystem path.

## State layout

```text
${XDG_STATE_HOME:-$HOME/.local/state}/qdu/profiles/<profile>/
├── index.json
├── snapshots/
│   ├── <id>.sqlite3
│   └── <id>.sqlite3.gz
├── tmp/
└── .lock
```

`index.json` records snapshot metadata, labels, and selectors. Indexed filenames must be basenames ending in `.sqlite3` or `.sqlite3.gz`; unsafe references are rejected before any read, decompression, verification, or deletion.

Snapshots are versioned SQLite databases. No database server is required. qdu creates state directories and files for the owning user and publishes indexes and compressed artifacts through atomic replacement. Reading an archived snapshot decompresses it into a private temporary file in the system temporary directory (`$TMPDIR`, usually `/tmp`), which is removed afterwards; this avoids writing large copies into a slow or nearly full network home directory.

The index version and SQLite schema version are separate compatibility contracts. qdu writes snapshot schema 4, whose `scan_errors` table records whether each failure was a denied permission, and still reads schemas 2 and 3 from the Python versions of qdu, 0.1.0 and 0.2.0, counting their failures as errors. The Python versions cannot read schema 4 snapshots. Opening a file that is not a snapshot of a readable schema is a format error, with exit status 4. Use `qdu verify` and `qdu doctor` before manual recovery. Prefer `qdu repair` over editing `index.json` by hand; it keeps the labels of a readable index.

See [Operations and maintenance](OPERATIONS.md) for lifecycle commands.
