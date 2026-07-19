# Configuration and storage

English | [日本語](CONFIGURATION_AND_STORAGE.md)

## Configuration file

Create the initial file without overwriting an existing one:

```bash
qdu config init
qdu config path
qdu config show
qdu config show --profile data
```

The default path follows XDG:

```text
${XDG_CONFIG_HOME:-$HOME/.config}/qdu/config.ini
```

An illustrative configuration is:

```ini
[defaults]
keep_snapshots = 100
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

Command-line overrides take precedence over configuration. Without `qdu config init`, no implicit `.git` or `node_modules` exclusions are applied.

## Profile-name boundary

Profile names must start with an ASCII alphanumeric character and then contain only alphanumerics, `.`, `_`, and `-`. Empty names, absolute paths, `..`, `/`, and `\` are rejected consistently by configuration operations and state-path construction.

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

`index.json` records snapshot metadata and selectors. Indexed filenames must be basenames ending in `.sqlite3` or `.sqlite3.gz`; unsafe references are rejected before any read, decompression, verification, or deletion.

Snapshots are versioned SQLite databases. No database server is required. qdu creates state directories and files for the owning user and publishes indexes and compressed artifacts through atomic replacement. Archived databases are materialized only in the profile's temporary directory.

The index version and SQLite schema version are separate compatibility contracts. Use `qdu verify` and `qdu doctor` before manual recovery. Prefer `qdu repair` over editing `index.json` by hand.

See [Operations and maintenance](OPERATIONS_en.md) for lifecycle commands.

